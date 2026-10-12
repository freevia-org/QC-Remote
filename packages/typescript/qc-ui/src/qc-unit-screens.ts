import type { GatewayTransport } from "@qc-remote/client";
import { QC_DEVICE_SCREEN_GESTURES } from "./device-screen-navigation.ts";
import { classifyQcGraphicsTree, type QcObservedScreenKind } from "./use-adaptive-screen-polling.ts";

/**
 * The one description of how the app and the Quad Cortex's own screen follow
 * each other: what the unit shows (read from its graphics tree), the app
 * destination that mirrors it, the gesture or command that opens and closes it
 * on the unit, and how well each is known. The routes below are the only code
 * that sends a navigation gesture to the unit.
 */

export type QcPopupName = "grid-menu" | "scene-menu" | "mode-menu";

/** How the app shows each screen the unit can show. */
export interface QcUnitScreenEntry {
  /** The app destination that mirrors it ("none": the app has no equivalent and mirrors the unit's picture). */
  app: string;
  /** How the app opens it on the unit. */
  open: "command" | "gesture" | "probe-needed" | "never";
  /** How the app closes it on the unit. */
  close: "command" | "gesture" | "probe-needed" | "never";
  /** What is known about the open and close, and where. */
  evidence: string;
}

/** The table behind the routes: every screen class the app recognises. Tests keep it complete. */
export const QC_UNIT_SCREENS: Readonly<Record<Exclude<QcObservedScreenKind, "unknown">, QcUnitScreenEntry>> = {
  grid: { app: "Grid (no screen)", open: "gesture", close: "never", evidence: "reached by closing the screen on top; observed zenUI::Grid" },
  gig: { app: "gig", open: "command", close: "command", evidence: "device.showGigView, one frame; the unit also pushes type 24 (state gigView). Observed on 4.1.0" },
  tuner: { app: "tuner", open: "command", close: "command", evidence: "device.showTuner (type 27 plus meter); the unit may mute the rig. Mirrored both ways by owner decision" },
  io: { app: "io-overview, global-eq", open: "gesture", close: "gesture", evidence: "one drag > ~245 px down from the Grid, one tap (743,30). Observed 2026-10-06/11" },
  "preset-menu": { app: "Grid menu (popup)", open: "gesture", close: "gesture", evidence: "tap the kebab (764,22) toggles it, 10 of 10 on 4.1.0" },
  settings: { app: "device settings", open: "gesture", close: "gesture", evidence: "kebab, then the Settings row at (600,406) on the bottom-scrolled menu (audited, not re-run); close tap (743,30) observed. Update panes are display only" },
  modes: { app: "modes", open: "gesture", close: "gesture", evidence: "long press (700,70) observed; close tap (743,30) observed" },
  "midi-out": { app: "midi-out", open: "probe-needed", close: "gesture", evidence: "the menu row is unproven (seventh row, needs a scroll); close tap (743,30) is the dialog's check button" },
  tempo: { app: "tempo", open: "gesture", close: "gesture", evidence: "kebab, then the Tempo row (610,286) of the bottom-scrolled menu; the check button (743,30) closes it. Measured on 4.1.0; tree reads work while it is open" },
  directory: { app: "preset directory", open: "gesture", close: "gesture", evidence: "tap the preset title area (320,50) on the Grid; the header check (743,30) closes it. Measured on 4.1.0. The directory's rows, banks and cloud button are never touched" },
  "cpu-monitor": { app: "cpu monitor over the Grid", open: "gesture", close: "gesture", evidence: "kebab, then the CPU Monitor row (578,346); an overlay beside the Grid (zenUI::CpuLoadIndicator), closed only by its own X at (628,32), never the check button. Measured on 4.1.0" },
  looper: { app: "looper-editor", open: "command", close: "command", evidence: "looper open/close commands; opened by a Looper X block" },
  "parameter-editor": { app: "block editor (unit-mirror picture when the unit opened it)", open: "gesture", close: "gesture", evidence: "app to unit: a Grid block tap, and the check tap (742,30) only on zenUI::ParameterEditor; the splitter, mixer and lane editors (SMParamsEditor, Lane*ControlEditor) are neither opened nor closed by the app. Unit to app: the tree carries no block identity the app has read, so a unit-opened editor shows as the unit's picture, which the Grid closes" },
  "scene-selector": { app: "scene menu (popup)", open: "gesture", close: "gesture", evidence: "tap the scene indicator (668,23); an overlay beside the Grid (zenUI::SceneSelectorPopup), closed by tapping the same indicator again. Measured on 4.1.0; no scene row is ever touched" },
  "route-selector": { app: "unit mirror", open: "probe-needed", close: "probe-needed", evidence: "tap a rail label; not recorded" },
  "copy-scene": { app: "unit mirror", open: "probe-needed", close: "probe-needed", evidence: "menu row; not recorded" },
  "device-browser": { app: "unit mirror", open: "probe-needed", close: "probe-needed", evidence: "tap an empty slot; not recorded" },
  search: { app: "unit mirror", open: "probe-needed", close: "probe-needed", evidence: "not recorded" },
  keyboard: { app: "unit mirror", open: "probe-needed", close: "probe-needed", evidence: "not recorded" },
  "message-dialog": { app: "unit mirror (read-only)", open: "never", close: "never", evidence: "Delete, Overwrite, Reset and Restart confirmations: shown, never confirmed or dismissed by the app; only an explicit user tap on the mirror is sent" },
  "neural-capture": { app: "unit mirror", open: "probe-needed", close: "probe-needed", evidence: "not recorded" },
  "block-menu": { app: "unit mirror", open: "probe-needed", close: "probe-needed", evidence: "not recorded" },
  "expression-assign": { app: "unit mirror", open: "probe-needed", close: "probe-needed", evidence: "not recorded" },
  busy: { app: "unit mirror", open: "never", close: "never", evidence: "a progress screen; shown only" }
};

/** The unit screens the app mirrors as a picture, because it has no screen of its own for them. */
export const QC_UNIT_MIRROR_KINDS: readonly QcObservedScreenKind[] = ["message-dialog", "keyboard", "search", "copy-scene", "route-selector", "device-browser", "neural-capture", "block-menu", "expression-assign", "busy"];

/** A screen whose tap could start an update: the app refuses to send any tap while the unit shows one. */
export const QC_UPDATE_SCREEN = /download update|install update|install now|update available|software update/i;

/**
 * Why a touch the USER makes on the app's picture of the unit must not be forwarded now, or undefined when it may be.
 * The unit's Settings dialog opens on its Updates pane (DOWNLOAD UPDATE, SEE WHAT'S NEW) beside rows such as Power
 * Functions and Factory Reset, none of which the app ever touches: Settings is shown natively in the app, and the
 * only touches the app itself sends there are the menu row that opens it and the header check button that closes it.
 */
export function untouchableUnitScreen(tree: string): string | undefined {
  if (QC_UPDATE_SCREEN.test(tree) || /zenUI::UpdaterSettings(?![A-Za-z0-9_])/.test(tree)) return "an update screen";
  if (classifyQcGraphicsTree(tree) === "settings") return "Settings";
  return undefined;
}

// ---------------------------------------------------------------------------------------------------- the app side

/** The screen the app is showing, in the unit's terms. */
export function appScreenKind(app: { view: string | null; deviceSettingsOpen: boolean; tempoOpen: boolean; popup: QcPopupName | null; directoryOpen?: boolean }): QcObservedScreenKind {
  if (app.tempoOpen) return "tempo";
  if (app.deviceSettingsOpen) return "settings";
  if (app.directoryOpen) return "directory";
  const view = app.view;
  if (view === "tuner") return "tuner";
  if (view?.startsWith("gig")) return "gig";
  if (view?.startsWith("io-") || view === "global-eq") return "io";
  if (view === "modes") return "modes";
  if (view === "midi-out") return "midi-out";
  if (view === "cpu-monitor") return "cpu-monitor";
  if (view === "looper-editor") return "looper";
  if (view === "unit-mirror") return "message-dialog";
  if (app.popup === "grid-menu") return "preset-menu";
  if (app.popup === "scene-menu") return "scene-selector";
  return "grid";
}

/**
 * The screen the app will show once it has followed the unit to `kind`, in the
 * unit's terms: the pictures of screens the app has no screen for count as one
 * app screen.
 */
export function appKindAfterFollowing(kind: QcObservedScreenKind): QcObservedScreenKind {
  if (QC_UNIT_MIRROR_KINDS.includes(kind) || kind === "parameter-editor") return "message-dialog";
  return kind;
}

// -------------------------------------------------------------------------------------------------- the loop guard

/**
 * Stops a screen opened by one side from being sent back by the other. When
 * the app asks the unit to open a screen it expects the unit to show it soon,
 * so following that observation must not touch the app again; when the unit's
 * screen moves the app, the app's change must not be driven back to the unit.
 * An expectation applies once and lapses after ttlMs, so a lost frame can
 * never silence a later, genuine change.
 */
export interface QcScreenLatch {
  expectUnit(kind: QcObservedScreenKind): void;
  takeUnit(kind: QcObservedScreenKind): boolean;
  expectApp(kind: QcObservedScreenKind): void;
  takeApp(kind: QcObservedScreenKind): boolean;
  /** Withdraw an expectation whose gesture was never sent or did not take, so it cannot swallow a later genuine change. */
  cancelUnit(kind: QcObservedScreenKind): void;
}

export function createQcScreenLatch(now: () => number = Date.now, ttlMs = 10_000): QcScreenLatch {
  const unit = new Map<QcObservedScreenKind, number>();
  const app = new Map<QcObservedScreenKind, number>();
  const take = (book: Map<QcObservedScreenKind, number>, kind: QcObservedScreenKind) => {
    const expiresAt = book.get(kind);
    book.delete(kind);
    return expiresAt !== undefined && expiresAt >= now();
  };
  return {
    expectUnit: (kind) => { unit.set(kind, now() + ttlMs); },
    takeUnit: (kind) => take(unit, kind),
    expectApp: (kind) => { app.set(kind, now() + ttlMs); },
    takeApp: (kind) => take(app, kind),
    cancelUnit: (kind) => { unit.delete(kind); }
  };
}

// ----------------------------------------------------------------------------------------------- the unit's screen

export interface QcScreenPoll { intervalMs: number; timeoutMs: number }
export const DEFAULT_SCREEN_POLL: QcScreenPoll = { intervalMs: 250, timeoutMs: 2_500 };

type RouteGateway = Pick<GatewayTransport, "graphicsTree" | "tapScreen" | "swipeScreen" | "longTapScreen">;

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** What the unit is showing, read from its graphics tree. */
export async function unitScreen(gateway: Pick<GatewayTransport, "graphicsTree">): Promise<QcObservedScreenKind> {
  return classifyQcGraphicsTree((await gateway.graphicsTree()).tree);
}

/** Wait (bounded) until the unit shows a screen satisfying the predicate. */
export async function untilUnitShows(gateway: Pick<GatewayTransport, "graphicsTree">, poll: QcScreenPoll, wanted: (kind: QcObservedScreenKind) => boolean): Promise<boolean> {
  for (let waited = 0; waited <= poll.timeoutMs; waited += poll.intervalMs) {
    if (wanted(await unitScreen(gateway))) return true;
    await sleep(poll.intervalMs);
  }
  return false;
}

export type QcOpenOnUnit = "shown" | "busy" | "not-shown" | "no-route";
export type QcOpenTarget = "io" | "preset-menu" | "settings" | "modes" | "tempo" | "cpu-monitor" | "directory" | "scene-selector";
export type QcCloseTarget = QcOpenTarget;

const { openIo, done, menuButton, modesHold, settingsRow, tempoRow, cpuRow, cpuClose, directoryOpen, sceneIndicator } = QC_DEVICE_SCREEN_GESTURES;

/** The preset-menu rows that open a screen, and the screen each opens. */
const MENU_ROWS = { settings: settingsRow, tempo: tempoRow, "cpu-monitor": cpuRow } as const;

/**
 * Open a screen on the unit with its proven gesture. Every gesture is sent only
 * after reading the unit's screen, and only from the screen it was proven on
 * (the Grid; the menu for the Settings row), because the same touch on another
 * screen could press something. "busy" means another screen is open.
 */
export async function openOnUnit(gateway: RouteGateway, poll: QcScreenPoll, target: QcOpenTarget, beforeGesture?: () => void): Promise<QcOpenOnUnit> {
  const current = await unitScreen(gateway);
  if (current === target) return "shown";
  // Called once, just before the first touch is sent and never when none is: the caller arms its loop guard here.
  let announced = false;
  const announce = () => { if (!announced) { announced = true; beforeGesture?.(); } };
  if (target === "io") {
    if (current !== "grid") return "busy";
    announce();
    await gateway.swipeScreen(openIo.x, openIo.y, openIo.toX, openIo.toY);
    return await untilUnitShows(gateway, poll, (kind) => kind === "io") ? "shown" : "not-shown";
  }
  if (target === "preset-menu") {
    if (current !== "grid") return "busy";
    announce();
    await gateway.tapScreen(menuButton.x, menuButton.y);
    return await untilUnitShows(gateway, poll, (kind) => kind === "preset-menu") ? "shown" : "not-shown";
  }
  if (target === "directory" || target === "scene-selector") {
    if (current !== "grid") return "busy";
    announce();
    const point = target === "directory" ? directoryOpen : sceneIndicator;
    await gateway.tapScreen(point.x, point.y);
    return await untilUnitShows(gateway, poll, (kind) => kind === target) ? "shown" : "not-shown";
  }
  if (target === "modes") {
    if (current !== "grid") return "busy";
    announce();
    await gateway.longTapScreen(modesHold.x, modesHold.y);
    return await untilUnitShows(gateway, poll, (kind) => kind === "modes") ? "shown" : "not-shown";
  }
  // settings, tempo, cpu monitor: through the preset menu
  const row = MENU_ROWS[target as keyof typeof MENU_ROWS];
  if (current === "grid") {
    announce();
    await gateway.tapScreen(menuButton.x, menuButton.y);
    if (!await untilUnitShows(gateway, poll, (kind) => kind === "preset-menu")) return "not-shown";
  } else if (current !== "preset-menu") {
    return "busy";
  }
  announce();
  await gateway.tapScreen(row.x, row.y);
  if (await untilUnitShows(gateway, poll, (kind) => kind === target)) return "shown";
  // The row was not where it was expected: leave the menu as it was found.
  if (await unitScreen(gateway) === "preset-menu") await gateway.tapScreen(menuButton.x, menuButton.y);
  return "not-shown";
}

/**
 * Close a screen on the unit: the kebab toggles its menu, a tap on the check
 * button closes a dialog. Only while the unit shows that screen; a dialog
 * nested inside it (Global EQ inside I/O, a Settings pane) may need a second
 * tap, after which a still-open dialog is reported.
 */
export async function closeOnUnit(gateway: RouteGateway, poll: QcScreenPoll, target: QcCloseTarget, beforeGesture?: () => void): Promise<boolean> {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    if (await unitScreen(gateway) !== target) return true;
    if (attempt === 0) beforeGesture?.();
    if (target === "preset-menu") await gateway.tapScreen(menuButton.x, menuButton.y);
    else if (target === "cpu-monitor") await gateway.tapScreen(cpuClose.x, cpuClose.y);
    else if (target === "scene-selector") await gateway.tapScreen(sceneIndicator.x, sceneIndicator.y);
    else await gateway.tapScreen(done.x, done.y);
    if (await untilUnitShows(gateway, poll, (kind) => kind !== target)) return true;
    if (target === "preset-menu" || target === "scene-selector") return false;
  }
  return false;
}
