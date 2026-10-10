import { requirePresetIdentity } from "@qc-remote/client";
import { useCallback, useMemo, useState } from "react";
import type { DeviceActionResult, GatewayTransport } from "@qc-remote/client";
import { QC_SCENE_COLOR_ARGB, sceneLetter } from "@qc-remote/core";
import type { DeviceHistoryEntry } from "./use-device-history";
import { useGuardedMutation } from "./use-guarded-mutation.ts";
import type { DeviceMutationWorkflowOptions } from "./workflow-options";

export interface SceneWorkflowOptions extends DeviceMutationWorkflowOptions {
  /**
   * Whether the gateway has the device's own undo history. Scene copy and swap
   * are undone through it, so without it (the demo device) no history entry is
   * recorded: one that can never succeed would sit on top of the stack and
   * block every undo beneath it. Defaults to true.
   */
  deviceUndo?: boolean;
  onOpen?(): void;
  onClose?(): void;
}

const scenePalette = QC_SCENE_COLOR_ARGB.map(argb => `#${(argb & 0xffffff).toString(16).padStart(6, "0")}`);
const isSceneColor = (color: unknown): color is string => typeof color === "string" && /^#[0-9a-fA-F]{6}$/.test(color);
const sceneColorChoices = (observed: string | undefined) => isSceneColor(observed) && !scenePalette.includes(observed.toLowerCase())
  ? [...scenePalette, observed.toLowerCase()] : scenePalette;
const sceneArgb = (hex: string) => 0xff000000 + Number.parseInt(hex.slice(1), 16);

/** Shared scene copy/swap, label, and color workflow. */
export function useSceneWorkflow(options: SceneWorkflowOptions) {
  const { gateway, snapshot, connected, pending, setPending, reconcile, recordHistory, notice, fail, onOpen, onClose } = options;
  const deviceUndo = options.deviceUndo !== false;
  const [sourceScene, setSourceSceneState] = useState(snapshot.activeScene);
  const [destinationScene, setDestinationScene] = useState(1);
  const [swap, setSwap] = useState(false);
  const [label, setLabel] = useState(snapshot.scenes[snapshot.activeScene] ?? "");
  const [color, setColor] = useState(() => {
    const observed = snapshot.sceneColors?.[snapshot.activeScene];
    return Math.max(0, sceneColorChoices(observed).indexOf(observed?.toLowerCase() ?? ""));
  });
  const colors = useMemo(() => sceneColorChoices(snapshot.sceneColors?.[sourceScene]), [snapshot.sceneColors, sourceScene]);
  const setSourceScene = useCallback((scene: number) => {
    if (!Number.isInteger(scene) || scene < 0 || scene >= snapshot.scenes.length) return;
    setSourceSceneState(scene);
    setLabel(snapshot.scenes[scene] ?? "");
    const observed = snapshot.sceneColors?.[scene];
    setColor(Math.max(0, sceneColorChoices(observed).indexOf(observed?.toLowerCase() ?? "")));
  }, [snapshot.sceneColors, snapshot.scenes]);

  const guarded = useGuardedMutation({ connected, pending, setPending, notice, fail }, "editing scenes");
  const unavailable = useCallback(() => !guarded.admit(), [guarded.admit]);

  const open = useCallback(() => {
    if (unavailable()) return;
    const scene = snapshot.activeScene;
    setSourceScene(scene);
    setDestinationScene(scene === snapshot.scenes.length - 1 ? 0 : scene + 1);
    onOpen?.();
  }, [onOpen, setSourceScene, snapshot.activeScene, snapshot.scenes.length, unavailable]);

  const commit = useCallback(async (labelText: string, action: () => ReturnType<GatewayTransport["undo"]>, history?: DeviceHistoryEntry | ((result: DeviceActionResult) => DeviceHistoryEntry)) => {
    await guarded.run({
      progress: `${labelText}…`,
      write: action,
      settle: (result) => {
        if (result.snapshot) reconcile(result.snapshot);
        if (history) recordHistory(typeof history === "function" ? history(result) : history);
        notice(result.detail);
      }
    });
  }, [guarded.run, notice, reconcile, recordHistory]);

  const transfer = useCallback(async (source: number, destination: number, swapMode: boolean) => {
    if (source === destination) { notice("Choose a different destination scene."); return; }
    const verb = swapMode ? "Swap" : "Copy";
    await commit(`${verb}ping scenes`, () => gateway.copyScene(source, destination, swapMode, snapshot.presetName, requirePresetIdentity(snapshot)), deviceUndo ? {
      label: `${verb.toLocaleLowerCase()} scenes`, execute: (current) => gateway.undo(requirePresetIdentity(current)), redo: (current) => gateway.redo(requirePresetIdentity(current))
    } : undefined);
  }, [commit, deviceUndo, gateway, notice, snapshot.presetIdentity, snapshot.presetName]);

  const copy = useCallback(() => transfer(sourceScene, destinationScene, swap), [destinationScene, sourceScene, swap, transfer]);

  const saveLabel = useCallback(async () => {
    const next = label.trim();
    const previous = snapshot.scenes[sourceScene] ?? "";
    if (next === previous) return;
    await commit(`Updating Scene ${sceneLetter(sourceScene)} label`, () => gateway.setSceneLabel(sourceScene, next || null, previous, snapshot.presetName, requirePresetIdentity(snapshot)), (result) => {
      // Only our confirmed steps advance history's expected operands. A newer
      // UI snapshot may contain an external edit, not permission to erase it.
      // Clearing may be reported as an empty string or the default scene name.
      let before = previous;
      let after = result.snapshot?.scenes[sourceScene] ?? next;
      return {
        label: `Scene ${sceneLetter(sourceScene)} label`,
        execute: async (current) => {
          const restored = await gateway.setSceneLabel(sourceScene, previous || null, after, current.presetName, requirePresetIdentity(current));
          before = restored.snapshot?.scenes[sourceScene] ?? previous;
          return restored;
        },
        redo: async (current) => {
          const reapplied = await gateway.setSceneLabel(sourceScene, next || null, before, current.presetName, requirePresetIdentity(current));
          after = reapplied.snapshot?.scenes[sourceScene] ?? next;
          return reapplied;
        }
      };
    });
  }, [commit, gateway, label, snapshot.presetIdentity, snapshot.presetName, snapshot.scenes, sourceScene]);

  const saveColor = useCallback(async () => {
    const previous = snapshot.sceneColors?.[sourceScene];
    const desired = colors[color];
    if (!isSceneColor(previous) || !isSceneColor(desired)) {
      notice("Synchronize the scene color and choose a color before applying it.");
      return;
    }
    if (desired.toLowerCase() === previous.toLowerCase()) return;
    await commit(`Updating Scene ${sceneLetter(sourceScene)} color`, () => gateway.setSceneColor(sourceScene, sceneArgb(desired), previous, snapshot.presetName, requirePresetIdentity(snapshot)), {
      label: `Scene ${sceneLetter(sourceScene)} color`,
      execute: (current) => gateway.setSceneColor(sourceScene, sceneArgb(previous), desired, current.presetName, requirePresetIdentity(current)),
      redo: (current) => gateway.setSceneColor(sourceScene, sceneArgb(desired), previous, current.presetName, requirePresetIdentity(current))
    });
  }, [color, colors, commit, gateway, notice, snapshot.presetIdentity, snapshot.presetName, snapshot.sceneColors, sourceScene]);

  // --- Device-accurate Copy/Swap Scene overlay (Windows) -------------------
  // Reached from the Grid's own context-menu "Copy Scene A"/"Swap Scene A"
  // entries (coros-ui.ts GRID_CONTEXT_MENU, actions "copy-scene"/"swap-scene"),
  // never from an arbitrary source picker: on the real device the source is
  // always whichever scene is active, exactly like `open` above already
  // assumes. `command` names which of the two the overlay is showing; `null`
  // means it is closed. This is additive - `sourceScene`/`destinationScene`/
  // `swap`/`copy` above are untouched, and still what Android's own generic
  // SceneEditor uses.
  const [command, setCommand] = useState<"copy" | "swap" | null>(null);
  // Where focus returns when the user dismisses the overlay: the Grid menu's
  // button for the Grid menu entry; none for Gig View's, whose tile has no
  // stable return target.
  const [commandReturnTarget, setCommandReturnTarget] = useState<"grid-menu" | undefined>(undefined);

  const openCommand = useCallback((swapMode: boolean) => {
    if (unavailable()) return;
    setSourceScene(snapshot.activeScene);
    setCommandReturnTarget("grid-menu");
    setCommand(swapMode ? "swap" : "copy");
  }, [setSourceScene, snapshot.activeScene, unavailable]);

  const openCommandForScene = useCallback((scene: number, swapMode: boolean) => {
    if (unavailable()) return;
    if (!Number.isInteger(scene) || scene < 0 || scene >= snapshot.scenes.length) return;
    setSourceScene(scene);
    setCommandReturnTarget(undefined);
    setCommand(swapMode ? "swap" : "copy");
  }, [setSourceScene, snapshot.scenes.length, unavailable]);

  const closeCommand = useCallback(() => setCommand(null), []);

  const chooseDestination = useCallback(async (destination: number) => {
    if (!command) return;
    await transfer(sourceScene, destination, command === "swap");
    setCommand(null);
  }, [command, sourceScene, transfer]);

  // --- Device scene label/color screen --------------------------------------
  // Gig View owns the entry point, like the physical unit. The selected tile
  // supplies the scene instead of an app-level shortcut silently assuming the
  // active one.
  const [labelColorOpen, setLabelColorOpen] = useState(false);

  const openLabelColor = useCallback((scene = snapshot.activeScene) => {
    if (unavailable()) return;
    setSourceScene(scene);
    setLabelColorOpen(true);
  }, [setSourceScene, snapshot.activeScene, unavailable]);

  const closeLabelColor = useCallback(() => setLabelColorOpen(false), []);

  return {
    sourceScene, setSourceScene, destinationScene, setDestinationScene, swap, setSwap, label, setLabel, color, setColor, colors, open, close: onClose, copy, saveLabel, saveColor,
    commandReturnTarget,
    command, openCommand, openCommandForScene, closeCommand, chooseDestination,
    labelColorOpen, openLabelColor, closeLabelColor
  };
}
