import { useCallback, useEffect, useRef, useState } from "react";
import type { QcStateUpdate } from "@qc-remote/core";
import {
  readQcScreenFollowSetting, useAdaptiveQcScreenPolling, writeQcScreenFollowSetting,
  type QcObservedScreenKind, type QcScreenObservation, type QcScreenPollingPhase
} from "./use-adaptive-screen-polling.ts";
import { appKindAfterFollowing, appScreenKind, closeOnUnit, DEFAULT_SCREEN_POLL, openOnUnit, QC_UNIT_MIRROR_KINDS, type QcCloseTarget, type QcOpenTarget, type QcScreenPoll } from "./qc-unit-screens.ts";
import type { QcCpuTelemetry } from "./use-qc-cpu-telemetry.ts";
import type { QcDeviceUndo } from "./use-qc-device-undo.ts";
import type { QcScreenDestination } from "./use-qc-screen-destination.ts";
import type { GatewayTransport } from "@qc-remote/client";

/**
 * The "Follow the Quad Cortex screen" setting, read and written in one place
 * for every host. On by default; only a stored "false" turns it off. It governs only the unit moving the app's
 * screen; the app moving the unit's screen (the user opening or closing a
 * screen in the app) is never affected by it.
 */
export function useQcFollowSetting(storage: Pick<Storage, "getItem" | "setItem">): { follow: boolean; setFollow(enabled: boolean): void } {
  const [follow, setFollowState] = useState(() => readQcScreenFollowSetting(storage));
  const setFollow = useCallback((enabled: boolean) => {
    setFollowState(enabled);
    writeQcScreenFollowSetting(storage, enabled);
  }, [storage]);
  return { follow, setFollow };
}

/** The workflows the unit's screens open and close in the app. */
export interface QcScreenFollowWorkflows {
  /** `details` is the block editor the app has open, if any. */
  grid: { close(): void; details?: unknown };
  preset: { closeDirectory(): void; openDirectory(): unknown; directoryOpen?: boolean };
  deviceSettings: { open: boolean; close(): void; openSettings(): unknown };
  tempo: { open: boolean; close(): void; openTempo(): unknown };
  tuner: { close(): void; openTuner(): unknown };
  ioEq: { refresh(): unknown };
  modes: { refresh(): unknown };
}

export interface QcScreenFollowOptions {
  gateway: Pick<GatewayTransport, "graphicsTree" | "tapScreen" | "swipeScreen" | "longTapScreen">;
  /** The user's "follow the Quad Cortex screen" setting: whether the unit's screens move the app's. */
  follow: boolean;
  /** Host gating that is not about the session: a demo device, a fixture run, a non-native build. */
  available: boolean;
  /** The device session can answer: connected, not suspended, not in a bulk transfer. */
  ready: boolean;
  destination: QcScreenDestination;
  workflows: QcScreenFollowWorkflows;
  notice(message: string): void;
  screenPoll?: QcScreenPoll;
}

const ROUTED: readonly QcObservedScreenKind[] = ["preset-menu", "settings", "modes", "tempo", "cpu-monitor", "directory", "scene-selector"];

/**
 * Keeps the app and the unit on the same screen, both ways, with one loop guard.
 *
 * Unit to app (only with the setting on): when the unit shows a screen, the
 * matching app screen opens; when it returns to the Grid, the local screens
 * close. Each kind is followed once until the unit shows a different one, and
 * the latch re-arms whenever following stops or the session stops answering.
 *
 * App to unit (always): when the user opens or closes the preset menu,
 * Settings or Modes in the app, the unit is driven to the same screen by its
 * proven gesture (qc-unit-screens.ts), after reading what it shows. The
 * latch keeps a screen one side opened from being sent back by the other.
 * Tuner, Gig View and I/O have their own commands in useQcDeviceViews.
 */
export function useQcScreenFollow(options: QcScreenFollowOptions): { phase: QcScreenPollingPhase; observe(observation: QcScreenObservation): void } {
  const { gateway, follow, available, ready } = options;
  const followed = useRef<QcObservedScreenKind | undefined>(undefined);
  useEffect(() => {
    if (!follow || !ready) followed.current = undefined;
  }, [follow, ready]);
  const latest = useRef(options);
  latest.current = options;
  /** Routes the app is driving on the unit right now: what the unit shows on the way is the app's doing, not the unit's. */
  const driving = useRef(0);

  const observe = useCallback((observation: QcScreenObservation) => {
    const kind = observation.kind;
    if (followed.current === kind) return;
    followed.current = kind;
    const { destination, workflows } = latest.current;
    if (kind === "unknown") return;
    // A block editor the app opened (its own block tap opened the unit's) is already on screen. One opened on the unit
    // carries no block identity the app can read, so the app shows the unit's own picture of it instead.
    if (kind === "parameter-editor" && workflows.grid.details) return;
    // The app asked the unit for this screen, so the app already shows it; the screens the unit passes through
    // while the app drives it there (the menu on the way to Settings) must not pull the app back.
    if (driving.current > 0) { destination.latch.takeUnit(kind); return; }
    if (kind !== "parameter-editor" && destination.latch.takeUnit(kind)) return;
    // The unit is moving the app: the app's change must not be driven back to the unit. Only a change that will
    // happen is expected; one the app already shows would otherwise swallow the user's next real change.
    const { view, popup } = destination;
    const shown = appScreenKind({ view, deviceSettingsOpen: workflows.deviceSettings.open, tempoOpen: workflows.tempo.open, popup, directoryOpen: workflows.preset.directoryOpen });
    const will = appKindAfterFollowing(kind);
    if (will !== shown) destination.latch.expectApp(will);
    if (kind !== "parameter-editor") workflows.grid.close();
    workflows.preset.closeDirectory();
    workflows.deviceSettings.close();
    workflows.tempo.close();
    workflows.tuner.close();
    destination.setView(null);
    destination.setPopup(null);
    const { setView, setPopup } = destination;
    if (kind === "tuner") { setView("tuner"); void workflows.tuner.openTuner(); }
    else if (kind === "tempo") void workflows.tempo.openTempo();
    else if (kind === "gig") setView("gig");
    else if (kind === "settings") void workflows.deviceSettings.openSettings();
    else if (kind === "io") { setView("io-overview"); void workflows.ioEq.refresh(); }
    else if (kind === "directory") void workflows.preset.openDirectory();
    else if (kind === "cpu-monitor") setView("cpu-monitor");
    else if (kind === "looper") setView("looper-editor");
    else if (kind === "modes") { setView("modes"); void workflows.modes.refresh(); }
    else if (kind === "midi-out") setView("midi-out");
    else if (kind === "preset-menu") setPopup("grid-menu");
    else if (kind === "scene-selector") setPopup("scene-menu");
    else if (kind === "parameter-editor" || QC_UNIT_MIRROR_KINDS.includes(kind)) setView("unit-mirror");
  }, []);

  const { phase } = useAdaptiveQcScreenPolling({ gateway, enabled: follow && available, connected: ready, onObservation: observe });

  // App to unit: the screen the app shows, in the unit's terms, changing by the user's hand.
  const appKind = appScreenKind({ view: options.destination.view, deviceSettingsOpen: options.workflows.deviceSettings.open, tempoOpen: options.workflows.tempo.open, popup: options.destination.popup, directoryOpen: options.workflows.preset.directoryOpen });
  const previousAppKind = useRef<QcObservedScreenKind | undefined>(undefined);
  useEffect(() => {
    const previous = previousAppKind.current;
    previousAppKind.current = appKind;
    if (previous === undefined || previous === appKind) return;
    const current = latest.current;
    if (!current.ready || !current.available) return;
    // A change the unit made is not sent back.
    if (current.destination.latch.takeApp(appKind)) return;
    const { gateway: unit, destination, notice } = current;
    const poll = current.screenPoll ?? DEFAULT_SCREEN_POLL;
    const run = async () => {
      if (ROUTED.includes(appKind)) {
        // Armed just before the first touch, so a screen the unit never was asked for cannot be mistaken for one it was.
        const result = await openOnUnit(unit, poll, appKind as QcOpenTarget, () => destination.latch.expectUnit(appKind));
        if (result === "busy") notice("Close the screen open on the Quad Cortex first; the app does not drive it from another screen.");
        else if (result === "not-shown") { destination.latch.cancelUnit(appKind); notice("The Quad Cortex did not show the same screen."); }
      } else if (ROUTED.includes(previous)) {
        if (!await closeOnUnit(unit, poll, previous as QcCloseTarget, () => destination.latch.expectUnit("grid"))) {
          destination.latch.cancelUnit("grid");
          notice("The Quad Cortex did not close the same screen.");
        }
      }
    };
    driving.current += 1;
    void run().catch((error: unknown) => notice(error instanceof Error ? error.message : "The Quad Cortex screen could not be followed."))
      .finally(() => { driving.current -= 1; });
  }, [appKind]);

  return { phase, observe };
}

/** Where Gig View's on-unit state takes the app's screen: shown opens Gig, hidden closes a Gig screen, anything else is left alone. */
export function qcGigViewTransition(current: QcScreenDestination["view"], shown: boolean): QcScreenDestination["view"] {
  return shown ? "gig" : current?.startsWith("gig") ? null : current;
}

/**
 * The part of every native state batch that decides what the display shows:
 * the CPU readings and whether the unit has something to undo or redo always,
 * and Gig View shown or hidden on the unit only when the user follows the
 * unit's screen (the same setting that gates every other screen). The hosts
 * call this first and keep only their transport-specific handling.
 */
export function consumeQcDisplayStates(states: readonly QcStateUpdate[], display: { setView: QcScreenDestination["setView"]; cpu: Pick<QcCpuTelemetry, "consume">; undo: Pick<QcDeviceUndo, "consume">; follow: boolean }): void {
  display.cpu.consume(states);
  display.undo.consume(states);
  if (!display.follow) return;
  for (const state of states) {
    if (state.kind === "gigView" && state.gigViewShown !== undefined) {
      const shown = state.gigViewShown;
      display.setView((current) => qcGigViewTransition(current, shown));
    }
  }
}
