import { useCallback, useState } from "react";
import { corOsUnavailableContextActionMessage, type CorOsContextAction } from "./coros-ui.ts";
import type { CorOsScreenView } from "./coros-screen-fixture-data.ts";

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
}

/**
 * The one owner of "which device screen is showing" for both hosts. Explicit
 * `setView` calls forget the return-focus target; only `open` carries one.
 */
export function useQcScreenDestination(): QcScreenDestination {
  const [state, setState] = useState<{ view: CorOsScreenView | null; returnFocusTarget?: QcScreenReturnFocus }>({ view: null });
  const setView = useCallback((next: QcScreenViewUpdate) => {
    setState((current) => {
      const view = typeof next === "function" ? next(current.view) : next;
      // Preserve a functional no-op from an unrelated gig-state publication.
      if (typeof next === "function" && view === current.view) return current;
      return { view };
    });
  }, []);
  const open = useCallback((view: CorOsScreenView, returnFocusTarget?: QcScreenReturnFocus) => {
    setState({ view, returnFocusTarget });
  }, []);
  return { view: state.view, returnFocusTarget: state.returnFocusTarget, setView, open };
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
