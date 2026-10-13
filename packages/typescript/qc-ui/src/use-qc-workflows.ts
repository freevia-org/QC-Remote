import { bindDeviceCommandScope } from "./device-command-ownership.ts";
import { useCallback, useEffect, useRef, type Dispatch, type SetStateAction } from "react";
import type { ConnectionSource, GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import { SupersededPresetOperation, type QcDeviceTransport } from "@qc-remote/core";
import type { BlockEditorSessionController } from "./use-block-editor-session";
import { useDeviceHistory } from "./use-device-history.ts";
import { useGridWorkflow } from "./use-grid-workflow.ts";
import { useParameterWorkflow } from "./use-parameter-workflow.ts";
import { usePerformanceWorkflow } from "./use-performance-workflow.ts";
import { usePresetWorkflow } from "./use-preset-workflow.ts";
import { useScopedGateway } from "./scoped-gateway.ts";
import type { WorkflowPrompts } from "./workflow-options";
import type { QcController } from "./use-qc-controller";
import { useRoutingWorkflow } from "./use-routing-workflow.ts";
import { useSceneWorkflow } from "./use-scene-workflow.ts";

export interface QcWorkflowPanels {
  openBlock?(): void;
  openAddBlock?(): void;
  /**
   * Open Add Device as the live device browser on the Grid surface
   * (`grid.addPickerProps`) rather than through `openAddBlock`.
   */
  liveAddDevicePicker?: boolean;
  openScenes?(): void;
  close?(): void;
}

export interface UseQcWorkflowsOptions {
  controller: QcController;
  transport: QcDeviceTransport;
  gateway: GatewayTransport;
  /** The host's physical command lease key, shared with device-level workflows. */
  commandSessionKey?: unknown;
  editor: BlockEditorSessionController;
  selectedBlockId: string;
  setSelectedBlockId: Dispatch<SetStateAction<string>>;
  connected: boolean;
  demo: boolean;
  /**
   * Which transport serves the gateway: the real Quad Cortex or the demo
   * device. When it changes, the in-app preset and device clipboards are
   * cleared, because what they point at belongs to the other transport. The
   * demo device has no device undo history, so scene copy and swap record no
   * undo step there. Defaults to "device".
   */
  source?: ConnectionSource;
  pending: boolean;
  setPending: Dispatch<SetStateAction<boolean>>;
  prompts: WorkflowPrompts;
  panels?: QcWorkflowPanels;
  notice(message: string): void;
  fail(error: unknown): void;
  performanceFail?(error: unknown): void;
  onPresetChanged?(snapshot?: PresetSnapshot): void;
  catalogRevision?: number;
}

/**
 * Composes every cross-platform QC editing workflow around one native gateway.
 * Hosts retain presentation, connection lifecycle, and provider integrations;
 * command sequencing, reconciliation, history, and preset transitions live here.
 */
export function useQcWorkflows(options: UseQcWorkflowsOptions) {
  const {
    controller, transport, editor, selectedBlockId, setSelectedBlockId,
    connected, demo, pending, prompts, panels, notice, onPresetChanged
  } = options;
  const source = options.source ?? "device";
  const { lifetime } = controller;
  const gridReady = controller.snapshot.gridLayoutComplete !== false;
  const { gateway, origin } = useScopedGateway(options.gateway, lifetime, controller.snapshot.setlistKey);
  // The host passes fresh callbacks on most renders. Read them through a ref
  // so these wrappers, and every workflow callback built on them, keep one
  // identity for as long as the lifetime checkpoint they guard is unchanged.
  const hostCallbacks = useRef(options);
  hostCallbacks.current = options;
  const setPending = useCallback<Dispatch<SetStateAction<boolean>>>((value) => {
    if (lifetime.current(origin)) hostCallbacks.current.setPending(value);
  }, [lifetime, origin]);
  bindDeviceCommandScope(setPending, options.setPending);
  const fail = useCallback((error: unknown) => {
    if (!(error instanceof SupersededPresetOperation)) hostCallbacks.current.fail(error);
  }, []);
  const performanceFail = useCallback((error: unknown) => {
    if (error instanceof SupersededPresetOperation) return;
    const host = hostCallbacks.current;
    (host.performanceFail ?? host.fail)(error);
  }, []);

  const reconcile = useCallback((incoming: PresetSnapshot) => {
    if (!lifetime.current(origin)) return;
    const next = controller.acceptSnapshot(incoming);
    setSelectedBlockId((current) => next.blocks.some((block) => block.id === current) ? current : "");
  }, [controller, lifetime, origin, setSelectedBlockId]);

  const history = useDeviceHistory({
    gateway, snapshot: controller.snapshot, connected, pending, setPending,
    reconcile, notice, fail
  });

  const finishPresetTransition = useCallback((next?: PresetSnapshot) => {
    invalidate.current();
    setSelectedBlockId((current) => next?.blocks.some((block) => block.id === current) ? current : "");
    editor.close();
    history.clear();
    onPresetChanged?.(next);
  }, [editor, history, onPresetChanged, setSelectedBlockId]);

  const preset = usePresetWorkflow({
    gateway,
    snapshotRef: controller.snapshotRef,
    setSnapshot: controller.setSnapshot,
    acceptSnapshot: controller.acceptSnapshot,
    connected,
    pending,
    setPending,
    prompts,
    notice,
    fail,
    lifetime: controller.lifetime,
    onPresetChanged: (next) => {
      controller.lifetime.invalidate();
      finishPresetTransition(next);
    }
  });

  const routing = useRoutingWorkflow({
    gateway,
    snapshot: controller.snapshot,
    connected: connected && gridReady,
    pending,
    setPending,
    reconcile,
    recordHistory: history.record,
    notice,
    fail
  });

  const grid = useGridWorkflow({
    clearHistory: history.clear,
    gateway,
    snapshot: controller.snapshot,
    selectedBlockId,
    setSelectedBlockId,
    editor,
    connected: connected && !demo,
    topologyComplete: gridReady,
    pending,
    setPending,
    reconcile,
    recordHistory: history.record,
    notice,
    fail,
    closePresetDirectory: preset.closeDirectory,
    onOpenManagement: panels?.openBlock,
    onOpenAdd: panels?.openAddBlock,
    liveAddDevicePicker: panels?.liveAddDevicePicker,
    onClosePanel: panels?.close,
    catalogRevision: options.catalogRevision
  });

  const parameter = useParameterWorkflow({
    controller,
    gateway,
    commandSessionKey: options.commandSessionKey ?? options.gateway,
    snapshot: controller.snapshot,
    editor,
    // Incomplete topology disables block selection in the Grid workflow. Keep
    // this connected flag tied to the actual transport so the parameter hook
    // cannot mistake an incomplete live device for an offline preview session.
    connected: connected && !demo,
    pending,
    setPending,
    reconcile,
    recordHistory: history.record,
    notice,
    fail
  });

  const scene = useSceneWorkflow({
    presetLifetime: lifetime,
    commandSessionKey: options.commandSessionKey ?? options.gateway,
    gateway,
    snapshot: controller.snapshot,
    connected,
    pending,
    setPending,
    reconcile,
    recordHistory: history.record,
    notice,
    fail,
    deviceUndo: source === "device",
    onOpen: panels?.openScenes,
    onClose: panels?.close
  });

  const performance = usePerformanceWorkflow({
    controller,
    transport,
    gateway,
    connected,
    demo,
    projectDeviceScreensLocally: demo || source === "virtual",
    recordHistory: history.record,
    onPresetChanged: () => finishPresetTransition(),
    notice,
    fail: performanceFail
  });

  // Invalidation is synchronous with authoritative state adoption, so even an
  // A-B-A burst batched into one React render revokes old editors and history.
  const invalidate = useRef(() => {});
  invalidate.current = () => {
    grid.close();
    parameter.cancel();
    routing.closePicker();
    grid.closeAddPicker();
    history.clear();
    preset.closeSave();
    options.setPending(false);
  };
  useEffect(() => controller.lifetime.subscribe(() => invalidate.current()), [controller.lifetime]);
  // A clipboard names a preset address, or holds settings read, on the
  // transport that was active when it was filled. Across a swap between the
  // demo device and the Quad Cortex that address can name a different preset
  // entirely, so both clipboards are dropped rather than pasted across.
  const clipboardSource = useRef(source);
  const { clearClipboard: clearPresetClipboard } = preset;
  const { clearClipboard: clearDeviceClipboard } = grid;
  useEffect(() => {
    if (clipboardSource.current === source) return;
    clipboardSource.current = source;
    clearPresetClipboard();
    clearDeviceClipboard();
  }, [clearDeviceClipboard, clearPresetClipboard, source]);
  const wasConnected = useRef(connected);
  useEffect(() => {
    if (wasConnected.current && !connected) controller.resetCommands();
    wasConnected.current = connected;
  }, [connected, controller.resetCommands]);

  return { reconcile, history, preset, routing, grid, parameter, scene, performance };
}
