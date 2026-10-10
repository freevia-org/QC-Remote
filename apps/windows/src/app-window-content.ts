/**
 * The data behind the three app windows: Settings, Device Info, and Workspace.
 *
 * Each is a native window of its own (`open_app_window` in the Rust shell),
 * loaded as `index.html?window=<kind>`, exactly as the Help windows are. This
 * module is the app windows' counterpart of `about-content.ts`: plain `.ts`, so
 * a test can execute the kinds, titles, and query parsing rather than read them
 * (Node's `--experimental-strip-types` runner cannot load a `.tsx` module).
 *
 * The state types are what the main window sends each window over the event
 * bus. They are plain, already-formatted values, because the windows share no
 * React state, no device snapshot, and no connection workflow with the main
 * window; each window only shows what it is sent.
 */

import type { ControlAccessMode } from "@qc-remote/core";

/** The three app windows, as the main window names them to the Rust side. */
export type AppWindowKind = "settings" | "device-info" | "workspace";

export const APP_WINDOW_KINDS: readonly AppWindowKind[] = ["settings", "device-info", "workspace"];

/** Each app window's title, the same one the Rust shell gives its native window. */
export const APP_WINDOW_TITLES: Readonly<Record<AppWindowKind, string>> = {
  settings: "QC Remote — Settings",
  "device-info": "QC Remote — Device Info",
  workspace: "QC Remote — Workspace"
};

/**
 * Which app window this webview is, from its `?window=` query parameter.
 * Null for the main window and for every Help window.
 */
export function appWindowKind(search: string): AppWindowKind | null {
  const kind = new URLSearchParams(search).get("window");
  return APP_WINDOW_KINDS.find((entry) => entry === kind) ?? null;
}

/** What the Device Info window shows: the live device, as the main window sees it. */
export type DeviceInfoState = {
  deviceName: string;
  /** "ready", "verifying", or the connection phase, as the in-app dialog said it. */
  connection: string;
  setlistName: string;
  presetLocation: string;
  presetName: string;
  mode: string;
  /** The active scene's letter. */
  scene: string;
  tempo: number;
  blockCount: number;
  dirty: boolean;
};

/**
 * What the Workspace window shows: the opened workspace file's snapshot, or
 * null when no workspace is open (the window then says so rather than showing
 * an empty table).
 */
export type WorkspaceState = {
  name: string;
  /** ISO timestamp the workspace was saved at. */
  savedAt: string;
  setlistName: string;
  presetLocation: string;
  presetName: string;
  scene: string;
  blockCount: number;
  dirty: boolean;
} | null;

/** A notice about local AI access, as `localAccessNotice` words it. */
export type LocalAccessCondition = {
  tone: "pending" | "ready" | "action-required" | "error";
  message: string;
  detail?: string;
};

export type BrokerTrayStatus = {
  trayEnabled: boolean;
  idleDisconnectMinutes: number | null;
  trayRunning: boolean;
  trayStopping: boolean;
  lastError: string | null;
};

export const DEFAULT_BROKER_TRAY_STATUS: BrokerTrayStatus = {
  trayEnabled: true, idleDisconnectMinutes: 15, trayRunning: false, trayStopping: false, lastError: null
};

export type SettingsSetting = "form-factor" | "control-access-mode" | "follow-qc-screen" | "tray-enabled" | "idle-disconnect-minutes";

/** Preserve invalid input for Rust to reject; NaN would otherwise serialize as null (Never). */
export function idleMinutesFromInput(text: string, valueAsNumber: number): number | string {
  return Number.isFinite(valueAsNumber) ? valueAsNumber : text;
}

export function idleDisconnectHelp(minutes: number | null, trayEnabled: boolean): string {
  if (!trayEnabled) return "Show the tray icon to apply this release setting.";
  if (minutes === null) return "Never: keep the device connected until you choose Disconnect or Quit.";
  if (minutes === 0) return "Immediately: release the device about five seconds after the last attached app or AI client disconnects.";
  return `Release the device ${minutes} minute${minutes === 1 ? "" : "s"} after the last attached app or AI client disconnects. An AI app that stays connected keeps the device, even when idle.`;
}

/** What the Settings window shows, including the native tray's applied status. */
export type SettingsState = BrokerTrayStatus & {
  formFactorId: string;
  formFactors: readonly { id: string; displayName: string }[];
  controlAccessMode: ControlAccessMode;
  followQcScreen: boolean;
  localAccessCondition: LocalAccessCondition | null;
};
