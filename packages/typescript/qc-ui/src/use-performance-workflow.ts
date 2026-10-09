import { requireModelId, requirePresetIdentity } from "@qc-remote/client";
import { useCallback, useRef } from "react";
import type { GatewayTransport, GridBlock } from "@qc-remote/client";
import { adjacentPresetPosition, deviceCommandDetail, footswitchIntent, presetBoundaryDetail, recordTempoTap, samePresetAddress, sceneLetter, type QcDeviceCommand, type QcDeviceTransport } from "@qc-remote/core";
import type { DeviceHistoryEntry } from "./use-device-history";
import type { WorkflowPrompts } from "./workflow-options";
import type { QcController } from "./use-qc-controller";

export interface PerformanceWorkflowOptions {
  controller: QcController;
  transport: QcDeviceTransport;
  gateway: GatewayTransport;
  connected: boolean;
  demo: boolean;
  /** Screen-only commands have no hardware target in a virtual-device session. */
  projectDeviceScreensLocally?: boolean;
  guardDirtyPreset?: boolean;
  recordHistory?: (entry: DeviceHistoryEntry) => void;
  onPresetChanged?: () => void;
  notice(message: string): void;
  fail(error: unknown): void;
}

/**
 * Shared controller for commands that must feel like the QC's physical surface.
 * Native hosts supply transports; optimistic state, rapid repeated input, error
 * recovery, and history semantics stay identical on Windows and Android.
 */
export function usePerformanceWorkflow(options: PerformanceWorkflowOptions) {
  const {
    controller, transport, gateway, connected, demo, projectDeviceScreensLocally = demo,
    guardDirtyPreset = true, recordHistory, onPresetChanged, notice, fail
  } = options;
  const tapTimes = useRef<number[]>([]);

  const selectScene = useCallback(async (index: number, reportFailure = false) => {
    const scene = Math.max(0, Math.min(controller.snapshotRef.current.scenes.length - 1, index));
    if (demo) {
      controller.settleCommand(controller.beginScene(scene));
      notice(`Demo: selected Scene ${sceneLetter(scene)}. Hardware was not changed.`);
      return;
    }
    if (!connected) {
      const error = new Error("Connect the Quad Cortex before selecting a scene.");
      if (reportFailure) throw error;
      notice(error.message);
      return;
    }
    try {
      const result = await controller.runScene(transport, scene);
      notice(result.detail ?? `Scene ${sceneLetter(scene)} selected.`);
    } catch (error) {
      if (reportFailure) throw error;
      fail(error);
    }
  }, [connected, controller, demo, fail, notice, transport]);

  const setBlockBypass = useCallback(async (block: GridBlock, bypassed: boolean, reportFailure = false) => {
    if (demo) {
      controller.settleCommand(controller.beginBypass(block.id, bypassed));
      const detail = `Demo: ${block.name} ${bypassed ? "bypassed" : "enabled"} locally.`;
      notice(detail);
      return detail;
    }
    if (!connected) {
      const error = new Error("Connect the Quad Cortex before changing bypass.");
      if (reportFailure) throw error;
      notice(error.message);
      return;
    }
    try {
      const before = controller.snapshotRef.current;
      const currentBlock = before.blocks.find((candidate) => candidate.id === block.id
        && candidate.row === block.row && candidate.column === block.column
        && candidate.modelId === block.modelId);
      if (!currentBlock) throw new Error("The block changed. Select it again before changing bypass.");
      const previousBypassed = currentBlock.bypassed ?? false;
      const activeScene = before.activeScene;
      const result = await controller.runBypass(transport, block.id, block.row, block.column, bypassed);
      if (previousBypassed !== bypassed) recordHistory?.({
        label: `${previousBypassed ? "enable" : "bypass"} ${block.name}`,
        execute: (current) => gateway.toggleBypass(block.row, block.column, activeScene, bypassed, previousBypassed, requireModelId(currentBlock), current.presetName, requirePresetIdentity(before)),
        redo: (current) => gateway.toggleBypass(block.row, block.column, activeScene, previousBypassed, bypassed, requireModelId(currentBlock), current.presetName, requirePresetIdentity(before))
      });
      const detail = result.detail ?? `${block.name} ${bypassed ? "bypassed" : "enabled"}.`;
      notice(detail);
      return detail;
    } catch (error) {
      if (reportFailure) throw error;
      fail(error);
    }
  }, [connected, controller, demo, fail, gateway, notice, recordHistory, transport]);

  const toggleBlockBypass = useCallback((block: GridBlock, reportFailure = false) => (
    setBlockBypass(block, !(block.bypassed ?? false), reportFailure)
  ), [setBlockBypass]);

  const selectModeSlot = useCallback(async (slot: 0 | 1 | 2) => {
    const selectedMode = controller.snapshotRef.current.modeSlots?.find((entry) => entry.slot === slot)
      ?? { label: (["PRESET", "SCENE", "STOMP"] as const)[slot] };
    if (demo) {
      controller.settleCommand(controller.beginModeSlot(slot));
      notice(`Demo: ${selectedMode.label} mode selected.`);
      return;
    }
    if (!connected) {
      notice("Connect the Quad Cortex before selecting a mode.");
      return;
    }
    try {
      const result = await controller.runModeSlot(transport, slot);
      notice(result.detail ?? `${selectedMode.label} mode selected.`);
    } catch (error) {
      fail(error);
    }
  }, [connected, controller, demo, fail, notice, transport]);

  const pressFootswitch = useCallback(async (index: number) => {
    const label = sceneLetter(index);
    if (demo) {
      controller.settleCommand(controller.beginFootswitch(index));
      notice(`Demo: Footswitch ${label} activated locally; hardware was not changed.`);
      return;
    }
    if (!connected) {
      notice("Connect the Quad Cortex before pressing a footswitch.");
      return;
    }
    try {
      const before = controller.snapshotRef.current;
      const intent = footswitchIntent(before, index);
      const result = await controller.runFootswitch(transport, index);
      const after = controller.snapshotRef.current;
      if (!samePresetAddress(before, after)) onPresetChanged?.();
      else if (intent.kind === "select-scene" && after.activeScene !== before.activeScene) {
        recordHistory?.({
          label: `Scene ${sceneLetter(after.activeScene)}`,
          execute: (current) => gateway.selectScene(before.activeScene, current.presetName, requirePresetIdentity(before)),
          redo: (current) => gateway.selectScene(after.activeScene, current.presetName, requirePresetIdentity(before))
        });
      } else if (intent.kind === "toggle-stomp") {
        recordHistory?.({
          label: `Footswitch ${label}`,
          execute: (current) => gateway.pressFootswitch(index, before.mode, current.presetName, requirePresetIdentity(before)),
          redo: (current) => gateway.pressFootswitch(index, before.mode, current.presetName, requirePresetIdentity(before))
        });
      }
      notice(result.detail ?? `Footswitch ${label} pressed.`);
    } catch (error) {
      fail(error);
    }
  }, [connected, controller, demo, fail, gateway, notice, onPresetChanged, recordHistory, transport]);

  const movePreset = useCallback(async (direction: -1 | 1, reportFailure = false) => {
    const loaded = controller.snapshotRef.current.presetPosition;
    if (adjacentPresetPosition(loaded, direction) === loaded) {
      notice(presetBoundaryDetail(direction));
      return;
    }
    if (demo) {
      controller.settleCommand(controller.beginPresetMove(direction, direction > 0 ? "Next preset" : "Previous preset"));
      notice("Demo: changed preset locally; hardware was not changed.");
      return;
    }
    if (!connected) {
      const error = new Error("Connect the Quad Cortex before navigating presets.");
      if (reportFailure) throw error;
      notice(error.message);
      return;
    }
    let current = controller.snapshotRef.current;
    if (guardDirtyPreset && current.dirty) {
      notice("Discarding the current preset changes…");
      try {
        const reload = await gateway.reloadPreset(current.presetName, current.presetPosition, requirePresetIdentity(current));
        if (!reload.snapshot) throw new Error("The Quad Cortex did not return the reloaded preset.");
        current = controller.acceptSnapshot(reload.snapshot);
        if (current.dirty) throw new Error("Newer unsaved changes arrived while reloading. Navigation was not started.");
      } catch (error) {
        fail(error);
        if (reportFailure) throw error;
        return;
      }
    }
    if (adjacentPresetPosition(current.presetPosition, direction) === current.presetPosition) {
      notice(presetBoundaryDetail(direction));
      return;
    }
    try {
      const result = await controller.runPresetMove(transport, direction);
      onPresetChanged?.();
      notice(result.detail ?? "Preset recalled.");
    } catch (error) {
      fail(error);
      if (reportFailure) throw error;
    }
  }, [connected, controller, demo, fail, gateway, guardDirtyPreset, notice, onPresetChanged, transport]);

  const navigateBank = useCallback(async (direction: -1 | 1, reportFailure = false): Promise<string | undefined> => {
    if (demo || !connected) {
      const error = new Error("Connect the Quad Cortex before navigating banks.");
      if (reportFailure) throw error;
      notice(error.message);
      return undefined;
    }
    try {
      const current = controller.snapshotRef.current;
      const result = await gateway.navigateBank(direction, current.presetName, current.presetPosition, requirePresetIdentity(current));
      if (result.snapshot) controller.acceptSnapshot(result.snapshot);
      onPresetChanged?.();
      notice(result.detail);
      return result.detail;
    } catch (error) {
      fail(error);
      if (reportFailure) throw error;
      return undefined;
    }
  }, [connected, controller, demo, fail, gateway, notice, onPresetChanged]);

  const runDeviceCommand = useCallback(async (command: QcDeviceCommand, reportFailure = false): Promise<string | undefined> => {
    if (demo || !connected) {
      const error = new Error("Connect the Quad Cortex before running that performance command.");
      if (reportFailure) throw error;
      notice(error.message);
      return undefined;
    }
    try {
      const before = controller.snapshotRef.current;
      const result = await controller.runDeviceCommand(transport, command);
      if (command.kind === "tempo") {
        recordHistory?.({
          label: "tempo change",
          execute: (current) => gateway.setTempo(before.tempo, command.bpm, current.presetName, requirePresetIdentity(before)),
          redo: (current) => gateway.setTempo(command.bpm, before.tempo, current.presetName, requirePresetIdentity(before))
        });
      }
      if (command.kind === "preset-step") onPresetChanged?.();
      const detail = deviceCommandDetail(command, result);
      notice(detail);
      return detail;
    } catch (error) {
      if (reportFailure) throw error;
      fail(error);
      return undefined;
    }
  }, [connected, controller, demo, fail, gateway, notice, onPresetChanged, recordHistory, transport]);

  const tapTempo = useCallback(async () => {
    const result = recordTempoTap(tapTimes.current, performance.now());
    tapTimes.current = result.taps;
    const token = result.bpm === undefined ? undefined : controller.beginTempo(result.bpm);
    if (demo) {
      if (token) controller.settleCommand(token);
      notice(result.bpm === undefined ? "Tap again to set the tempo." : `Demo: tempo ${result.bpm} BPM.`);
      return;
    }
    if (!connected) {
      if (token) controller.failCommand(token);
      notice("Connect the Quad Cortex before sending Tap Tempo.");
      return;
    }
    try {
      const sent = await transport.tapTempo(controller.snapshotRef.current);
      notice(sent.detail ?? "Tap Tempo sent.");
    } catch (error) {
      if (token) controller.failCommand(token);
      fail(error);
    }
  }, [connected, controller, demo, fail, notice, transport]);

  const showDeviceView = useCallback(async (view: "tuner" | "gig", visible = true) => {
    const label = view === "tuner" ? "Tuner" : "Gig View";
    if (projectDeviceScreensLocally) {
      notice(`Demo: ${label} ${visible ? "opened" : "closed"}.`);
      return true;
    }
    if (!connected) {
      notice(`Connect the Quad Cortex before ${visible ? "opening" : "closing"} ${view === "tuner" ? "the tuner" : "Gig View"}.`);
      return false;
    }
    try {
      const current = controller.snapshotRef.current;
      const result = view === "tuner" ? await transport.setTuner(visible, current) : await transport.setGigView(visible, current);
      notice(result.detail ?? `${label} ${visible ? "opened" : "closed"}.`);
      return true;
    } catch (error) {
      fail(error);
      return false;
    }
  }, [connected, controller, fail, notice, projectDeviceScreensLocally, transport]);

  return { selectScene, setBlockBypass, toggleBlockBypass, selectModeSlot, pressFootswitch, movePreset, navigateBank, runDeviceCommand, tapTempo, showDeviceView };
}
