import type { ReactNode } from "react";
import type { PresetSnapshot } from "@qc-remote/client";
import type { CorOsScreenView } from "./coros-screen-fixture-data";
import type { ExpressionAssignmentState } from "./expression-assignment";
import type { LaneControlPanelState } from "./lane-control-panel";
import type { CorOsParameterEditorProps } from "./parameter-editor";
import { qcDeviceScreenContent, type QcDeviceScreenContentOptions } from "./qc-device-screen-content";
import { qcParameterEditorLeds, type QcParameterEditorLed } from "./qc-parameter-editor-leds";
import { CorOsSceneCommand } from "./scene-editor";
import { QuadCortexSurface, type QcDisplaySurfaceProps, type QcHostSurfaceProps } from "./quad-cortex-surface";
import { QC_DISABLED_CONTEXT_ACTIONS, qcContextActionHandler } from "./use-qc-screen-destination";
import type { QcCpuTelemetry } from "./use-qc-cpu-telemetry";
import type { QcDeviceUndo } from "./use-qc-device-undo";
import type { QcDeviceViews } from "./use-qc-device-views";
import type { QcScreenDestination } from "./use-qc-screen-destination";
import type { useGridWorkflow } from "./use-grid-workflow";
import type { usePresetWorkflow } from "./use-preset-workflow";
import type { useRoutingWorkflow } from "./use-routing-workflow";
import type { useSceneWorkflow } from "./use-scene-workflow";

/**
 * Everything a host hands to the display. All of it is a value or a workflow
 * the host already owns; the display decides what to draw from them, once, so
 * Windows and Android cannot assemble it differently.
 */
export interface QcDeviceDisplayHost {
  snapshot: PresetSnapshot;
  selectedBlockId: string;
  /** A command is running: the scene editor and the scene command show it. */
  pending: boolean;
  destination: QcScreenDestination;
  views: QcDeviceViews;
  cpu: QcCpuTelemetry;
  undo: QcDeviceUndo;
  workflows: Omit<QcDeviceScreenContentOptions["workflows"], "grid"> & {
    grid: ReturnType<typeof useGridWorkflow>;
    scene: ReturnType<typeof useSceneWorkflow>;
    preset: ReturnType<typeof usePresetWorkflow>;
    routing: ReturnType<typeof useRoutingWorkflow>;
  };
  selectScene: QcDeviceScreenContentOptions["selectScene"];
  queueTempo: QcDeviceScreenContentOptions["queueTempo"];
  setParameterMenuInitiallyOpen: QcDeviceScreenContentOptions["setParameterMenuInitiallyOpen"];
  notice(message: string): void;
  /** The editors the host's own workflows have open over the Grid. */
  expressionAssignment?: ExpressionAssignmentState;
  laneControlEditor?: LaneControlPanelState;
  parameterEditor?: CorOsParameterEditorProps;
  /** What the host's own browser, refresh and routing entry points are. */
  openPreset(): void;
  refresh(): void;
  openRouting(row: number, side: "input" | "output"): void;
  /** Development only: a corpus fixture screen, and the production audit page, replace the live screen. */
  fixtureScreenView?: CorOsScreenView | null;
  auditContent?: ReactNode;
}

/** The assembled display. Opaque to hosts: they can only hand it to QcDeviceDisplay. */
export interface QcDeviceDisplayModel {
  /** @internal Everything the surface draws; consumed only by QcDeviceDisplay. */
  readonly surface: QcDisplaySurfaceProps;
  /** @internal The scene copy/swap dialog, which sits over the whole app rather than inside the display. */
  readonly overlay: ReactNode;
  /** The ten encoder-row LEDs while a parameter editor is open, for hosts that draw the unit's switches themselves. */
  readonly hardwareLeds: readonly QcParameterEditorLed[] | undefined;
}

/**
 * Assemble the display from the host's values and workflows: the screen it
 * shows (live content, CPU overlay, development fixtures), what every control
 * on it does, and the overlays it owns. The one place display props are built.
 */
export function createQcDeviceDisplay(host: QcDeviceDisplayHost): QcDeviceDisplayModel {
  const { snapshot, destination, views, cpu, undo, workflows, pending } = host;
  const { scene, preset, routing, grid } = workflows;
  const cpuMonitorOpen = destination.view === "cpu-monitor";
  // The CPU Monitor is laid over the normal Grid, as on the unit, rather than replacing it.
  const cpuMonitor = cpu.monitor(cpuMonitorOpen, destination.returnFocusTarget, () => destination.setView(null));
  const screenContent = host.auditContent ?? qcDeviceScreenContent({
    snapshot, destination, workflows, selectScene: host.selectScene, queueTempo: host.queueTempo,
    setParameterMenuInitiallyOpen: host.setParameterMenuInitiallyOpen, closeScreen: views.close, notice: host.notice
  });
  const contextAction = qcContextActionHandler({
    destination, openSave: preset.openSave, deviceSettings: workflows.deviceSettings, scene, modes: workflows.modes, tempo: workflows.tempo, notice: host.notice
  });
  const surface: QcDisplaySurfaceProps = {
    snapshot,
    selectedBlockId: host.selectedBlockId,
    screenView: host.fixtureScreenView ?? (cpuMonitorOpen ? undefined : destination.view) ?? undefined,
    screenContent,
    cpuMonitor,
    deviceInformation: views.deviceInformation,
    onCloseScreen: () => void views.close(),
    onDeviceScreenGesture: (x, y, toX, toY, kind) => void views.forwardGesture(x, y, toX, toY, kind),
    onOpenGigView: () => void views.toggle("gig"),
    onOpenIoView: () => void views.toggle("io"),
    onEditScene: scene.openLabelColor,
    onRecallGigPreset: (entry) => void preset.recall(entry),
    onCopyGigScene: (index) => scene.openCommandForScene(index, false),
    onSwapGigScene: (index) => scene.openCommandForScene(index, true),
    sceneEditor: scene.labelColorOpen ? {
      sourceScene: scene.sourceScene, setSourceScene: scene.setSourceScene, scenes: snapshot.scenes, label: scene.label, setLabel: scene.setLabel,
      color: scene.color, setColor: scene.setColor, colors: scene.colors, pending,
      saveLabel: () => void scene.saveLabel(), saveColor: () => void scene.saveColor(), onClose: scene.closeLabelColor
    } : undefined,
    onOpenPreset: host.openPreset,
    onUndo: () => void undo.undo(),
    canUndo: undo.canUndo,
    undoLabel: undo.undoLabel,
    onSave: preset.openSave,
    onOpenRouting: host.openRouting,
    onRefresh: host.refresh,
    savePreset: preset.saveProps,
    presetDirectory: preset.directoryProps,
    gigPresetList: preset.presetList,
    routingPicker: routing.pickerProps,
    addDevicePicker: grid.addPickerProps,
    expressionAssignment: host.expressionAssignment,
    laneControlEditor: host.laneControlEditor,
    parameterEditor: host.parameterEditor,
    onContextAction: contextAction,
    disabledContextActions: QC_DISABLED_CONTEXT_ACTIONS
  };
  // Copy/swap is the Grid context-menu command; it is a dialog over the whole app, not part of the display's canvas.
  const overlay = scene.command ? <CorOsSceneCommand scenes={snapshot.scenes} sourceScene={scene.sourceScene} swap={scene.command === "swap"} pending={pending} onSelect={(destinationScene) => void scene.chooseDestination(destinationScene)} onCancel={scene.closeCommand} returnFocusTarget={scene.commandReturnTarget} /> : null;
  return { surface, overlay, hardwareLeds: qcParameterEditorLeds(host.parameterEditor) };
}

/** The on-device display, drawn from an assembled model. A host adds only what it decides itself. */
export function QcDeviceDisplay({ display, ...host }: { display: QcDeviceDisplayModel } & QcHostSurfaceProps) {
  return <QuadCortexSurface {...host} {...display.surface} />;
}

/** The dialogs the display owns that cover the whole app (scene copy/swap). Render once, outside the display's box. */
export function QcDeviceDisplayOverlay({ display }: { display: QcDeviceDisplayModel }) {
  return <>{display.overlay}</>;
}
