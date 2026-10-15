import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createQcScreenLatch, type QcPopupName, type QcScreenLatch } from "./qc-unit-screens.ts";
import { corOsUnavailableContextActionMessage, type CorOsContextAction } from "./coros-ui.ts";
import type { CorOsScreenView } from "./coros-screen-fixture-data.ts";

export type { QcPopupName as QcPopup } from "./qc-unit-screens.ts";
export type QcScreenReturnFocus = "grid-menu";
export type QcScreenViewUpdate = CorOsScreenView | null | ((current: CorOsScreenView | null) => CorOsScreenView | null);

export interface QcScreenDestination {
  /** The device screen the surface is showing in place of the Grid, if any. */
  view: CorOsScreenView | null;
  /** Where focus returns when the screen closes (set only by Grid-menu entries). */
  returnFocusTarget?: QcScreenReturnFocus;
  /** Show or clear a screen. A functional update that changes nothing keeps the
   * previous state object, so an unrelated state publication never re-renders. */
  setView(next: QcScreenViewUpdate): void;
  /** Open a screen and remember where focus returns to. */
  open(view: CorOsScreenView, returnFocusTarget?: QcScreenReturnFocus): void;
  /** The Grid popup that is open (the preset menu, the scene menu, the mode menu): shared with the unit's own menu. */
  popup: QcPopupName | null;
  setPopup(popup: QcPopupName | null): void;
  /** The loop guard shared by everything that moves the app's screen or the unit's. */
  latch: QcScreenLatch;
  captureCurrent(): () => boolean;
  /** The context menu of the open block editor, which the editor owns and reports here so the unit can follow it. */
  blockMenuOpen: boolean;
  setBlockMenuOpen(open: boolean): void;
  /** The Directory's search field, reported the same way. */
  directorySearchOpen: boolean;
  setDirectorySearchOpen(open: boolean): void;
}

/**
 * The one owner of "which device screen is showing" for both hosts. Explicit
 * `setView` calls forget the return-focus target; only `open` carries one.
 */
export function useQcScreenDestination(): QcScreenDestination {
  const generation = useRef(0);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; generation.current++; }; }, []);
  const captureCurrent = useCallback(() => { const origin = generation.current; return () => mounted.current && origin === generation.current; }, []);
  const [state, setState] = useState<{ view: CorOsScreenView | null; returnFocusTarget?: QcScreenReturnFocus }>({ view: null });
  const latestView = useRef<CorOsScreenView | null>(null);
  const setView = useCallback((next: QcScreenViewUpdate) => {
    const view = typeof next === "function" ? next(latestView.current) : next;
    if (typeof next === "function" && view === latestView.current) return;
    generation.current++;
    latestView.current = view;
    setState({ view });
  }, []);
  const open = useCallback((view: CorOsScreenView, returnFocusTarget?: QcScreenReturnFocus) => {
    generation.current++;
    latestView.current = view;
    setState({ view, returnFocusTarget });
  }, []);
  const [popup, publishPopup] = useState<QcPopupName | null>(null);
  const latestPopup = useRef<QcPopupName | null>(null);
  const setPopup = useCallback((next: QcPopupName | null) => {
    if (next === latestPopup.current) return;
    generation.current++;
    latestPopup.current = next;
    publishPopup(next);
  }, []);
  const latch = useMemo(() => createQcScreenLatch(), []);
  const [blockMenuOpen, setBlockMenuOpen] = useState(false);
  const [directorySearchOpen, setDirectorySearchOpen] = useState(false);
  return { view: state.view, returnFocusTarget: state.returnFocusTarget, setView, open, popup, setPopup, latch, captureCurrent, blockMenuOpen, setBlockMenuOpen, directorySearchOpen, setDirectorySearchOpen };
}

/** Grid-menu entries the device cannot honour through the semantic gateway, with the reason shown on the entry. */
export const QC_DISABLED_CONTEXT_ACTIONS: Readonly<Partial<Record<CorOsContextAction, string>>> = {
  "create-new": "Creating an empty preset is not exposed by the current semantic gateway.",
  favorite: "Favorites require library identity that is not available for the active working buffer.",
  "delete-preset": "Delete the preset from the Preset Directory, where its exact stored position can be verified.",
  "new-capture": "The Neural Capture creation workflow is not exposed by the current semantic gateway."
};

export interface QcContextActionOptions {
  destination: Pick<QcScreenDestination, "open">;
  /** Opens the Save/Edit Details dialog. */
  openSave(): unknown;
  deviceSettings: { openSettings(returnFocusTarget?: QcScreenReturnFocus): void };
  scene: { openCommand(swap: boolean): void };
  modes: { refresh(): unknown };
  tempo: { openTempo(returnFocusTarget?: QcScreenReturnFocus): void };
  notice(message: string): void;
}

/**
 * Routes a Grid context-menu action to the workflow or screen that owns it.
 * Not a hook: the workflow objects change identity every render, so the
 * handler is rebuilt each render and reads them fresh.
 */
export function qcContextActionHandler(options: QcContextActionOptions) {
  const { destination, openSave, deviceSettings, scene, modes, tempo, notice } = options;
  return (action: CorOsContextAction, returnFocusTarget?: QcScreenReturnFocus): void => {
    if (action === "edit-details") void openSave();
    // The Grid's own Settings row opens the device's real multi-section Settings
    // screen (Account/System/Device/MIDI) in-app, never the app-preferences window.
    else if (action === "settings") deviceSettings.openSettings(returnFocusTarget);
    else if (action === "copy-scene") scene.openCommand(false);
    else if (action === "swap-scene") scene.openCommand(true);
    else if (action === "preset-midi-out") destination.open("midi-out", returnFocusTarget);
    else if (action === "modes-configuration") { destination.open("modes", returnFocusTarget); void modes.refresh(); }
    else if (action === "tempo") tempo.openTempo(returnFocusTarget);
    else if (action === "cpu-monitor") destination.open("cpu-monitor", returnFocusTarget);
    else notice(corOsUnavailableContextActionMessage(action));
  };
}
