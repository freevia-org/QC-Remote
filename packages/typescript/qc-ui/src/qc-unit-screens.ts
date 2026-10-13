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
  "midi-out": { app: "midi-out", open: "gesture", close: "gesture", evidence: "kebab, scroll the menu to its top (drag (600,150) to (600,420)), the Preset MIDI Out row (580,406); the check (743,30) closes it and the delete button beside it is never touched. Measured on 4.1.0" },
  tempo: { app: "tempo", open: "gesture", close: "gesture", evidence: "kebab, then the Tempo row (610,286) of the bottom-scrolled menu; the check button (743,30) closes it. Measured on 4.1.0; tree reads work while it is open" },
  directory: { app: "preset directory", open: "gesture", close: "gesture", evidence: "tap the preset title area (320,50) on the Grid; the header check (743,30) closes it. Measured on 4.1.0. The directory's rows, banks and cloud button are never touched" },
  "cpu-monitor": { app: "cpu monitor over the Grid", open: "gesture", close: "gesture", evidence: "kebab, then the CPU Monitor row (578,346); an overlay beside the Grid (zenUI::CpuLoadIndicator), closed only by its own X at (628,32), never the check button. Measured on 4.1.0" },
  looper: { app: "looper-editor", open: "command", close: "command", evidence: "looper open/close commands; opened by a Looper X block" },
  "parameter-editor": { app: "block editor (unit-mirror picture when the unit opened it)", open: "gesture", close: "gesture", evidence: "app to unit: a Grid block tap, and the check tap (742,30) only on zenUI::ParameterEditor; the splitter, mixer and lane editors (SMParamsEditor, Lane*ControlEditor) are neither opened nor closed by the app. Unit to app: the tree carries no block identity the app has read, so a unit-opened editor shows as the unit's picture, which the Grid closes" },
  "scene-selector": { app: "scene menu (popup)", open: "gesture", close: "gesture", evidence: "tap the scene indicator (668,23); an overlay beside the Grid (zenUI::SceneSelectorPopup), closed by tapping the same indicator again. Measured on 4.1.0; no scene row is ever touched" },
  "route-selector": { app: "unit mirror (the first row's input picker when the app opened it)", open: "gesture", close: "gesture", evidence: "tap the first row's input label (30,147) or output label (772,147), the same label again to close; a PortSelector list beside the Grid. Measured on 4.1.0. No row is ever touched: a pick rewrites the routing" },
  "copy-scene": { app: "unit mirror (the Copy/Swap Scene command when the app opened it)", open: "gesture", close: "gesture", evidence: "kebab, scroll to the top, the Copy Scene row (582,286) or Swap Scene row (582,346): one CopySceneDialog class for both, so the app remembers which; CANCEL (400,300) closes it and no footswitch is ever pressed. Measured on 4.1.0" },
  "device-browser": { app: "unit mirror", open: "never", close: "never", evidence: "a drawer over the Grid opened by tapping an empty cell, closed by tapping the dimmed area at its right (measured on 4.1.0, class name not read). Display only: a model row adds a block and dirties the preset, so the app routes nothing here" },
  search: { app: "unit mirror (the Directory search when the app opened it)", open: "gesture", close: "gesture", evidence: "from the Directory only: the search icon (634,30); the X at the top left (57,30) closes it back to the Directory (the check does nothing there). The keyboard, Done, recent searches and Clear all are never touched. Measured on 4.1.0" },
  keyboard: { app: "unit mirror", open: "probe-needed", close: "probe-needed", evidence: "not recorded" },
  "message-dialog": { app: "unit mirror (read-only)", open: "never", close: "never", evidence: "Delete, Overwrite, Reset and Restart confirmations: shown, never confirmed or dismissed by the app; only an explicit user tap on the mirror is sent" },
  "neural-capture": { app: "unit mirror", open: "probe-needed", close: "probe-needed", evidence: "not recorded" },
  "block-menu": { app: "unit mirror (the block editor menu when the app opened it)", open: "gesture", close: "gesture", evidence: "only on a block editor that is a bottom panel with the Grid above it: the kebab (30,226); an empty area outside the popup (450,215) closes it. A full-screen editor (Cab) has no measured menu button and is not routed. No menu row is ever touched. Measured on 4.1.0" },
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
  // Settings is view-only whatever else the tree holds (a busy indicator on the Updates pane, a keyboard, a confirmation).
  if (classifyQcGraphicsTree(tree) === "settings" || /zenUI::(SettingsDialog|DSPDiagnosticsDialog|ZencoderDiagnosticsDialog|SOC2ARMDiagnosticsDialog)(?![A-Za-z0-9_])/.test(tree)) return "Settings";
  return undefined;
}

// ---------------------------------------------------------------------------------------------------- the app side

/** The screen the app is showing, in the unit's terms. */
export function appScreenKind(app: { view: string | null; deviceSettingsOpen: boolean; tempoOpen: boolean; popup: QcPopupName | null; directoryOpen?: boolean; directorySearchOpen?: boolean; routePicker?: { row: number; side: string }; sceneCommand?: boolean; blockMenuOpen?: boolean }): QcObservedScreenKind {
  if (app.tempoOpen) return "tempo";
  if (app.deviceSettingsOpen) return "settings";
  if (app.directoryOpen) return app.directorySearchOpen ? "search" : "directory";
  if (app.sceneCommand) return "copy-scene";
  if (app.blockMenuOpen) return "block-menu";
  const view = app.view;
  if (view === "tuner") return "tuner";
  if (view?.startsWith("gig")) return "gig";
  if (view?.startsWith("io-") || view === "global-eq") return "io";
  if (view === "modes") return "modes";
  if (view === "midi-out") return "midi-out";
  if (view === "cpu-monitor") return "cpu-monitor";
  if (view === "looper-editor") return "looper";
  if (view === "unit-mirror") return "message-dialog";
  if (app.routePicker?.row === 0 && (app.routePicker.side === "input" || app.routePicker.side === "output")) return "route-selector";
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

export interface QcScreenPoll {
  intervalMs: number;
  timeoutMs: number;
  /** How long the preset menu is given to settle after each scroll drag (default 1500 ms). */
  scrollSettleMs?: number;
  /** The wait between the last drag and the row tap, which the unit ignores while the scroll animates (default 1000 ms). */
  rowDelayMs?: number;
  /** How long the expected dialog is waited for after the row tap (default 3000 ms). */
  rowTimeoutMs?: number;
}
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

/**
 * Whether the unit is showing `target`. The CPU Monitor is an overlay beside the Grid, so it is read by its own
 * class rather than by what the classifier ranks first, which a second class in the tree could change; a dialog on
 * top of it (a confirmation, a keyboard, Settings) still makes it not the screen to touch.
 */
export function treeShows(tree: string, target: QcObservedScreenKind): boolean {
  const kind = classifyQcGraphicsTree(tree);
  if (target !== "cpu-monitor") return kind === target;
  return /zenUI::CpuLoadIndicator(?![A-Za-z0-9_])/.test(tree) && !["message-dialog", "keyboard", "search", "settings", "busy"].includes(kind);
}

async function untilTree(gateway: Pick<GatewayTransport, "graphicsTree">, poll: QcScreenPoll, wanted: (tree: string) => boolean, timeoutMs = poll.timeoutMs): Promise<boolean> {
  for (let waited = 0; waited <= timeoutMs; waited += poll.intervalMs) {
    if (wanted((await gateway.graphicsTree()).tree)) return true;
    await sleep(poll.intervalMs);
  }
  return false;
}

export type QcOpenOnUnit = "shown" | "busy" | "not-shown" | "no-route" | "mismatch";
export type QcOpenTarget = "io" | "preset-menu" | "settings" | "modes" | "tempo" | "cpu-monitor" | "directory" | "scene-selector" | "route-selector" | "midi-out" | "copy-scene" | "search" | "block-menu";
export type QcCloseTarget = QcOpenTarget;

/** What the app knows and the unit's tree cannot say. */
export interface QcRouteDetail {
  /** Copy Scene and Swap Scene share one dialog class. */
  swap?: boolean;
  /** Which rail label opened the first row's route picker. */
  side?: "input" | "output";
}

const G = QC_DEVICE_SCREEN_GESTURES;
const { openIo, done, menuButton, modesHold } = G;

// The preset menu remembers its scroll, and the user's finger or another client can move it at any time. Rows at the
// bottom (Settings, Tempo, CPU Monitor) and rows at the top (Preset MIDI Out, Copy and Swap Scene) are never both
// visible, so every route scrolls to the extreme its row lives at immediately before tapping it and never relies on
// where it believes the menu was left.

const BOTTOM_ROWS = { settings: G.settingsRow, tempo: G.tempoRow, "cpu-monitor": G.cpuRow } as const;

/** Screens whose close is one toggle or one dismissal: a second touch would reopen or press something else. */
const CLOSES_ONCE: readonly QcCloseTarget[] = ["preset-menu", "scene-selector", "route-selector", "block-menu", "copy-scene", "search"];

/**
 * Open a screen on the unit with its measured gesture. Every gesture is sent only
 * after reading the unit's screen, and only from the screen it was measured on
 * (the Grid; the menu for its rows; the Directory for its search; a panel editor
 * for its menu), because the same touch on another screen could press
 * something. "busy" means another screen is open; "no-route" that this
 * particular screen has no measured way (a full-screen editor's menu).
 */
export async function openOnUnit(gateway: RouteGateway, poll: QcScreenPoll, target: QcOpenTarget, beforeGesture?: () => void, detail: QcRouteDetail = {}): Promise<QcOpenOnUnit> {
  const first = (await gateway.graphicsTree()).tree;
  const current = classifyQcGraphicsTree(first);
  if (current === target || treeShows(first, target)) return "shown";
  // Called once, just before the first touch is sent and never when none is: the caller arms its loop guard here.
  let announced = false;
  const announce = () => { if (!announced) { announced = true; beforeGesture?.(); } };
  const shows = (kind: QcObservedScreenKind) => untilUnitShows(gateway, poll, (seen) => seen === kind);
  const stillShows = async (kind: QcObservedScreenKind) => treeShows((await gateway.graphicsTree()).tree, kind);
  const tap = async (point: { x: number; y: number }) => { announce(); await gateway.tapScreen(point.x, point.y); };

  if (target === "io") {
    if (current !== "grid") return "busy";
    announce();
    await gateway.swipeScreen(openIo.x, openIo.y, openIo.toX, openIo.toY);
    return await shows("io") ? "shown" : "not-shown";
  }
  if (target === "modes") {
    if (current !== "grid") return "busy";
    announce();
    await gateway.longTapScreen(modesHold.x, modesHold.y);
    return await shows("modes") ? "shown" : "not-shown";
  }
  if (target === "preset-menu" || target === "directory" || target === "scene-selector" || target === "route-selector") {
    if (current !== "grid") return "busy";
    await tap(target === "preset-menu" ? menuButton : target === "directory" ? G.directoryOpen : target === "scene-selector" ? G.sceneIndicator : detail.side === "output" ? G.outputRail : G.inputRail);
    return await shows(target) ? "shown" : "not-shown";
  }
  if (target === "search") {
    if (current === "grid") {
      await tap(G.directoryOpen);
      if (!await shows("directory")) return "not-shown";
    } else if (current !== "directory") {
      return "busy";
    }
    if (!await stillShows("directory")) return "mismatch";
    await tap(G.directorySearch);
    return await shows("search") ? "shown" : "not-shown";
  }
  if (target === "block-menu") {
    if (current !== "parameter-editor") return "busy";
    // Measured on a block editor that is a bottom panel with the Grid still above it; a full-screen editor has no measured menu button.
    const panel = (await gateway.graphicsTree()).tree;
    if (!treeShows(panel, "parameter-editor")) return "mismatch";
    if (!/zenUI::ParameterEditor(?![A-Za-z0-9_])/.test(panel)
        || !/zenUI::Grid(?![A-Za-z0-9_])/.test(panel)) return "no-route";
    await tap(G.blockMenuButton);
    return await shows("block-menu") ? "shown" : "not-shown";
  }
  // Rows of the preset menu: settings, tempo, cpu-monitor (bottom) and midi-out, copy-scene (top).
  if (current === "grid") {
    await tap(menuButton);
    if (!await shows("preset-menu")) return "not-shown";
  } else if (current !== "preset-menu") {
    return "busy";
  }
  const top = target === "midi-out" || target === "copy-scene";
  // One drag does not reach the end of the menu (it stops part way), so two are sent, each given time to settle,
  // and the row is tapped only after a further wait: a touch during the scroll animation is ignored.
  const drag = top ? G.menuScrollTop : G.menuScrollBottom;
  for (let pass = 0; pass < 2; pass += 1) {
    // A scroll wait gives the user or another client time to leave the menu.
    // Re-admit every touch against the current screen, not the initial read.
    if (!await stillShows("preset-menu")) return "mismatch";
    announce();
    await gateway.swipeScreen(drag.x, drag.y, drag.toX, drag.toY);
    await sleep(poll.scrollSettleMs ?? 1_500);
  }
  await sleep(poll.rowDelayMs ?? 1_000);
  const row = target === "midi-out" ? G.midiOutRow : target === "copy-scene" ? (detail.swap ? G.swapSceneRow : G.copySceneRow) : BOTTOM_ROWS[target as keyof typeof BOTTOM_ROWS];
  if (!await stillShows("preset-menu")) return "mismatch";
  await tap(row);
  // One tap only: if the dialog does not appear it is not tapped for again.
  if (await untilTree(gateway, poll, (tree) => treeShows(tree, target), poll.rowTimeoutMs ?? 3_000)) return "shown";
  // The row was not where it was expected. Read what the unit shows now and send nothing more into it.
  const seen = await unitScreen(gateway);
  if (seen === "preset-menu") { await gateway.tapScreen(menuButton.x, menuButton.y); return "mismatch"; }
  if (seen === "grid") return "not-shown";
  // Another screen opened. Settings, which a wrong row opens, is left by its header check alone and nothing is touched inside it.
  if (seen === "settings" && target !== "settings") await gateway.tapScreen(done.x, done.y);
  return "mismatch";
}

/**
 * Close a screen on the unit: the kebab toggles its menu, a tap on the check
 * button closes a dialog, and the screens measured to close another way use
 * that (the CPU overlay's X, the search dialog's X, the same label or indicator
 * again, the Copy Scene dialog's CANCEL, an empty area outside the block menu).
 * Only while the unit shows that screen; a dialog nested inside it (Global EQ
 * inside I/O, a Settings pane) may need a second tap, after which a
 * still-open dialog is reported.
 */
export async function closeOnUnit(gateway: RouteGateway, poll: QcScreenPoll, target: QcCloseTarget, beforeGesture?: () => void, detail: QcRouteDetail = {}): Promise<boolean> {
  const point = target === "preset-menu" ? menuButton
    : target === "cpu-monitor" ? G.cpuClose
    : target === "scene-selector" ? G.sceneIndicator
    : target === "route-selector" ? (detail.side === "output" ? G.outputRail : G.inputRail)
    : target === "block-menu" ? G.blockMenuDismiss
    : target === "copy-scene" ? G.copySceneCancel
    : target === "search" ? G.searchClose
    : done;
  for (let attempt = 0; attempt < 2; attempt += 1) {
    if (!treeShows((await gateway.graphicsTree()).tree, target)) return true;
    if (attempt === 0) beforeGesture?.();
    await gateway.tapScreen(point.x, point.y);
    if (await untilTree(gateway, poll, (tree) => !treeShows(tree, target))) return true;
    if (CLOSES_ONCE.includes(target)) return false;
  }
  return false;
}

/** The pixel size of a PNG, read from its IHDR chunk; undefined when it is not one. */
export function pngSize(base64: string): { width: number; height: number } | undefined {
  try {
    const head = atob(base64.slice(0, 44));
    if (head.length < 24 || head.slice(1, 4) !== "PNG") return undefined;
    const word = (offset: number) => ((head.charCodeAt(offset) << 24) | (head.charCodeAt(offset + 1) << 16) | (head.charCodeAt(offset + 2) << 8) | head.charCodeAt(offset + 3)) >>> 0;
    return { width: word(16), height: word(20) };
  } catch { return undefined; }
}

/** Only a frame with the unit's own 800x480 shape is drawn: anything else would be stretched or squashed into the display. */
export function isUnitFrame(base64: string): boolean {
  const size = pngSize(base64);
  return size !== undefined && size.width * 480 === size.height * 800;
}
