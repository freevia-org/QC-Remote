import { createGatewayClientTransport, withDeviceConnectionSource, type ConnectionState, type DiagnosticsReport, type GatewayTransport, type RuntimeStatus, type WorkspaceDocument, type WorkspaceFileResult } from "@qc-remote/client";
import type { HelpWindowKind, IssueKind } from "./about-content";
import { DEFAULT_BROKER_TRAY_STATUS, type AppWindowKind, type BrokerTrayStatus, type DeviceInfoState, type SettingsSetting, type SettingsState, type WorkspaceState } from "./app-window-content.ts";
// With its extension: Node's test runner loads this module directly.
import { normalizeBrokerConnection } from "./qc-readiness.ts";
import type { VirtualDeviceStorage } from "@qc-remote/ui";
import { callTauri } from "./tauri-invoke.ts";

export { publicRelay, publicRelayBrowserPairing } from "./public-relay-transport.ts";

export interface PhysicalQcDevice { id: string; name: string; connected?: boolean; phase?: string }
export const physicalDevices = {
  /** `brokerInstanceId` names the broker generation that answered; an older host omits it. */
  list(): Promise<{devices: PhysicalQcDevice[]; selectedDeviceId: string | null; brokerInstanceId?: string | null}> {
    return callTauri("broker_devices");
  },
  select(deviceId: string): Promise<{selectedDeviceId: string}> {
    return callTauri("select_broker_device", {deviceId});
  }
};

export const brokerDevice = {
  disconnect(): Promise<ConnectionState> {
    return callTauri<ConnectionState>("disconnect_broker_device");
  },
  /**
   * The user's own Connect, USB or device click in this window supersedes a
   * recorded Quit, so the connect that follows may start QC Remote's broker
   * again. Never called on mount, by a timer or on device detection: those
   * keep respecting the Quit.
   */
  acknowledgeUserConnect(): Promise<void> {
    if (!window.__TAURI_INTERNALS__) return Promise.resolve();
    return callTauri<void>("acknowledge_user_connect");
  }
};

export const brokerTray = {
  status(): Promise<BrokerTrayStatus> {
    if (!window.__TAURI_INTERNALS__) return Promise.resolve({ ...DEFAULT_BROKER_TRAY_STATUS });
    return callTauri<BrokerTrayStatus>("broker_tray_settings");
  },
  setEnabled(enabled: unknown): Promise<BrokerTrayStatus> {
    if (typeof enabled !== "boolean") return Promise.reject(new Error("trayEnabled must be a boolean"));
    return callTauri<BrokerTrayStatus>("set_broker_tray_enabled", { enabled });
  },
  /** An app setting, not a Quad Cortex action: no gateway method and no AI tool reaches it. */
  setUpdaterRelayEnabled(enabled: unknown): Promise<BrokerTrayStatus> {
    if (typeof enabled !== "boolean") return Promise.reject(new Error("updaterRelayEnabled must be a boolean"));
    return callTauri<BrokerTrayStatus>("set_broker_updater_relay_enabled", { enabled });
  },
  setIdleMinutes(value: unknown): Promise<BrokerTrayStatus> {
    // JSON turns NaN/Infinity into null. Neither malformed input nor an absent
    // event value is the user's explicit request to turn the policy off.
    const minutes = value === undefined || (typeof value === "number" && !Number.isFinite(value)) ? "" : value;
    return callTauri<BrokerTrayStatus>("set_broker_tray_idle_minutes", { minutes });
  }
};

/** A startup effect may fail after the preference was saved. Always recover the
 * applied status before showing that failure, while preserving its reason. */
export async function changeBrokerTray(
  change: () => Promise<BrokerTrayStatus>,
  apply: (status: BrokerTrayStatus) => void,
  readStatus: () => Promise<BrokerTrayStatus> = brokerTray.status
): Promise<BrokerTrayStatus> {
  try {
    const status = await change();
    apply(status);
    return status;
  } catch (error) {
    try { apply(await readStatus()); } catch { /* Keep the original command refusal. */ }
    throw error;
  }
}

/** Native event names shared by the app shell and its renderer-side listeners. */
export const TAURI_EVENTS = {
  feedbackIssueKind: "feedback-issue-kind",
  devicePhase: "qc-device-phase",
  stateFrame: "qc-state-frame"
} as const;

const SKILL_STATES = ["not-installed", "current", "update-available", "newer-installed", "modified", "different-configuration", "status-error"] as const;
type SkillState = typeof SKILL_STATES[number];

/** Attach a Tauri event listener through the lazy, web-preview-safe bridge. */
export function listenTauri<T>(event: string, handler: (event: { payload: T }) => void): Promise<() => void> {
  if (!window.__TAURI_INTERNALS__) return Promise.resolve(() => undefined);
  return import("@tauri-apps/api/event").then(({ listen }) => listen<T>(event, handler));
}

export function macosMenuEventsReady(): Promise<void> {
  return callTauri<void>("macos_menu_events_ready");
}

const generatedGatewayTransport = createGatewayClientTransport<GatewayTransport>(
  <T,>(method: string, params?: Record<string, unknown>) => callTauri<T>("gateway_invoke", { method, params }),
  "rpc"
);

export const tauriTransport: GatewayTransport = withDeviceConnectionSource({
  ...generatedGatewayTransport,
  async runtimeStatus(): Promise<RuntimeStatus> {
    if (!window.__TAURI_INTERNALS__) {
      return {
        platform: "Browser preview",
        gatewayAvailable: false,
        message: "UI preview mode — the device gateway is not attached."
      };
    }
    return generatedGatewayTransport.runtimeStatus();
  },
  // The broker answers a reconnect with its own worker phase; every surface
  // reads the host's vocabulary, so the result is translated here, once.
  async reconnect() {
    return normalizeBrokerConnection(await generatedGatewayTransport.reconnect());
  },
  async resetSession() {
    return normalizeBrokerConnection(await generatedGatewayTransport.resetSession());
  },
  createDeviceBackup(name: string): Promise<WorkspaceFileResult> {
    return callTauri<WorkspaceFileResult>("create_device_backup", { name });
  }
});

/**
 * The virtual device's one JSON document, at
 * `%LOCALAPPDATA%\QC Remote\virtual-device\store.json` (`VIRTUAL_DEVICE_STORE_RELATIVE_PATH`).
 * The native side owns the path; the webview can only read or replace that
 * single file, never name another.
 */
export const virtualDeviceStorage: VirtualDeviceStorage = {
  read(): Promise<string | null> {
    return callTauri<string | null>("read_virtual_device_store");
  },
  write(text: string): Promise<void> {
    return callTauri<void>("write_virtual_device_store", { text });
  }
};

export const workspaceFiles = {
  saveAs(document: WorkspaceDocument, suggestedName: string): Promise<WorkspaceFileResult> {
    return callTauri<WorkspaceFileResult>("save_workspace_as", { document, suggestedName });
  },
  save(path: string, document: WorkspaceDocument): Promise<WorkspaceFileResult> {
    return callTauri<WorkspaceFileResult>("save_workspace", { path, document });
  },
  open(): Promise<WorkspaceFileResult> {
    return callTauri<WorkspaceFileResult>("open_workspace");
  }
};

export const diagnosticsFiles = {
  export(report: DiagnosticsReport): Promise<WorkspaceFileResult> {
    return callTauri<WorkspaceFileResult>("export_diagnostics", { report });
  }
};

export const externalLinks = {
  open(url: string): Promise<void> {
    return callTauri<void>("open_external_url", { url });
  }
};

/**
 * The User Guide, Feedback, and About windows. Each is its own native window,
 * and opening one that is already open focuses it rather than making a copy.
 *
 * `issue` opens the Feedback window on one kind of report. A Feedback window
 * that is already open is focused and switched to that kind.
 */
export const helpWindows = {
  open(kind: HelpWindowKind, issue?: IssueKind): Promise<void> {
    return callTauri<void>("open_help_window", { kind, issue: issue ?? null });
  }
};

/** The event the Rust shell sends an already-open Feedback window to switch its kind of report. */
export const FEEDBACK_ISSUE_KIND_EVENT = TAURI_EVENTS.feedbackIssueKind;

/**
 * Feedback window: follow the Help menu when it asks this already-open window
 * for the other kind of report. Returns the function that stops listening.
 */
export function onFeedbackIssueKind(handler: (kind: IssueKind) => void): () => void {
  if (!window.__TAURI_INTERNALS__) return () => undefined;
  let disposed = false;
  let unlisten: (() => void) | undefined;
  void listenTauri<string>(FEEDBACK_ISSUE_KIND_EVENT, ({ payload }) => {
    if (!disposed && (payload === "bug" || payload === "feature")) handler(payload);
  }).then((stop) => { if (disposed) stop(); else unlisten = stop; })
    .catch((error: unknown) => { console.error(`QC Remote could not listen for ${FEEDBACK_ISSUE_KIND_EVENT}:`, error); });
  return () => { disposed = true; unlisten?.(); };
}

/** The native broker's own session phase, pushed whenever it changes. */
export interface DevicePhaseEvent {
  deviceId?: string;
  phase: string;
  detail: string;
  connected: boolean;
  synchronized: boolean;
}

/**
 * Follow the broker's session phase while a reconnect is still pending.
 *
 * `device.reconnect` holds the gateway for the whole handshake, so nothing
 * can be asked of the broker until it returns; this pushed stream is the only
 * way to see, meanwhile, whether a Quad Cortex has answered. Returns the
 * function that stops listening.
 */
export function onDevicePhase(handler: (event: DevicePhaseEvent) => void): () => void {
  if (!window.__TAURI_INTERNALS__) return () => undefined;
  let disposed = false;
  let unlisten: (() => void) | undefined;
  void listenTauri<DevicePhaseEvent>(TAURI_EVENTS.devicePhase, ({ payload }) => {
    if (!disposed) handler(payload);
  }).then((stop) => { if (disposed) stop(); else unlisten = stop; })
    .catch((error: unknown) => { console.error("QC Remote could not listen for qc-device-phase:", error); });
  return () => { disposed = true; unlisten?.(); };
}

/**
 * The host operating system's name and version, such as "Windows 11 Pro 24H2
 * (build 26100.2033)", read by the Rust shell from the registry. Null when it
 * could not be read.
 */
export const hostSystem = {
  osVersion(): Promise<string | null> {
    return callTauri<string | null>("host_os_version");
  }
};

/**
 * The events that carry a diagnostics request from the Feedback window to the
 * main window and its outcome back.
 *
 * The report is built from the main window's live state (device snapshot,
 * connection phase, lifecycle events), which the Feedback window, a separate
 * webview, has no access to. The main window answers every request; the
 * Feedback window shows the outcome in its own message line.
 */
export const DIAGNOSTICS_EVENTS = {
  exportRequest: "request-diagnostics-export",
  exportResult: "diagnostics-export-result",
  copyRequest: "request-diagnostics-copy",
  copyResult: "diagnostics-copy-result"
} as const;

/** A window's request to the main window. `value` is what a Settings change asks for. */
type MainWindowRequest = { id: string; value?: unknown };
/**
 * The main window's answer. It first sends `{ id, accepted: true }` the moment
 * it takes the request, then the outcome. The acknowledgement is what lets the
 * asking window tell "the main window is not listening" (no acknowledgement
 * within the timeout) from "the user is still in the save dialog".
 */
type MainWindowResult = { id: string; accepted?: true; ok: boolean; message: string };

/** How long a window waits for the main window to acknowledge a request, and for a bounded request to finish. */
export const DIAGNOSTICS_ANSWER_TIMEOUT_MS = 10_000;
export const DIAGNOSTICS_NO_ANSWER = "The main window did not answer. Try again from Help > Report an Issue….";

/**
 * Main window: answer requests from the other windows, each on its own
 * request/result event pair.
 *
 * Every request is acknowledged the moment it is taken, then answered with
 * the handler's outcome: its resolved message, or its failure. A failed emit
 * would leave the asking window waiting with nothing to show, so it is logged
 * here rather than dropped as an unhandled rejection. Returns the function
 * that stops answering.
 */
function answerMainWindowRequests(routes: ReadonlyArray<{ request: string; result: string; run: (value: unknown) => Promise<string> }>): () => void {
  if (!window.__TAURI_INTERNALS__) return () => undefined;
  let disposed = false;
  const unlisteners: Array<() => void> = [];
  const pendingAdmissions = new Set<() => void>();
  for (const { request, result, run } of routes) {
    void import("@tauri-apps/api/event").then(({ emit }) => {
      if (disposed) return;
      return listenTauri<MainWindowRequest>(request, async ({ payload }) => {
      const send = (outcome: MainWindowResult) => emit(result, outcome)
        .catch((error: unknown) => { console.error(`QC Remote could not send ${result}:`, error); });
      const cancelled = () => send({ id: payload.id, ok: false, message: "The main window closed before this request started. Reopen it and try again." });
      // This listener owns no request until it acknowledges it. A replacement
      // listener may be handling this new event while native unlisten settles.
      if (disposed) return;
      let timer: ReturnType<typeof setTimeout> | undefined;
      let releaseAdmission: () => void = () => undefined;
      const unavailable = new Promise<boolean>((resolve) => {
        releaseAdmission = () => resolve(false);
        pendingAdmissions.add(releaseAdmission);
        timer = setTimeout(releaseAdmission, DIAGNOSTICS_ANSWER_TIMEOUT_MS);
      });
      let accepted: boolean;
      try {
        // Native emit can deliver accepted before its IPC receipt settles.
        // Bound receipt admission separately; failed delivery never admits run.
        accepted = await Promise.race([
          emit(result, { id: payload.id, accepted: true, ok: true, message: "" } satisfies MainWindowResult)
            .then(() => true, (error: unknown) => {
              console.error(`QC Remote could not acknowledge ${request}:`, error);
              return false;
            }),
          unavailable
        ]);
      } finally {
        clearTimeout(timer);
        pendingAdmissions.delete(releaseAdmission);
      }
      // Disposal/stalled acknowledgement refuses this listener's unstarted
      // handler. Do not await another native receipt before returning. This
      // doesn't claim cancellation of IPC or any other listener's action.
      if (disposed) { void cancelled(); return; }
      if (!accepted) {
        void send({ id: payload.id, ok: false, message: "This window did not start the request because its acknowledgement could not be completed. Check the current state before trying again." });
        return;
      }
      // Once run starts, its real result settles even after this listener closes.
      let outcome: MainWindowResult;
      try {
        outcome = { id: payload.id, ok: true, message: await run(payload.value) };
      } catch (error) {
        outcome = { id: payload.id, ok: false, message: error instanceof Error ? error.message : String(error) };
      }
      await send(outcome);
      });
    }).then((unlisten) => { if (!unlisten) return; if (disposed) unlisten(); else unlisteners.push(unlisten); })
      .catch((error: unknown) => { console.error(`QC Remote could not answer ${request}:`, error); });
  }
  return () => { disposed = true; for (const release of pendingAdmissions) release(); for (const unlisten of unlisteners) unlisten(); };
}

let mainWindowRequestCount = 0;

/**
 * Any other window: ask the main window for something and wait for its answer.
 *
 * Fails with `noAnswer` when the main window does not acknowledge within
 * `DIAGNOSTICS_ANSWER_TIMEOUT_MS` (its listener is gone, for instance after a
 * reload). Once acknowledged, an `"unbounded"` request (one that opens a save
 * dialog) waits as long as the user keeps that dialog open; a `"bounded"` one
 * has nothing to wait on but the main window's own work, so the same limit
 * covers its whole round trip.
 *
 * The result listener is attached before the request goes out, so an answer
 * cannot arrive unheard, and each request carries an id so two presses in a
 * row each settle on their own answer. Resolves with the main window's message;
 * rejects with its failure.
 */
async function askMainWindow(ask: { request: string; result: string; value?: unknown; afterAccept: "bounded" | "unbounded"; noAnswer: string }): Promise<string> {
  if (!window.__TAURI_INTERNALS__) {
    throw new Error("Desktop runtime is not active. Start with npm run tauri:dev.");
  }
  mainWindowRequestCount += 1;
  const id = `${Date.now()}-${mainWindowRequestCount}`;
  let settle: (outcome: MainWindowResult) => void = () => undefined;
  const answer = new Promise<MainWindowResult>((resolve) => { settle = resolve; });
  let timer: ReturnType<typeof setTimeout> | undefined;
  let expired = false;
  let finished = false;
  let acknowledged = false;
  const noAnswer = new Promise<never>((_, reject) => {
    timer = setTimeout(() => { expired = true; reject(new Error(ask.noAnswer)); }, DIAGNOSTICS_ANSWER_TIMEOUT_MS);
  });
  // Observed here as well as in the race below, so a timeout that fires while
  // the request is still being sent is never an unhandled rejection.
  noAnswer.catch(() => undefined);
  let unlisten: (() => void) | undefined;
  try {
    const { emit } = await Promise.race([import("@tauri-apps/api/event"), noAnswer]);
    if (expired) throw new Error(ask.noAnswer);
    // Registration is native work already started: if it finishes after the
    // deadline, its actual unlistener is still owned and retired exactly once.
    const listening = listenTauri<MainWindowResult>(ask.result, ({ payload }) => {
      if (finished || expired || payload.id !== id) return;
      if (!payload.accepted) { settle(payload); return; }
      acknowledged = true;
      // A save dialog may stay open indefinitely once the main window has the
      // request; everything else keeps its limit.
      if (ask.afterAccept === "unbounded") clearTimeout(timer);
    }).then((stop) => {
      if (finished || expired) stop();
      else unlisten = stop;
    });
    await Promise.race([listening, noAnswer]);
    if (expired) throw new Error(ask.noAnswer);
    // The result/acceptance can arrive before the native emit promise settles.
    // Don't let a stalled emit bypass the bound or strand an accepted save.
    void emit(ask.request, { id, value: ask.value } satisfies MainWindowRequest)
      .catch((error: unknown) => {
        if (!finished && !acknowledged) settle({ id, ok: false, message: error instanceof Error ? error.message : String(error) });
        else if (!finished) console.error(`QC Remote emit failed after the main window accepted ${ask.request}:`, error);
      });
    const outcome = await Promise.race([answer, noAnswer]);
    if (!outcome.ok) throw new Error(outcome.message);
    return outcome.message;
  } finally {
    finished = true;
    clearTimeout(timer);
    unlisten?.();
  }
}

/**
 * Main window: answer the Feedback window's diagnostics requests.
 *
 * `exportReport` resolves with the outcome line the Feedback window shows.
 * `reportText` resolves with the redacted report itself rather than writing it
 * to the clipboard here: the clipboard refuses a write from a document that
 * does not have focus, and the focused window is the Feedback window, which
 * writes it. Returns the function that stops answering.
 */
export function serveDiagnosticsRequests(handlers: { exportReport: () => Promise<string>; reportText: () => Promise<string> }): () => void {
  return answerMainWindowRequests([
    { request: DIAGNOSTICS_EVENTS.exportRequest, result: DIAGNOSTICS_EVENTS.exportResult, run: () => handlers.exportReport() },
    { request: DIAGNOSTICS_EVENTS.copyRequest, result: DIAGNOSTICS_EVENTS.copyResult, run: () => handlers.reportText() }
  ]);
}

/**
 * Feedback window: ask the main window for diagnostics and wait for its answer.
 * Export opens a save dialog, so it waits as long as that dialog stays open
 * once acknowledged; Copy is bounded end to end. Resolves with the main
 * window's message (the export outcome, or the report text).
 */
export function requestDiagnostics(kind: "export" | "copy"): Promise<string> {
  return askMainWindow(kind === "export"
    ? { request: DIAGNOSTICS_EVENTS.exportRequest, result: DIAGNOSTICS_EVENTS.exportResult, afterAccept: "unbounded", noAnswer: DIAGNOSTICS_NO_ANSWER }
    : { request: DIAGNOSTICS_EVENTS.copyRequest, result: DIAGNOSTICS_EVENTS.copyResult, afterAccept: "bounded", noAnswer: DIAGNOSTICS_NO_ANSWER });
}

/**
 * The Settings, Device Info, and Workspace windows. Each is its own native
 * window, opened by the same one-per-label mechanism as the Help windows:
 * opening one that is already open focuses it rather than making a copy.
 */
export const appWindows = {
  open(kind: AppWindowKind): Promise<void> {
    return callTauri<void>("open_app_window", { kind });
  }
};

/**
 * The events that carry the main window's state to the app windows, and the
 * Settings and Workspace windows' requests back.
 *
 * Named the way `DIAGNOSTICS_EVENTS` are: a window's ask is `request-<subject>`
 * and the main window's answer is `<subject>-result`, sent first as an
 * acknowledgement and then as the outcome. A state event has no `-result`
 * suffix because it is both the answer to its request and the push the main
 * window sends whenever that state changes, so a window that asked once stays
 * current without polling.
 */
export const DEVICE_INFO_EVENTS = {
  stateRequest: "request-device-info-state",
  state: "device-info-state"
} as const;

export const WORKSPACE_EVENTS = {
  stateRequest: "request-workspace-state",
  state: "workspace-state",
  saveCopyRequest: "request-workspace-save-copy",
  saveCopyResult: "workspace-save-copy-result"
} as const;

export const SETTINGS_EVENTS = {
  stateRequest: "request-settings-state",
  state: "settings-state",
  formFactorRequest: "request-form-factor-change",
  formFactorResult: "form-factor-change-result",
  accessModeRequest: "request-control-access-mode-change",
  accessModeResult: "control-access-mode-change-result",
  followQcScreenRequest: "request-follow-qc-screen-change",
  followQcScreenResult: "follow-qc-screen-change-result",
  trayEnabledRequest: "request-broker-tray-enabled-change",
  trayEnabledResult: "broker-tray-enabled-change-result",
  idleMinutesRequest: "request-idle-disconnect-minutes-change",
  idleMinutesResult: "idle-disconnect-minutes-change-result",
  updaterRelayRequest: "request-updater-relay-enabled-change",
  updaterRelayResult: "updater-relay-enabled-change-result"
} as const;

/** What a Settings or Workspace window says when the main window never acknowledges its request. */
export const SETTINGS_NO_ANSWER = "The main window isn't responding. The change may still finish; reopen Settings to check its current value before trying again.";
export const WORKSPACE_NO_ANSWER = "The main window isn't responding. The save may still finish; check the destination before trying again.";

/** A window's own state event and the request that asks the main window for it. */
type WindowStateEvents = { stateRequest: string; state: string };

/**
 * Main window: send one window's current state. Called whenever that state
 * changes, and by `serveAppWindowRequests` when a window asks. An emit nobody
 * listens for (the window is closed) is a no-op.
 */
export function publishWindowState<State>(events: WindowStateEvents, state: State): void {
  if (!window.__TAURI_INTERNALS__) return;
  void import("@tauri-apps/api/event").then(({ emit }) => emit(events.state, state))
    .catch((error: unknown) => { console.error(`QC Remote could not send ${events.state}:`, error); });
}

/**
 * Main window: answer the app windows' state requests, and the Settings and
 * Workspace windows' action requests.
 *
 * `setFormFactor` and `setControlAccessMode` apply the change with the same
 * functions the main window's own controls call, so it stays the one place a
 * change is applied and remembered; each resolves with the line the Settings
 * window shows, and rejects when the value names no real choice. Action
 * requests are acknowledged and answered exactly as diagnostics requests are.
 * Returns the function that stops answering.
 */
export function serveAppWindowRequests(handlers: {
  deviceInfoState: () => DeviceInfoState;
  workspaceState: () => WorkspaceState;
  settingsState: () => SettingsState;
  setFormFactor: (formFactorId: unknown) => Promise<string>;
  setControlAccessMode: (mode: unknown) => Promise<string>;
  setFollowQcScreen: (enabled: unknown) => Promise<string>;
  setBrokerTrayEnabled: (enabled: unknown) => Promise<string>;
  setIdleDisconnectMinutes: (minutes: unknown) => Promise<string>;
  setUpdaterRelayEnabled: (enabled: unknown) => Promise<string>;
  saveWorkspaceCopy: (documentId: unknown) => Promise<string>;
}): () => void {
  if (!window.__TAURI_INTERNALS__) return () => undefined;
  let disposed = false;
  const unlisteners: Array<() => void> = [];
  const on = (event: string, run: () => void) => {
    void listenTauri<unknown>(event, () => { if (!disposed) run(); })
      .then((unlisten) => { if (disposed) unlisten(); else unlisteners.push(unlisten); })
      .catch((error: unknown) => { console.error(`QC Remote could not answer ${event}:`, error); });
  };
  on(DEVICE_INFO_EVENTS.stateRequest, () => publishWindowState(DEVICE_INFO_EVENTS, handlers.deviceInfoState()));
  on(WORKSPACE_EVENTS.stateRequest, () => publishWindowState(WORKSPACE_EVENTS, handlers.workspaceState()));
  on(SETTINGS_EVENTS.stateRequest, () => publishWindowState(SETTINGS_EVENTS, handlers.settingsState()));
  const stopAnswering = answerMainWindowRequests([
    { request: SETTINGS_EVENTS.formFactorRequest, result: SETTINGS_EVENTS.formFactorResult, run: handlers.setFormFactor },
    { request: SETTINGS_EVENTS.accessModeRequest, result: SETTINGS_EVENTS.accessModeResult, run: handlers.setControlAccessMode },
    { request: SETTINGS_EVENTS.followQcScreenRequest, result: SETTINGS_EVENTS.followQcScreenResult, run: handlers.setFollowQcScreen },
    { request: SETTINGS_EVENTS.trayEnabledRequest, result: SETTINGS_EVENTS.trayEnabledResult, run: handlers.setBrokerTrayEnabled },
    { request: SETTINGS_EVENTS.idleMinutesRequest, result: SETTINGS_EVENTS.idleMinutesResult, run: handlers.setIdleDisconnectMinutes },
    { request: SETTINGS_EVENTS.updaterRelayRequest, result: SETTINGS_EVENTS.updaterRelayResult, run: handlers.setUpdaterRelayEnabled },
    { request: WORKSPACE_EVENTS.saveCopyRequest, result: WORKSPACE_EVENTS.saveCopyResult, run: handlers.saveWorkspaceCopy }
  ]);
  return () => { disposed = true; stopAnswering(); for (const unlisten of unlisteners) unlisten(); };
}

/** What an app window says when the main window never answers its state request. */
export const WINDOW_STATE_NO_ANSWER = "The main window isn't responding, so there is nothing to show yet. Close this window and reopen it from the main window.";

/**
 * App window: follow one kind of main-window state. The listener is attached
 * before the request goes out, so the immediate answer cannot arrive unheard;
 * every later push reaches `handler` the same way. Returns the function that
 * stops listening.
 *
 * Bounded like every action request in this bridge. When no state has arrived
 * `DIAGNOSTICS_ANSWER_TIMEOUT_MS` after the request (the main window's
 * listener was not attached yet, or was lost to a reload), the request is sent
 * once more; if that one goes unanswered for the same time too, `onNoAnswer`
 * runs, so the window can say so instead of waiting forever. A state that
 * arrives later is still shown.
 */
export function subscribeWindowState<State>(events: WindowStateEvents, handler: (state: State) => void, onNoAnswer?: () => void): () => void {
  if (!window.__TAURI_INTERNALS__) return () => undefined;
  let disposed = false;
  let answered = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let unlisten: (() => void) | undefined;
  let emitRequest: (() => Promise<void>) | undefined;
  const send = () => {
    if (disposed || !emitRequest) return;
    void emitRequest().catch((error: unknown) => { console.error(`QC Remote could not ask for ${events.state}:`, error); });
  };
  // Import, native listener admission and emit may themselves be delayed.
  // Count both answer windows from subscription entry, never from their
  // completion. Late read-only state is still accepted for recovery.
  timer = setTimeout(() => {
    if (disposed || answered) return;
    send();
    if (disposed || answered) return;
    timer = setTimeout(() => { if (!disposed && !answered) onNoAnswer?.(); }, DIAGNOSTICS_ANSWER_TIMEOUT_MS);
  }, DIAGNOSTICS_ANSWER_TIMEOUT_MS);
  void import("@tauri-apps/api/event").then(async ({ emit }) => {
    if (disposed) return;
    const stop = await listenTauri<State>(events.state, ({ payload }) => {
      if (!disposed) { answered = true; clearTimeout(timer); handler(payload); }
    });
    if (disposed) { stop(); return; }
    unlisten = stop;
    emitRequest = () => emit(events.stateRequest);
    send();
  }).catch((error: unknown) => {
    console.error(`QC Remote could not follow ${events.state}:`, error);
    clearTimeout(timer);
    if (!disposed && !answered) onNoAnswer?.();
  });
  return () => { disposed = true; clearTimeout(timer); unlisten?.(); };
}

/**
 * Settings window: ask the main window to apply a change and wait for its
 * outcome. Bounded end to end, like Copy diagnostics: applying a choice opens
 * no dialog. Resolves with the line to show; rejects with the main window's
 * refusal, or with `SETTINGS_NO_ANSWER` when it never acknowledges.
 */
export function requestSettingsChange(setting: SettingsSetting, value: unknown): Promise<string> {
  const route = {
    "form-factor": { request: SETTINGS_EVENTS.formFactorRequest, result: SETTINGS_EVENTS.formFactorResult },
    "control-access-mode": { request: SETTINGS_EVENTS.accessModeRequest, result: SETTINGS_EVENTS.accessModeResult },
    "follow-qc-screen": { request: SETTINGS_EVENTS.followQcScreenRequest, result: SETTINGS_EVENTS.followQcScreenResult },
    "tray-enabled": { request: SETTINGS_EVENTS.trayEnabledRequest, result: SETTINGS_EVENTS.trayEnabledResult },
    "idle-disconnect-minutes": { request: SETTINGS_EVENTS.idleMinutesRequest, result: SETTINGS_EVENTS.idleMinutesResult },
    "updater-relay-enabled": { request: SETTINGS_EVENTS.updaterRelayRequest, result: SETTINGS_EVENTS.updaterRelayResult }
  }[setting];
  return askMainWindow({ ...route, value, afterAccept: "bounded", noAnswer: SETTINGS_NO_ANSWER });
}

/**
 * Workspace window: ask the main window to save a copy of the workspace. It
 * opens a save dialog, so once acknowledged it waits as long as Export
 * diagnostics does. Resolves with the outcome line; rejects with the failure,
 * or with `WORKSPACE_NO_ANSWER` when the main window never acknowledges.
 */
export function requestWorkspaceSaveCopy(documentId: string): Promise<string> {
  return askMainWindow({ request: WORKSPACE_EVENTS.saveCopyRequest, result: WORKSPACE_EVENTS.saveCopyResult, value: documentId, afterAccept: "unbounded", noAnswer: WORKSPACE_NO_ANSWER });
}

export interface LocalClientPackageResult {
  cancelled: boolean;
  fileName?: string;
}

/**
 * The pairing states each native status document can report, as values.
 *
 * The `state` fields below are typed from these arrays rather than spelling
 * the same words a second time, so a test can ask what a provider is able to
 * report - "does Antigravity report a connection newer than this build?" -
 * by reading the declaration through the module instead of matching this
 * file's text. Adding a state to a provider changes the value and the type in
 * one edit, and a test that asserts a provider has no such state fails the
 * moment the native layer gains one.
 */
export const CLAUDE_EXTENSION_STATES = [
  "claude-not-installed", "not-installed", "current", "update-available", "newer-installed", "different-version"
] as const;
export type ClaudeDesktopExtensionState = (typeof CLAUDE_EXTENSION_STATES)[number];

export const CHATGPT_INTEGRATION_STATES = [
  "chatgpt-not-installed", "not-installed", "current", "update-available", "newer-installed", "different-configuration", "status-error"
] as const;
export type ChatGptLocalIntegrationState = (typeof CHATGPT_INTEGRATION_STATES)[number];

export const ANTIGRAVITY_INTEGRATION_STATES = [
  "antigravity-not-installed", "not-installed", "current", "update-available", "different-configuration", "status-error"
] as const;
export type AntigravityLocalIntegrationState = (typeof ANTIGRAVITY_INTEGRATION_STATES)[number];

export const MANAGED_JSON_MCP_STATES = [
  "app-not-installed", "app-update-required", "not-installed", "current", "update-available", "different-configuration", "status-error"
] as const;
export type ManagedJsonMcpIntegrationState = (typeof MANAGED_JSON_MCP_STATES)[number];

export interface ClaudeDesktopExtensionStatus {
  /** The sandbox awaits an explicit host-folder selection; polling never prompts. */
  folderAccessRequired?: boolean;
  claudeInstalled: boolean;
  appVersion?: string;
  extensionInstalled: boolean;
  installedVersion?: string;
  availableVersion: string;
  statusError?: string;
  /**
   * Liveness, with the same meaning the managed providers give it:
   * `bridgeAttached` means Claude has started QC Remote's MCP process at least
   * once, `connectionActive` means it is talking to it right now. Both are
   * optional because a build of the native layer that predates them sends
   * neither - and an absent field is "not observed", never "connected".
   */
  connectionActive?: boolean;
  bridgeAttached?: boolean;
  /** @see {@link LocalLiveness.connectionPending} */
  connectionPending?: boolean;
  state: ClaudeDesktopExtensionState;
}

/**
 * The third liveness value, added because "no heartbeat" meant two things.
 *
 * QC Remote's record of which hosts have announced themselves is per-process:
 * a heartbeat is a claim about now, so remembering one across a restart would
 * outlive the bridge that made it. That leaves a window right after the app
 * starts in which a perfectly healthy bridge has not ticked yet, and the panel
 * used to render that window as "not connected" - or, for ChatGPT, as a
 * restart the user did not need. `connectionPending` names it: still not
 * connected, and not evidence of absence either. A row in this state says it
 * is checking. It never says Connected: only `connectionActive`, which is a
 * heartbeat from that exact client id, does that.
 *
 * Optional throughout for the same reason the other two are - a native layer
 * that predates it sends nothing, and the honest state is the fallback.
 */
export interface LocalLiveness {
  connectionActive?: boolean;
  bridgeAttached?: boolean;
  connectionPending?: boolean;
}

export interface ClaudeDesktopInstallResult {
  launched: boolean;
  availableVersion: string;
}

/**
 * What the app's own start-up pass decided about this provider's route.
 *
 * Every release stamps its version into the client id a managed route names
 * itself by, so shipping a build makes every paired route one version stale.
 * QC Remote freshens its own rather than leaving a row saying "Update
 * available" until somebody presses it, and reports here what it did:
 *
 * - `rewritten` - it was this app's own route (its managed identity and this
 *   app's executable name), either an older build or different from what this
 *   build writes, for instance another executable path. The whole entry was
 *   replaced, so added arguments or environment keys are gone and a disabled
 *   route is enabled again; a backup was taken and other servers were kept.
 *   The host picks the new entry up on its own schedule.
 * - `current` - nothing to do.
 * - `not-paired` - this host carries no QC Remote route.
 * - `customised` - the entry has no valid managed identity or launches a
 *   different executable, so ownership could not be established and it was
 *   left exactly as it was.
 * - `paired-with-newer-build` - the route names a newer app than the one
 *   running, so only the explicit Update control may relabel it.
 * - `unreadable` - the pass could not parse the host's configuration, so it
 *   never learned whose the route is or what version it names. The panel does
 *   not render this one: the identical read failing is already what makes the
 *   row say the status could not be read, in red, offering Refresh - which is
 *   the remedy an unreadable file actually has.
 * - `failed` - the rewrite was refused and `error` says why. The previous
 *   route is intact and still working. This is the only outcome the panel
 *   renders: the app tried to do the user's work for him and could not, so
 *   the Update he no longer has to press is owed to him again.
 */
export interface LocalRouteRefresh {
  outcome: "rewritten" | "current" | "not-paired" | "customised" | "unreadable" | "paired-with-newer-build" | "failed";
  pairedVersion?: string | null;
  availableVersion: string;
  error?: string | null;
}

export interface ChatGptLocalIntegrationStatus {
  /** The sandbox awaits an explicit host-folder selection; polling never prompts. */
  folderAccessRequired?: boolean;
  chatgptInstalled: boolean;
  /** Native host shown by this row on the current OS (Linux Codex CLI, otherwise ChatGPT). */
  clientName?: string;
  codexCliInstalled?: boolean;
  appVersion?: string;
  integrationInstalled: boolean;
  installedVersion?: string;
  availableVersion: string;
  restartRequired: boolean;
  chatgptRunning: boolean;
  connectionActive: boolean;
  /** Optional for the same reason as Claude's: absent means "not observed". */
  bridgeAttached?: boolean;
  /** @see {@link LocalLiveness.connectionPending} */
  connectionPending?: boolean;
  approvalModeCurrent: boolean;
  skillInstalled: boolean;
  installedSkillVersion?: string;
  availableSkillVersion: string;
  skillState: SkillState;
  skillStatusError?: string;
  state: ChatGptLocalIntegrationState;
  statusError?: string;
  /** @see {@link LocalRouteRefresh} */
  routeRefresh?: LocalRouteRefresh | null;
}

export interface ChatGptLocalInstallResult {
  installed: boolean;
  availableVersion: string;
  restartRequired: boolean;
  skillInstalled: boolean;
  skillVersion: string;
  skillBackupPath?: string;
  /** @see {@link ManagedJsonMcpInstallResult.skillError} */
  skillError?: string;
}

export interface ChatGptLocalRemoveResult {
  removed: boolean;
  restartRequired: boolean;
  /** Unpair takes out the QC Device Knowledge skill this app installed. */
  skillRemoved?: boolean;
  /** The user's own copy put back from the backup taken at pairing, if any. */
  skillRestoredPath?: string | null;
}

export interface AntigravityLocalIntegrationStatus {
  /** The sandbox awaits an explicit host-folder selection; polling never prompts. */
  folderAccessRequired?: boolean;
  antigravityInstalled: boolean;
  appVersion?: string;
  integrationInstalled: boolean;
  installedVersion?: string;
  availableVersion: string;
  state: AntigravityLocalIntegrationState;
  statusError?: string;
  connectionActive: boolean;
  bridgeAttached: boolean;
  /** @see {@link LocalLiveness.connectionPending} */
  connectionPending?: boolean;
  skillInstalled: boolean;
  installedSkillVersion?: string;
  availableSkillVersion: string;
  skillState: SkillState;
  skillStatusError?: string;
  /** @see {@link LocalRouteRefresh} */
  routeRefresh?: LocalRouteRefresh | null;
}

export interface AntigravityLocalInstallResult {
  installed: boolean;
  launched: boolean;
  availableVersion: string;
  refreshAttempted: boolean;
  refreshSucceeded: boolean;
  connectionActive: boolean;
  refreshError?: string;
  skillInstalled: boolean;
  skillVersion: string;
  skillBackupPath?: string;
  /** @see {@link ManagedJsonMcpInstallResult.skillError} */
  skillError?: string;
}

export interface AntigravityLocalRemoveResult {
  removed: boolean;
}

export interface ManagedJsonMcpIntegrationStatus {
  /** The sandbox awaits an explicit host-folder selection; polling never prompts. */
  folderAccessRequired?: boolean;
  appVersion?: string;
  /**
   * Which of the provider's numbering series `appVersion` belongs to, named
   * after the product that ships it - so LM Studio's row reports `"Bionic"`
   * for the 1.x line and `"LM Studio"` for the 0.3.x one it renamed away from,
   * and Jan's reports `"Jan"` because Jan has renumbered nothing.
   *
   * Null when the installation reports no version, or one this app cannot
   * parse: the native side files such a version under no series rather than
   * guessing at the newest.
   */
  appVersionSeries?: string | null;
  integrationInstalled: boolean;
  installedVersion?: string;
  availableVersion: string;
  /**
   * The QC Device Knowledge skill, for a host on this shared row that has a
   * skills mechanism - which Bionic does and Jan does not.
   *
   * Bionic's own bundled `skill-management` skill documents a global skill as
   * `~/.lmstudio/skills/<name>/SKILL.md`, the same folder-plus-SKILL.md shape
   * this app already installs for Codex and Antigravity, so its row carries
   * the same five fields those do. Jan has no skills concept at all - only one
   * `instructions` string per assistant, with nowhere for the catalog to
   * live - so its native status sets none of these.
   *
   * Optional for that reason, and it is the whole point of them being
   * optional: the presentation is shared, so a host without a skill must be
   * able to say *nothing* rather than report "not installed" about something
   * it could never install. Absent means "this host has no skills", not "the
   * skill is missing".
   *
   * Neither host is short of the knowledge either way. Every paired host can
   * read `qc://device-knowledge/guide` and `qc://device-knowledge/catalog` off
   * the bridge and call `search_qc_device_knowledge`; installing the skill only
   * makes a host that supports skills trigger on it unprompted.
   */
  skillInstalled?: boolean;
  installedSkillVersion?: string;
  availableSkillVersion?: string;
  skillState?: SkillState;
  skillStatusError?: string;
  state: ManagedJsonMcpIntegrationState;
  statusError?: string;
  connectionActive: boolean;
  bridgeAttached: boolean;
  /** @see {@link LocalLiveness.connectionPending} */
  connectionPending?: boolean;
  /** @see {@link LocalRouteRefresh} */
  routeRefresh?: LocalRouteRefresh | null;
}

export interface LmStudioLocalIntegrationStatus extends ManagedJsonMcpIntegrationStatus {
  lmStudioInstalled: boolean;
}

export interface JanLocalIntegrationStatus extends ManagedJsonMcpIntegrationStatus {
  janInstalled: boolean;
}

export interface ManagedJsonMcpInstallResult {
  installed: boolean;
  availableVersion: string;
  /**
   * The skill half of the result, for a host on this shared shape that has a
   * skills mechanism - Bionic does, Jan does not.
   *
   * Optional because Jan's native command sets none of them, which is what
   * keeps `localPairOutcome`'s `skillInstalled === false` branch from firing
   * for a host that could never install one. They were absent from this type
   * entirely, so the Bionic path could not read what its own native command
   * had been returning all along: `installed` reports the MCP route write, so
   * a pair whose skill failed was announced as a completed pair, and the row
   * then asked for an Update that reported success and changed nothing.
   */
  skillInstalled?: boolean;
  skillVersion?: string;
  skillBackupPath?: string;
  /**
   * Why the skill did not install. Composed by the native side for every
   * failure mode, including the ones that retrying cannot fix - a link in the
   * skill folder that has to be moved aside first, or a rollback that failed
   * and names where the user's files still are.
   */
  skillError?: string;
}

export interface ManagedJsonMcpRemoveResult {
  removed: boolean;
}

/**
 * What the app says about its own loopback MCP surface.
 *
 * `servedByAnotherInstance` is typed rather than left to be read out of
 * `lastError`: a second copy of QC Remote taking the published descriptor over
 * is the one start-up failure whose remedy is not "restart QC Remote", and the
 * panel must never have to match against wording to tell them apart.
 */
export interface LocalAccessStatus {
  enabled: boolean;
  running: boolean;
  port?: number;
  instanceId?: string;
  descriptorPaths: string[];
  lastError?: string;
  servedByAnotherInstance: boolean;
}

/** The outcomes of one round trip through the published loopback endpoint. */
export type LocalMcpTestState =
  | "disabled"
  | "notPublished"
  | "notAnswering"
  | "unauthorized"
  | "servedByAnotherCopy"
  | "deviceNotConnected"
  | "ready";

export interface LocalMcpTestResult {
  state: LocalMcpTestState;
  appVersion: string;
  /** The preset the Quad Cortex reports as loaded, when one is loaded. */
  presetName?: string;
  /** The raw diagnosis, for Copy details. Never shown as the message. */
  detail?: string;
}

export interface OtherMcpStdioSetup {
  configuration: string;
  availableVersion: string;
  connectionActive: boolean;
  bridgeAttached: boolean;
  /** @see {@link LocalLiveness.connectionPending} */
  connectionPending?: boolean;
}

export const localAccess = {
  status(): Promise<LocalAccessStatus> {
    return callTauri<LocalAccessStatus>("local_mcp_status");
  },
  /**
   * One read-only round trip through the published endpoint, authenticated
   * with the published bearer token exactly as a paired AI app would be.
   */
  test(): Promise<LocalMcpTestResult> {
    return callTauri<LocalMcpTestResult>("test_local_mcp");
  }
};

export const localClientPackages = {
  claudeDesktopStatus(): Promise<ClaudeDesktopExtensionStatus> {
    return callTauri<ClaudeDesktopExtensionStatus>("claude_desktop_extension_status");
  },
  installClaudeDesktop(): Promise<ClaudeDesktopInstallResult> {
    return callTauri<ClaudeDesktopInstallResult>("install_claude_desktop_extension");
  },
  openClaudeDesktop(): Promise<void> {
    return callTauri<void>("open_claude_desktop");
  },
  openClaudeDesktopDownload(): Promise<void> {
    return callTauri<void>("open_claude_desktop_download");
  },
  chatgptStatus(): Promise<ChatGptLocalIntegrationStatus> {
    return callTauri<ChatGptLocalIntegrationStatus>("chatgpt_local_integration_status");
  },
  installChatgpt(): Promise<ChatGptLocalInstallResult> {
    return callTauri<ChatGptLocalInstallResult>("install_chatgpt_local_integration");
  },
  removeChatgpt(): Promise<ChatGptLocalRemoveResult> {
    return callTauri<ChatGptLocalRemoveResult>("remove_chatgpt_local_integration");
  },
  openChatgpt(): Promise<void> {
    return callTauri<void>("open_chatgpt_desktop");
  },
  openChatgptDownload(): Promise<void> {
    return callTauri<void>("open_chatgpt_desktop_download");
  },
  openChatgptConfiguration(): Promise<void> {
    return callTauri<void>("open_chatgpt_configuration");
  },
  antigravityStatus(): Promise<AntigravityLocalIntegrationStatus> {
    return callTauri<AntigravityLocalIntegrationStatus>("antigravity_local_integration_status");
  },
  openAntigravityDownload(): Promise<void> {
    return callTauri<void>("open_antigravity_download");
  },
  installAntigravity(): Promise<AntigravityLocalInstallResult> {
    return callTauri<AntigravityLocalInstallResult>("install_antigravity_local_integration");
  },
  removeAntigravity(): Promise<AntigravityLocalRemoveResult> {
    return callTauri<AntigravityLocalRemoveResult>("remove_antigravity_local_integration");
  },
  openAntigravity(): Promise<void> {
    return callTauri<void>("open_antigravity");
  },
  lmStudioStatus(): Promise<LmStudioLocalIntegrationStatus> {
    return callTauri<LmStudioLocalIntegrationStatus>("lm_studio_local_integration_status");
  },
  installLmStudio(): Promise<ManagedJsonMcpInstallResult> {
    return callTauri<ManagedJsonMcpInstallResult>("install_lm_studio_local_integration");
  },
  removeLmStudio(): Promise<ManagedJsonMcpRemoveResult> {
    return callTauri<ManagedJsonMcpRemoveResult>("remove_lm_studio_local_integration");
  },
  openLmStudio(): Promise<void> {
    return callTauri<void>("open_lm_studio");
  },
  openLmStudioDownload(): Promise<void> {
    return callTauri<void>("open_lm_studio_download");
  },
  janStatus(): Promise<JanLocalIntegrationStatus> {
    return callTauri<JanLocalIntegrationStatus>("jan_local_integration_status");
  },
  installJan(): Promise<ManagedJsonMcpInstallResult> {
    return callTauri<ManagedJsonMcpInstallResult>("install_jan_local_integration");
  },
  removeJan(): Promise<ManagedJsonMcpRemoveResult> {
    return callTauri<ManagedJsonMcpRemoveResult>("remove_jan_local_integration");
  },
  openJan(): Promise<void> {
    return callTauri<void>("open_jan");
  },
  openJanDownload(): Promise<void> {
    return callTauri<void>("open_jan_download");
  },
  otherMcpStdioSetup(): Promise<OtherMcpStdioSetup> {
    return callTauri<OtherMcpStdioSetup>("other_mcp_stdio_setup");
  },
  saveOtherClients(): Promise<LocalClientPackageResult> {
    return callTauri<LocalClientPackageResult>("save_local_client_package");
  }
};
