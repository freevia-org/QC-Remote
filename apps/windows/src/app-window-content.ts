import type { CorOsDeviceInformation } from "@qc-remote/ui";
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
  /** What the unit reports about itself, for the Device Information screen. Absent until a session answers. */
  information?: CorOsDeviceInformation;
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
  /** "Internet for the Quad Cortex". On unless the settings file says off. */
  updaterRelayEnabled: boolean;
  lastError: string | null;
};

export const DEFAULT_BROKER_TRAY_STATUS: BrokerTrayStatus = {
  trayEnabled: true, idleDisconnectMinutes: 15, trayRunning: false, trayStopping: false, updaterRelayEnabled: true, lastError: null
};

export type SettingsSetting = "form-factor" | "control-access-mode" | "follow-qc-screen" | "tray-enabled" | "idle-disconnect-minutes" | "updater-relay-enabled";

/** The Settings row that lets the Quad Cortex use this computer's internet for updates. */
export const UPDATER_RELAY_LABEL = "Internet for the Quad Cortex";
export const UPDATER_RELAY_EXPLANATION = "Lets the Quad Cortex use this computer's internet connection to reach Neural DSP's servers (neuraldsp.com, plus any extra domain you add) for updates while it is connected over USB. QC Remote only passes the Quad Cortex's own requests through; they include the unit's serial number and its own credentials, which go to the address the unit asks for: Neural DSP's, or an extra domain you added.";

/**
 * What the broker's update relay has done, reduced to what the row can say.
 * It is all the broker's `system.status` carries: that status is readable by
 * the AI apps attached to the broker, so it names no host, path or domain.
 */
export type UpdaterRelaySummary = {
  /** What the background component is doing now (not what the settings file says); null when it did not say. */
  enabled: boolean | null;
  requestsSeen: number;
  requestsWhileDisabled: number;
  inFlight: boolean;
  /** The broker's outcome word for the latest finished request ("completed", "timeout", ...). */
  lastOutcome: string | null;
} | null;

const asCount = (value: unknown): number => typeof value === "number" && Number.isFinite(value) && value > 0 ? Math.floor(value) : 0;

/**
 * The relay diagnostics are untyped JSON from the broker, and an older broker
 * sends none: read each field defensively and return null when there is no
 * object at all.
 */
export function summarizeUpdaterRelay(raw: unknown): UpdaterRelaySummary {
  if (typeof raw !== "object" || raw === null) return null;
  const relay = raw as Record<string, unknown>;
  const outcome = typeof relay.lastOutcome === "string" && relay.lastOutcome.length > 0 && relay.lastOutcome.length <= 80 ? relay.lastOutcome : null;
  return {
    enabled: typeof relay.enabled === "boolean" ? relay.enabled : null,
    requestsSeen: asCount(relay.requestsSeen),
    requestsWhileDisabled: asCount(relay.requestsWhileDisabled),
    inFlight: relay.inFlight === true,
    lastOutcome: outcome
  };
}

const times = (count: number) => count === 1 ? "once" : `${count} times`;

/** The broker's outcome word for a request it refused because the address is not on the allowed list. */
const DECLINED_OUTCOME = "refused:domain-not-allowed";

/**
 * The live line under the row: what the switch is doing now. `enabled` is the
 * stored setting, which the row's checkbox shows. The background component
 * keeps its own copy of it: it notices a change a couple of seconds after the
 * file changes, and it keeps its last value when the file cannot be read or is
 * damaged. When the two differ the line says so, so it never claims more than
 * the component is really doing.
 */
export function updaterRelayStatusLine(enabled: boolean, relay: UpdaterRelaySummary): string | null {
  if (relay && relay.enabled !== null && relay.enabled !== enabled) {
    return enabled
      ? "On in Settings, but QC Remote's background component still has it off. It re-reads the setting every couple of seconds; if this stays, the settings file may be damaged (turning the switch off and on again rewrites it) or this build has no settings file to read."
      : "Off in Settings, but QC Remote's background component still has it on. It re-reads the setting every couple of seconds.";
  }
  if (!enabled) {
    return relay && relay.requestsWhileDisabled > 0
      ? `Off. The Quad Cortex asked for the internet ${times(relay.requestsWhileDisabled)} while this was off.`
      : "Off. The Quad Cortex cannot use this computer's internet connection.";
  }
  if (!relay) return "On.";
  if (relay.inFlight) return "On. Passing a request from the Quad Cortex through now.";
  if (relay.lastOutcome === DECLINED_OUTCOME) {
    return "On. Declined the Quad Cortex's last request: only neuraldsp.com addresses and the extra domains you added are passed through.";
  }
  if (relay.lastOutcome === "completed") return "On. The Quad Cortex's last request went through.";
  if (relay.lastOutcome) return "On. The Quad Cortex's last request did not complete.";
  return "On. The Quad Cortex has not asked for anything yet.";
}

/** Preserve invalid input for Rust to reject; NaN would otherwise serialize as null (Never). */
export function idleMinutesFromInput(text: string, valueAsNumber: number): number | string {
  return Number.isFinite(valueAsNumber) ? valueAsNumber : text;
}

export function idleDisconnectHelp(minutes: number | null, trayEnabled: boolean): string {
  if (!trayEnabled) return "Enable the background broker to apply this release setting.";
  if (minutes === null) return "Never: keep the device connected until you choose Disconnect or Quit.";
  if (minutes === 0) return "Immediately: release the device about five seconds after the last attached app or AI client disconnects.";
  return `Release the device ${minutes} minute${minutes === 1 ? "" : "s"} after the last attached app or AI client disconnects. An AI app that stays connected keeps the device, even when idle.`;
}

/** What the Settings window shows, including the native tray's applied status. */
export type SettingsState = BrokerTrayStatus & {
  /** What the broker's update relay reports; null when the broker sends none. */
  updaterRelay: UpdaterRelaySummary;
  formFactorId: string;
  formFactors: readonly { id: string; displayName: string }[];
  controlAccessMode: ControlAccessMode;
  followQcScreen: boolean;
  localAccessCondition: LocalAccessCondition | null;
};
