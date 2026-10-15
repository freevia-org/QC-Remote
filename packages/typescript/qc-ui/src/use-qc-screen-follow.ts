import { useCallback, useEffect, useRef, useState } from "react";
import { SupersededPresetOperation, type PresetLifetime, type QcStateUpdate } from "@qc-remote/core";
import { useDeviceWorkflowSession } from "./use-device-workflow-session.ts";
import {
  readQcScreenFollowSetting, useAdaptiveQcScreenPolling, writeQcScreenFollowSetting,
  type QcObservedScreenKind, type QcScreenObservation, type QcScreenPollingPhase
} from "./use-adaptive-screen-polling.ts";
import { appKindAfterFollowing, appScreenKind, closeOnUnit, DEFAULT_SCREEN_POLL, openOnUnit, QC_UNIT_MIRROR_KINDS, treeShows, type QcCloseTarget, type QcOpenTarget, type QcScreenPoll } from "./qc-unit-screens.ts";
import { corOsVersionFromTree } from "./cor-os-version.ts";
import { decodeUnitFrame, sceneCommandVerbFromFrame, sceneIndexFromTree, type UnitFrameDecoder } from "./scene-command-reader.ts";
import type { QcCpuTelemetry } from "./use-qc-cpu-telemetry.ts";
import type { QcDeviceUndo } from "./use-qc-device-undo.ts";
import type { QcScreenDestination } from "./use-qc-screen-destination.ts";
import type { GatewayTransport } from "@qc-remote/client";
import type { IoUnitSelection } from "./io-unit-selection.ts";
import { settingsSelectionKey, type QcSettingsUnitSelection } from "./settings-unit-selection.ts";

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
  /** `noteCorOsVersionFromScreen` keeps the CorOS version the unit's Device Information screen shows (see cor-os-version.ts). */
  deviceSettings: { open: boolean; close(): void; openSettings(): unknown; noteCorOsVersionFromScreen?(version: string): void;
    /** Selects the section and row the unit's Settings dialog shows (see settings-unit-selection.ts). Never sends anything to the unit. */
    followUnitSelection?(selection: QcSettingsUnitSelection): void };
  tempo: { open: boolean; close(): void; openTempo(): unknown };
  tuner: { close(): void; openTuner(): unknown };
  ioEq: { refresh(): unknown; followUnitSelection?(selection: IoUnitSelection | undefined): void };
  modes: { refresh(): unknown };
  /** The app's route picker, if any is open. */
  routing?: { picker?: { row: number; side: string } };
  /** The Copy/Swap Scene command the app has open, if any, and how the unit's own dialog opens and closes it. */
  scene?: { command?: "copy" | "swap" | null; openUnitCommand?(swap: boolean, scene: number): void; closeCommand?(): void };
}

export interface QcScreenFollowOptions {
  lifetime?: PresetLifetime;
  sessionKey?: unknown;
  captureSessionCurrent?(): () => boolean;
  /** `captureScreen` is read-only and only used to tell the unit's Copy Scene dialog from its Swap Scene dialog. */
  gateway: Pick<GatewayTransport, "graphicsTree" | "tapScreen" | "swipeScreen" | "longTapScreen"> & Partial<Pick<GatewayTransport, "captureScreen">>;
  /** Decodes a captured frame (the browser's decoder by default). */
  decodeFrame?: UnitFrameDecoder;
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

/** The screen the app shows, in the unit's terms. */
function appKindOf(o: Pick<QcScreenFollowOptions, "destination" | "workflows">): QcObservedScreenKind {
  const { destination, workflows } = o;
  return appScreenKind({
    view: destination.view, deviceSettingsOpen: workflows.deviceSettings.open, tempoOpen: workflows.tempo.open, popup: destination.popup,
    directoryOpen: workflows.preset.directoryOpen, directorySearchOpen: destination.directorySearchOpen, routePicker: workflows.routing?.picker,
    sceneCommand: Boolean(workflows.scene?.command), blockMenuOpen: destination.blockMenuOpen
  });
}

const ROUTED: readonly QcObservedScreenKind[] = ["preset-menu", "settings", "modes", "tempo", "cpu-monitor", "directory", "scene-selector", "route-selector", "midi-out", "copy-scene", "search", "block-menu"];

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
export function useQcScreenFollow(options: QcScreenFollowOptions): { phase: QcScreenPollingPhase; observe(observation: QcScreenObservation): void; observeTree(tree: string): void } {
  const { follow, available, ready } = options;
  const lifetime = useDeviceWorkflowSession({ ...options, connected: ready && available,
    fail: (error: unknown) => options.notice(error instanceof Error ? error.message : "The Quad Cortex screen could not be followed.") });
  const gateway = lifetime.options.gateway;
  const followed = useRef<QcObservedScreenKind | undefined>(undefined);
  /** The Settings pane the unit last named and the app was moved to; empty while the unit shows no known Settings pane. */
  const followedSettingsPane = useRef("");
  const observationGeneration = useRef(0);
  useEffect(() => {
    if (!follow || !ready) { followed.current = undefined; followedSettingsPane.current = ""; observationGeneration.current += 1; }
  }, [follow, ready]);
  const latest = useRef({ ...lifetime.options, lifetime });
  latest.current = { ...lifetime.options, lifetime };
  /** Routes the app is driving on the unit right now: what the unit shows on the way is the app's doing, not the unit's. */
  const driving = useRef({ key: lifetime.key, count: 0 });
  if (driving.current.key !== lifetime.key) {
    driving.current = { key: lifetime.key, count: 0 };
    followed.current = undefined;
    followedSettingsPane.current = "";
    observationGeneration.current += 1;
  }

  /** Closes every app screen the unit's new screen replaces. */
  const leaveAppScreens = (closeEditor: boolean) => {
    const { destination, workflows } = latest.current;
    if (closeEditor) workflows.grid.close();
    workflows.scene?.closeCommand?.();
    workflows.preset.closeDirectory();
    workflows.deviceSettings.close();
    workflows.tempo.close();
    workflows.tuner.close();
    destination.setView(null);
    destination.setPopup(null);
  };

  /**
   * The unit shows its Copy/Swap Scene dialog and the app did not open it: draw the native panel with the unit's own
   * title. Copy or Swap is read from a captured frame and the source scene from the Grid's indicator; if either cannot
   * be read (no capture, a frame mid-animation), the unit's picture is shown instead, never a guessed title.
   */
  const followSceneCommand = async () => {
    const owner = latest.current.lifetime;
    const generation = observationGeneration.current;
    const { destination, workflows, gateway: unit, screenPoll } = latest.current;
    const poll = screenPoll ?? DEFAULT_SCREEN_POLL;
    const stale = () => !owner.current() || generation !== observationGeneration.current || followed.current !== "copy-scene";
    let verb: Awaited<ReturnType<typeof sceneCommandVerbFromFrame>>;
    let source: number | undefined;
    try {
      for (let attempt = 0; attempt < 4 && !verb && !stale(); attempt += 1) {
        if (attempt > 0) await new Promise((resolve) => setTimeout(resolve, poll.intervalMs));
        const tree = (await unit.graphicsTree()).tree;
        if (stale() || !treeShows(tree, "copy-scene")) return;
        source = sceneIndexFromTree(tree);
        verb = unit.captureScreen ? await sceneCommandVerbFromFrame(await unit.captureScreen(), latest.current.decodeFrame ?? decodeUnitFrame) : undefined;
        if (!unit.captureScreen) break;
      }
    } catch { /* the picture below is the answer when the unit cannot be read */ }
    if (stale()) return;
    // The app already shows the panel (its own Copy/Swap Scene command whose expectation lapsed): leave it as it is.
    if (appKindOf(latest.current) === "copy-scene") return;
    const native = verb !== undefined && source !== undefined && workflows.scene?.openUnitCommand !== undefined;
    const shown = appKindOf(latest.current);
    const will: QcObservedScreenKind = native ? "copy-scene" : "message-dialog";
    if (will !== shown) destination.latch.expectApp(will);
    leaveAppScreens(true);
    if (native) workflows.scene!.openUnitCommand!(verb === "swap", source!);
    else destination.setView("unit-mirror");
  };

  const observe = useCallback((observation: QcScreenObservation) => {
    if (!lifetime.current()) return;
    if (!latest.current.lifetime.current()) return;
    const kind = observation.kind;
    // The unit's I/O selection is the unit's to name on every open and while open (the app's own taps are mirrored to the
    // unit and come back as the same selection), so it is applied before the once-per-kind latch below; leaving I/O clears it.
    latest.current.workflows.ioEq.followUnitSelection?.(kind === "io" ? observation.ioSelection : undefined);
    // The unit's Settings section and row are the unit's to name when it changes pane (and on every open, since it opens on
    // the last section visited). They are applied once per change, not on every observation, so a section the user chose in
    // the app stays until the unit moves to another pane; the app's own choice is never sent to the unit.
    const settingsPane = kind === "settings" ? settingsSelectionKey(observation.settingsSelection) : "";
    if (settingsPane !== followedSettingsPane.current) {
      followedSettingsPane.current = settingsPane;
      if (observation.settingsSelection) latest.current.workflows.deviceSettings.followUnitSelection?.(observation.settingsSelection);
    }
    if (followed.current === kind) return;
    followed.current = kind;
    observationGeneration.current += 1;
    const { destination, workflows } = latest.current;
    if (kind === "unknown") return;
    // A block editor the app opened (its own block tap opened the unit's) is already on screen. One opened on the unit
    // carries no block identity the app can read, so the app shows the unit's own picture of it instead.
    if (kind === "parameter-editor" && workflows.grid.details) return;
    // The app asked the unit for this screen, so the app already shows it; the screens the unit passes through
    // while the app drives it there (the menu on the way to Settings) must not pull the app back.
    if (driving.current.count > 0) { destination.latch.takeUnit(kind); return; }
    if (kind !== "parameter-editor" && destination.latch.takeUnit(kind)) return;
    // The unit is moving the app: the app's change must not be driven back to the unit. Only a change that will
    // happen is expected; one the app already shows would otherwise swallow the user's next real change.
    const shown = appKindOf(latest.current);
    // The Copy/Swap Scene dialog has no text in the unit's tree: read which one it is, then draw the panel (below).
    if (kind === "copy-scene") { void followSceneCommand(); return; }
    const will = appKindAfterFollowing(kind);
    if (will !== shown) destination.latch.expectApp(will);
    leaveAppScreens(kind !== "parameter-editor");
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
  }, [lifetime.key]);

  /**
   * Every tree the poll reads. While the unit shows Settings > Support > Device Information the tree states the CorOS
   * version; the app keeps it (only that one value, only as a dotted version number) so Device Information shows it, and
   * keeps showing it after the screen closes. Reading is all this does: nothing is sent to the unit.
   */
  const observeTree = useCallback((tree: string) => {
    if (!lifetime.current() || !latest.current.lifetime.current()) return;
    const version = corOsVersionFromTree(tree);
    if (version !== undefined) latest.current.workflows.deviceSettings.noteCorOsVersionFromScreen?.(version);
  }, [lifetime.key]);

  const { phase } = useAdaptiveQcScreenPolling({ gateway, lifetime: options.lifetime, enabled: follow && available, connected: ready, onObservation: observe, onTree: observeTree });

  // App to unit: the screen the app shows, in the unit's terms, changing by the user's hand.
  const appKind = appKindOf(options);
  /** Which rail label the first row's picker was opened from, kept for its close. */
  const routeSide = useRef<"input" | "output">("input");
  if (appKind === "route-selector") routeSide.current = options.workflows.routing?.picker?.side === "output" ? "output" : "input";
  const previousAppKind = useRef<QcObservedScreenKind | undefined>(undefined);
  /**
   * The route the app is driving on the unit, from the screen it left to the one it shows, until it has an answer. A new
   * lifetime (the native state stream restarting, the session becoming ready again) retires the route that was running,
   * and the effect below then re-runs with the app's screen unchanged, which used to forget the route: the user's CANCEL
   * closed the panel and nothing reached the unit, whose dialog was then drawn again. The new lifetime resumes the route
   * instead. A close reads the unit's screen before its touch, so resuming one the unit has already finished (or the user
   * finished on the unit) sends nothing.
   */
  const owedRoute = useRef<{ physical?: ReturnType<PresetLifetime["checkpoint"]>; gateway: QcScreenFollowOptions["gateway"]; from: QcObservedScreenKind; to: QcObservedScreenKind; detail: { swap: boolean; side: "input" | "output" } } | undefined>(undefined);
  useEffect(() => {
    const previousKind = previousAppKind.current;
    previousAppKind.current = appKind;
    const changed = previousKind !== undefined && previousKind !== appKind;
    const owed = owedRoute.current;
    // A stream restart may resume a read-before-close on the SAME physical
    // session only. Never replay navigation against a replacement device.
    if (changed || (owed && (!owed.physical || !options.lifetime?.currentSession(owed.physical) || owed.gateway !== options.gateway))) owedRoute.current = undefined;
    const resumed = !changed && owedRoute.current?.to === appKind ? owedRoute.current : undefined;
    if (!changed && !resumed) return;
    const previous = resumed ? resumed.from : previousKind!;
    const current = latest.current;
    if (!current.ready || !current.available) return;
    // A change the unit made is not sent back (a resumed route was never one).
    if (!resumed && current.destination.latch.takeApp(appKind)) return;
    const { destination, notice, lifetime: owner } = current;
    let live = true;
    // Cleanup retires this navigation route too, so an old screen read cannot
    // dispatch the next tap after either the destination or device changes.
    const assertCurrent = () => { if (!live || !owner.current()) throw new SupersededPresetOperation(); };
    const unit = new Proxy(current.gateway, {
      get(target, key) {
        const method = Reflect.get(target, key);
        if (typeof method !== "function") return method;
        return async (...args: unknown[]) => {
          assertCurrent();
          const result = await method.apply(target, args);
          assertCurrent();
          return result;
        };
      }
    });
    const poll = current.screenPoll ?? DEFAULT_SCREEN_POLL;
    const detail = resumed ? resumed.detail : { swap: current.workflows.scene?.command === "swap", side: routeSide.current };
    const run = async () => {
      if (previous === "search" && (appKind === "directory" || !ROUTED.includes(appKind))) {
        // Leaving the search dialog; leaving the Directory with it closes that too.
        let closed = await closeOnUnit(unit, poll, "search", () => { assertCurrent(); destination.latch.expectUnit("directory"); });
        if (closed && appKind !== "directory") closed = await closeOnUnit(unit, poll, "directory", () => { assertCurrent(); destination.latch.expectUnit("grid"); });
        assertCurrent();
        if (!closed) notice("The Quad Cortex did not close the same screen.");
      } else if (ROUTED.includes(appKind)) {
        // Armed just before the first touch, so a screen the unit never was asked for cannot be mistaken for one it was.
        const result = await openOnUnit(unit, poll, appKind as QcOpenTarget, () => { assertCurrent(); destination.latch.expectUnit(appKind); }, detail);
        assertCurrent();
        if (result === "busy") notice("Close the screen open on the Quad Cortex first; the app does not drive it from another screen.");
        else if (result === "mismatch") { destination.latch.cancelUnit(appKind); notice("The Quad Cortex did not show the screen the app opened, so the app stopped sending touches. Check the unit."); }
        else if (result === "not-shown") { destination.latch.cancelUnit(appKind); notice("The Quad Cortex did not show the same screen."); }
      } else if (ROUTED.includes(previous)) {
        if (!await closeOnUnit(unit, poll, previous as QcCloseTarget, () => { assertCurrent(); destination.latch.expectUnit("grid"); }, detail)) {
          assertCurrent();
          destination.latch.cancelUnit("grid");
          notice("The Quad Cortex did not close the same screen.");
        }
      }
    };
    const driven = driving.current;
    driven.count += 1;
    // Only a close is owed: it is the user's own dismissal of a screen the unit still shows, and it reads that screen before
    // its touch. An open never outlives its lifetime: it would send its next gesture into a successor session.
    if (!ROUTED.includes(appKind) && ROUTED.includes(previous) && previous !== "search") owedRoute.current = { physical: options.lifetime?.checkpoint(), gateway: options.gateway, from: previous, to: appKind, detail };
    void run().catch((error: unknown) => {
      if (live && owner.current() && !(error instanceof SupersededPresetOperation)) notice(error instanceof Error ? error.message : "The Quad Cortex screen could not be followed.");
    }).finally(() => {
      if (driving.current === driven) driven.count -= 1;
      // Answered, or failed for a reason of its own: the route is done. A retired one stays owed to the next lifetime.
      if (live && owner.current() && owedRoute.current?.from === previous && owedRoute.current.to === appKind) owedRoute.current = undefined;
    });
    return () => { live = false; };
  }, [appKind, lifetime.key]);

  return { phase, observe, observeTree };
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
