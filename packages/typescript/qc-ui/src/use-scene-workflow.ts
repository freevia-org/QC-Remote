import { requirePresetIdentity } from "@qc-remote/client";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DeviceActionResult, GatewayTransport } from "@qc-remote/client";
import { QC_SCENE_COLOR_ARGB, sceneLetter, type PresetLifetime } from "@qc-remote/core";
import type { DeviceHistoryEntry } from "./use-device-history";
import { useGuardedMutation } from "./use-guarded-mutation.ts";
import type { DeviceMutationWorkflowOptions } from "./workflow-options";

export interface SceneWorkflowOptions extends DeviceMutationWorkflowOptions {
  /** Retires scene drafts and commands on same-address preset replacement. */
  presetLifetime?: PresetLifetime;
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
  const presetLifetime = options.presetLifetime ?? options.lifetime;
  const [, refreshLifetime] = useState(0);
  useEffect(() => presetLifetime?.subscribe(() => refreshLifetime((value) => value + 1)), [presetLifetime]);
  const context = JSON.stringify([snapshot.stateVersion?.session, snapshot.setlistKey, snapshot.presetPosition,
    presetLifetime?.version ?? snapshot.presetIdentity]);
  type SceneOwner = { gateway: GatewayTransport; context: string; physicalKey: unknown; connected: boolean; current(): boolean };
  const ownerRef = useRef<SceneOwner | undefined>(undefined);
  const mounted = useRef(true);
  const probe = options.captureSessionCurrent?.() ?? (() => true);
  let owner = ownerRef.current;
  if (!owner || owner.gateway !== gateway || owner.context !== context || owner.physicalKey !== options.commandSessionKey
      || owner.connected !== connected || (!owner.current() && probe())) {
    const checkpoint = presetLifetime?.checkpoint();
    const replacement: SceneOwner = { gateway, context, physicalKey: options.commandSessionKey, connected, current: () => false };
    replacement.current = () => mounted.current && ownerRef.current === replacement && probe()
      && (!checkpoint || presetLifetime!.current(checkpoint));
    ownerRef.current = replacement;
    owner = replacement;
  }
  const currentOwner = owner;
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
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
    if (!currentOwner.current()) return;
    if (!Number.isInteger(scene) || scene < 0 || scene >= snapshot.scenes.length) return;
    setSourceSceneState(scene);
    setLabel(snapshot.scenes[scene] ?? "");
    const observed = snapshot.sceneColors?.[scene];
    setColor(Math.max(0, sceneColorChoices(observed).indexOf(observed?.toLowerCase() ?? "")));
  }, [currentOwner, snapshot.sceneColors, snapshot.scenes]);

  const guarded = useGuardedMutation({ connected, pending, setPending, notice, fail, lifetime: options.lifetime,
    sessionKey: currentOwner, commandSessionKey: options.commandSessionKey,
    captureSessionCurrent: () => currentOwner.current }, "editing scenes");
  const unavailable = useCallback(() => !currentOwner.current() || !guarded.admit(), [currentOwner, guarded.admit]);

  const open = useCallback(() => {
    if (unavailable()) return;
    const scene = snapshot.activeScene;
    setSourceScene(scene);
    setDestinationScene(scene === snapshot.scenes.length - 1 ? 0 : scene + 1);
    onOpen?.();
  }, [onOpen, setSourceScene, snapshot.activeScene, snapshot.scenes.length, unavailable]);

  const commit = useCallback(async (labelText: string, action: () => ReturnType<GatewayTransport["undo"]>, history?: DeviceHistoryEntry | ((result: DeviceActionResult) => DeviceHistoryEntry)) => {
    return guarded.run({
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
    if (source === destination) { notice("Choose a different destination scene."); return false; }
    const verb = swapMode ? "Swap" : "Copy";
    return commit(`${verb}ping scenes`, () => gateway.copyScene(source, destination, swapMode, snapshot.presetName, requirePresetIdentity(snapshot)), deviceUndo ? {
      label: "device edit", execute: (current) => gateway.undo(requirePresetIdentity(current)), redo: (current) => gateway.redo(requirePresetIdentity(current))
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
  type Command = { verb: "copy" | "swap"; owner: SceneOwner };
  const [commandState, setCommandState] = useState<Command | null>(null);
  const commandRef = useRef<Command | null>(null);
  const command = commandState?.owner === currentOwner && currentOwner.current() ? commandState.verb : null;
  const setCommand = (verb: "copy" | "swap" | null) => {
    if (!currentOwner.current()) return;
    const next = verb ? { verb, owner: currentOwner } : null;
    commandRef.current = next;
    setCommandState(next);
  };
  // Where focus returns when the user dismisses the overlay: the Grid menu's
  // button for the Grid menu entry; none for Gig View's, whose tile has no
  // stable return target.
  const [commandReturnTarget, setCommandReturnTarget] = useState<"grid-menu" | undefined>(undefined);
  // Whether the panel is the unit's own dialog (opened from the Grid menu, or by the unit): it then shows the
  // unit's title, prompt and CANCEL only, because the destination is a footswitch press on the unit and the app
  // never picks one. Gig View's Copy/Swap tile (below) has no unit dialog behind it and keeps its destination list.
  const [commandMirrorsUnit, setCommandMirrorsUnit] = useState(false);

  const openCommand = useCallback((swapMode: boolean) => {
    if (unavailable()) return;
    setSourceScene(snapshot.activeScene);
    setCommandReturnTarget("grid-menu");
    setCommandMirrorsUnit(true);
    setCommand(swapMode ? "swap" : "copy");
  }, [setSourceScene, snapshot.activeScene, unavailable]);

  /** The unit opened its Copy/Swap Scene dialog: show the same panel. Not gated by a pending write; the unit is already there. */
  const openUnitCommand = useCallback((swapMode: boolean, scene: number) => {
    if (!currentOwner.current()) return;
    if (!Number.isInteger(scene) || scene < 0 || scene >= snapshot.scenes.length) return;
    setSourceScene(scene);
    setCommandReturnTarget(undefined);
    setCommandMirrorsUnit(true);
    setCommand(swapMode ? "swap" : "copy");
  }, [currentOwner, setSourceScene, snapshot.scenes.length]);

  const openCommandForScene = useCallback((scene: number, swapMode: boolean) => {
    if (unavailable()) return;
    if (!Number.isInteger(scene) || scene < 0 || scene >= snapshot.scenes.length) return;
    setSourceScene(scene);
    setCommandReturnTarget(undefined);
    setCommandMirrorsUnit(false);
    setCommand(swapMode ? "swap" : "copy");
  }, [setSourceScene, snapshot.scenes.length, unavailable]);

  const closeCommand = useCallback(() => {
    if (commandRef.current === commandState) setCommand(null);
  }, [commandState, currentOwner]);

  const chooseDestination = useCallback(async (destination: number) => {
    if (!command) return;
    const started = commandState;
    const succeeded = await transfer(sourceScene, destination, command === "swap");
    if (succeeded && currentOwner.current() && commandRef.current === started) setCommand(null);
  }, [command, commandState, currentOwner, sourceScene, transfer]);

  // --- Device scene label/color screen --------------------------------------
  // Gig View owns the entry point, like the physical unit. The selected tile
  // supplies the scene instead of an app-level shortcut silently assuming the
  // active one.
  type LabelDialog = { owner: SceneOwner };
  const [labelColorState, setLabelColorState] = useState<LabelDialog>();
  const labelDialogRef = useRef<LabelDialog | undefined>(undefined);
  const labelColorOpen = labelColorState?.owner === currentOwner && currentOwner.current();

  const openLabelColor = useCallback((scene = snapshot.activeScene) => {
    if (unavailable()) return;
    setSourceScene(scene);
    const dialog = { owner: currentOwner };
    labelDialogRef.current = dialog;
    setLabelColorState(dialog);
  }, [currentOwner, setSourceScene, snapshot.activeScene, unavailable]);

  const closeLabelColor = useCallback(() => {
    if (currentOwner.current() && labelDialogRef.current === labelColorState) {
      labelDialogRef.current = undefined;
      setLabelColorState(undefined);
    }
  }, [currentOwner, labelColorState]);
  const dialogCurrent = () => currentOwner.current() && (!labelColorState || (labelColorState.owner === currentOwner && labelDialogRef.current === labelColorState));

  return {
    sourceScene, setSourceScene: (scene: number) => { if (dialogCurrent()) setSourceScene(scene); },
    destinationScene, setDestinationScene, swap, setSwap, label, setLabel: (value: string) => { if (dialogCurrent()) setLabel(value); },
    color, setColor: (value: number) => { if (dialogCurrent()) setColor(value); }, colors, open, close: onClose, copy,
    saveLabel: () => { if (dialogCurrent()) return saveLabel(); }, saveColor: () => { if (dialogCurrent()) return saveColor(); },
    commandReturnTarget,
    command, commandMirrorsUnit, openCommand, openUnitCommand, openCommandForScene, closeCommand, chooseDestination,
    labelColorOpen, openLabelColor, closeLabelColor
  };
}
