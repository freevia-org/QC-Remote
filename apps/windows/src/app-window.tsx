/**
 * The page an app window renders: Settings, Device Info, or Workspace.
 *
 * Each is a native window of its own (`open_app_window` in the Rust shell),
 * loaded as `index.html?window=<kind>` like the Help windows, and set in the
 * same reading-size type (`help-windows.css`).
 *
 * This webview shares no React state with the main window, so every value it
 * shows is sent from there over the event bus (`subscribeWindowState`): Device
 * Info and Settings follow the main window's live state, and Workspace gets the
 * opened workspace. Settings' controls do not apply anything here; they
 * send the chosen value to the main window, which applies and remembers it as
 * its in-app controls always did, and the new value comes back in the next
 * `settings-state`.
 *
 * A Settings change and Save Copy As… are requests the main window
 * acknowledges and then answers, exactly like the Feedback window's
 * diagnostics requests. Their outcome is shown here, where the button was
 * pressed; a main window that never acknowledges is reported as not
 * responding rather than left to look like a success.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { QC_BRAND } from "@qc-remote/theme";
import { ControlAccessSelect, QcDeviceInformationDisplay } from "@qc-remote/ui";
import { idleDisconnectHelp, idleMinutesFromInput, UPDATER_RELAY_EXPLANATION, UPDATER_RELAY_LABEL, updaterRelayStatusLine, type AppWindowKind, type DeviceInfoState, type SettingsSetting, type SettingsState, type WorkspaceState } from "./app-window-content";
import { DEVICE_INFO_EVENTS, requestSettingsChange, requestWorkspaceSaveCopy, SETTINGS_EVENTS, subscribeWindowState, WINDOW_STATE_NO_ANSWER, WORKSPACE_EVENTS } from "./tauri-transport";
import "./help-windows.css";

export type { AppWindowKind } from "./app-window-content";

const WAITING = "Waiting for the main window…";

const errorText = (error: unknown): string => error instanceof Error ? error.message : String(error);

/**
 * Follow one kind of main-window state for the life of this window. The flag
 * is set when the main window never answered, so the page can say so rather
 * than keep waiting.
 */
function useWindowState<State>(events: { stateRequest: string; state: string }): [State | undefined, boolean] {
  const [state, setState] = useState<State>();
  const [unanswered, setUnanswered] = useState(false);
  useEffect(() => subscribeWindowState<State>(events, setState, () => setUnanswered(true)), [events]);
  return [state, unanswered];
}

/** The line an app window shows until its state arrives. */
const waitingLine = (unanswered: boolean) => unanswered ? WINDOW_STATE_NO_ANSWER : WAITING;

/**
 * The Device Information screen is the shared display from @qc-remote/ui, the
 * same frame, canvas and screen the main window draws, so it cannot drift from
 * it, with the same data: what the unit reports about itself, row by row (a row
 * the unit did not report says so). Done closes this native window, as it
 * closes Settings on the unit. The live fields the device screen has no rows
 * for sit in the accessible `<dl>` below it.
 */
function DeviceInfoScreen({ device, onClose }: { device: DeviceInfoState; onClose(): void }) {
  return <QcDeviceInformationDisplay deviceInformation={device.information ?? { deviceName: device.deviceName }} onClose={onClose} />;
}

function DeviceInfoContent() {
  const [device, unanswered] = useWindowState<DeviceInfoState>(DEVICE_INFO_EVENTS);
  const [closeError, setCloseError] = useState<string>();
  const close = useCallback(() => {
    void import("@tauri-apps/api/window")
      .then(({ getCurrentWindow }) => getCurrentWindow().close())
      .catch((error: unknown) => setCloseError(errorText(error)));
  }, []);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      close();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [close]);
  return <>
    <header className="help-window-header">
      <div className="dialog-kicker">CURRENT DEVICE</div>
      <h2 id="help-window-title">{device?.deviceName ?? "Quad Cortex"}</h2>
    </header>
    <div className="help-window-scroll">
      {device ? <>
        <DeviceInfoScreen device={device} onClose={close} />
        <dl className="app-window-facts">
          <dt>Connection</dt><dd>{device.connection}</dd>
          <dt>Setlist</dt><dd>{device.setlistName}</dd>
          <dt>Preset</dt><dd>{device.presetLocation} · {device.presetName}</dd>
          <dt>Mode</dt><dd>{device.mode}</dd>
          <dt>Scene</dt><dd>{device.scene}</dd>
          <dt>Tempo</dt><dd>{device.tempo} BPM</dd>
          <dt>Grid</dt><dd>{device.blockCount} blocks</dd>
          <dt>State</dt><dd>{device.dirty ? "Unsaved device changes" : "Clean"}</dd>
        </dl>
      </> : <p role="status">{waitingLine(unanswered)}</p>}
      <p>The values on the screen are the ones the Quad Cortex reports about itself. Cloud account identifiers are not read or displayed, and none of this is included in diagnostics.</p>
      <p>Software and firmware versions are shown only when the connected unit reports them; unreported fields are identified on the screen.</p>
      {closeError && <p className="about-message" role="status">Could not close this window: {closeError}</p>}
    </div>
  </>;
}

function WorkspaceContent() {
  const [workspace, unanswered] = useWindowState<WorkspaceState>(WORKSPACE_EVENTS);
  const [message, setMessage] = useState<string>();
  // Stays set until the main window answers, so a second press cannot open a
  // second save dialog behind the first.
  const [saving, setSaving] = useState(false);
  const mounted = useRef(true);
  const mountGeneration = useRef(0);
  const saveFlight = useRef<object | undefined>(undefined);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; mountGeneration.current += 1; };
  }, []);
  const saveCopy = () => {
    if (!mounted.current || saveFlight.current || !workspace) return;
    const token = {};
    saveFlight.current = token;
    const generation = mountGeneration.current;
    const current = () => mounted.current && mountGeneration.current === generation;
    setSaving(true);
    setMessage("Waiting for the save dialog…");
    requestWorkspaceSaveCopy(workspace.documentId)
      .then((line) => { if (current()) setMessage(line); }, (error: unknown) => { if (current()) setMessage(errorText(error)); })
      .finally(() => {
        if (saveFlight.current !== token) return;
        saveFlight.current = undefined;
        if (mounted.current) setSaving(false);
      });
  };
  return <>
    <header className="help-window-header">
      <div className="dialog-kicker">LOCAL WORKSPACE</div>
      <h2 id="help-window-title">{workspace?.name ?? "QC Workspace"}</h2>
    </header>
    <div className="help-window-scroll">
      {workspace === undefined && <p role="status">{waitingLine(unanswered)}</p>}
      {workspace === null && <p role="status">No workspace is open. Use Preset &gt; Open Preset… in the main window to open one.</p>}
      {workspace && <>
        <dl className="app-window-facts">
          <dt>Saved</dt><dd>{new Date(workspace.savedAt).toLocaleString()}</dd>
          <dt>Source</dt><dd>{workspace.setlistName} · {workspace.presetLocation}</dd>
          <dt>Preset</dt><dd>{workspace.presetName}</dd>
          <dt>Scene</dt><dd>{workspace.scene}</dd>
          <dt>Blocks</dt><dd>{workspace.blockCount}</dd>
          <dt>Device state</dt><dd>{workspace.dirty ? "Captured with unsaved changes" : "Clean at capture"}</dd>
        </dl>
        <p>The workspace is a local reference snapshot. Opening it never writes to the connected Quad Cortex.</p>
        <div className="dialog-actions">
          <button type="button" className="primary" disabled={saving} aria-busy={saving} onClick={saveCopy}>Save Copy As…</button>
        </div>
      </>}
      {message && <p className="about-message" role="status">{message}</p>}
    </div>
  </>;
}

function SettingsContent() {
  const isMacOS = typeof navigator !== "undefined" && /Mac/.test(navigator.platform);
  const [settings, unanswered] = useWindowState<SettingsState>(SETTINGS_EVENTS);
  const [message, setMessage] = useState<string>();
  // The change this window asked for. While the main window has not answered,
  // its control keeps showing the chosen value (disabled) instead of snapping
  // back to the last value the main window sent; once answered, it keeps
  // showing it until the next settings-state carries what was applied. A
  // refusal or no answer at all drops it, with the reason in the message line.
  const [pending, setPending] = useState<{ setting: SettingsSetting; value: unknown; answered: boolean }>();
  const mounted = useRef(true);
  const mountGeneration = useRef(0);
  const changeFlight = useRef<object | undefined>(undefined);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; mountGeneration.current += 1; };
  }, []);
  const [idleDraft, setIdleDraft] = useState("15");
  const lastIdleMinutes = useRef(15);
  useEffect(() => {
    if (settings?.idleDisconnectMinutes === null) {
      setIdleDraft(String(lastIdleMinutes.current));
    } else if (typeof settings?.idleDisconnectMinutes === "number") {
      lastIdleMinutes.current = settings.idleDisconnectMinutes;
      setIdleDraft(String(settings.idleDisconnectMinutes));
    }
  }, [settings?.idleDisconnectMinutes]);
  useEffect(() => { setPending((current) => current?.answered ? undefined : current); }, [settings]);
  const waiting = pending !== undefined && !pending.answered;
  const change = (setting: SettingsSetting, value: unknown) => {
    if (!mounted.current || changeFlight.current) return;
    const token = {};
    changeFlight.current = token;
    const generation = mountGeneration.current;
    const current = () => mounted.current && mountGeneration.current === generation && changeFlight.current === token;
    setPending({ setting, value, answered: false });
    setMessage(undefined);
    requestSettingsChange(setting, value).then(
      (line) => { if (current()) { setMessage(line); setPending((pendingValue) => pendingValue && { ...pendingValue, answered: true }); } },
      (error: unknown) => { if (current()) { setMessage(errorText(error)); setPending(undefined); } }
    ).finally(() => {
      // The native operation already admitted stays owned until its real
      // response, even if the requesting window has since closed.
      if (changeFlight.current !== token) return;
      changeFlight.current = undefined;
      if (mounted.current && mountGeneration.current !== generation) setPending(undefined);
    });
  };
  const shown = <T,>(setting: SettingsSetting, current: T): T => pending?.setting === setting ? pending.value as T : current;
  const idleMinutes = shown("idle-disconnect-minutes", settings?.idleDisconnectMinutes ?? null);
  const idleEnabled = idleMinutes !== null;
  const changeIdleEnabled = (enabled: boolean) => {
    setIdleDraft(String(lastIdleMinutes.current));
    change("idle-disconnect-minutes", enabled ? lastIdleMinutes.current : null);
  };
  const submitIdleMinutes = (input: HTMLInputElement) => {
    if (idleDraft !== String(settings?.idleDisconnectMinutes)) {
      change("idle-disconnect-minutes", idleMinutesFromInput(input.value, input.valueAsNumber));
    }
  };
  const trayEnabled = shown("tray-enabled", settings?.trayEnabled ?? false);
  const trayStopping = settings?.trayStopping ?? false;
  const trayRunning = settings?.trayRunning ?? false;
  const trayDescription = isMacOS
    ? trayStopping
      ? "Hiding the menu bar item while the background broker stops…"
      : trayEnabled
        ? trayRunning
          ? "Connect and Disconnect are available from the menu bar. The broker and MCP access stay available when QC Remote is closed."
          : "The menu bar item and login startup are enabled, but the broker is not running yet."
        : "The menu bar item, background broker, and login startup are off."
    : trayStopping
      ? "Hiding the tray icon…"
      : trayEnabled
        ? trayRunning
          ? "The broker shows its icon whenever it holds the Quad Cortex, and MCP access stays available when QC Remote is closed."
          : "The broker icon is on; the background MCP host is not running."
        : "The tray icon and its background MCP host are off.";
  // The main window has focus only when it is clicked, so this window, which
  // has it now, is the one that can write the clipboard.
  const copyDetail = (detail: string) => {
    navigator.clipboard.writeText(detail)
      .then(() => setMessage("The technical detail was copied to the clipboard."))
      .catch(() => setMessage(detail));
  };
  const condition = settings?.localAccessCondition;
  return <>
    <header className="help-window-header">
      <div className="dialog-kicker">SETTINGS</div>
      <h2 id="help-window-title">{QC_BRAND.appName} settings</h2>
    </header>
    <div className="help-window-scroll">
      {!settings ? <p role="status">{waitingLine(unanswered)}</p> : <section className="settings-panel">
        <label className="setting-row borderless"><span>Device model<small>Hardware geometry, controls, and appearance</small></span><select value={shown("form-factor", settings.formFactorId)} disabled={waiting} aria-busy={waiting && pending?.setting === "form-factor"} onChange={(event) => change("form-factor", event.target.value)}>{settings.formFactors.map((item) => <option value={item.id} key={item.id}>{item.displayName}</option>)}</select></label>
        <label className="setting-row"><span>AI access level<small>Read-only inspects; Performance permits buttons, volume, and tempo; Modify adds Grid, presets, and scenes; Full also permits system operations. Confirmation gates still apply.</small></span><ControlAccessSelect value={shown("control-access-mode", settings.controlAccessMode) as SettingsState["controlAccessMode"]} disabled={waiting} onChange={(mode) => change("control-access-mode", mode)} /></label>
        <label className="setting-row privacy-toggle"><span>Follow QC screen<small>When enabled, QC Remote checks the device screen adaptively while this app is visible. It pauses while hidden or disconnected.</small></span><input type="checkbox" checked={shown("follow-qc-screen", settings.followQcScreen)} disabled={waiting} aria-busy={waiting && pending?.setting === "follow-qc-screen"} onChange={(event) => change("follow-qc-screen", event.target.checked)} /></label>
        <label className="setting-row privacy-toggle"><span>{isMacOS ? "Show menu bar item and run broker in the background" : "Show tray icon"}<small>{trayDescription}</small>{isMacOS && <small>For background login startup, keep QC Remote.app directly in /Applications or ~/Applications. Foreground use works elsewhere. Before moving or removing it, turn this off and wait for the broker to stop.</small>}</span><input type="checkbox" checked={trayEnabled} disabled={waiting} aria-busy={waiting && pending?.setting === "tray-enabled"} onChange={(event) => change("tray-enabled", event.target.checked)} /></label>
        <label className="setting-row privacy-toggle"><span>{UPDATER_RELAY_LABEL}<small>{UPDATER_RELAY_EXPLANATION}</small><small role="status">{updaterRelayStatusLine(shown("updater-relay-enabled", settings.updaterRelayEnabled), settings.updaterRelay)}</small></span><input type="checkbox" checked={shown("updater-relay-enabled", settings.updaterRelayEnabled)} disabled={waiting} aria-busy={waiting && pending?.setting === "updater-relay-enabled"} onChange={(event) => change("updater-relay-enabled", event.target.checked)} /></label>
        <div className="setting-row privacy-toggle"><span><label htmlFor="idle-disconnect-enabled">Release QC after last client</label><small>{idleDisconnectHelp(settings.idleDisconnectMinutes, settings.trayEnabled)}</small></span><div className="idle-disconnect-controls">
          <input id="idle-disconnect-enabled" type="checkbox" checked={idleEnabled} disabled={waiting} aria-label="Release the QC after the last client disconnects" aria-busy={waiting && pending?.setting === "idle-disconnect-minutes"} onChange={(event) => changeIdleEnabled(event.target.checked)} />
          <input type="number" min="0" max="1440" step="1" aria-label="Minutes after the last client disconnects (0 means Immediately)" value={idleDraft} disabled={waiting || !idleEnabled} onChange={(event) => setIdleDraft(event.target.value)} onBlur={(event) => { if (event.relatedTarget?.id !== "idle-disconnect-enabled") submitIdleMinutes(event.currentTarget); }} onKeyDown={(event) => { if (event.key === "Enter") event.currentTarget.blur(); }} />
          <span>minutes</span>
        </div></div>
        {settings.lastError && <p className="about-message" role="status">{settings.lastError}</p>}
        {condition && <div className={`local-access-notice tone-${condition.tone}`} role="status">
          <span>{condition.message}</span>
          {condition.detail && <button type="button" className="local-outcome-detail" onClick={() => copyDetail(condition.detail as string)}>Copy details</button>}
        </div>}
      </section>}
      {message && <p className="about-message" role="status">{message}</p>}
    </div>
  </>;
}

export function AppWindow({ kind }: { kind: AppWindowKind }) {
  return <main className={`help-window app-window app-window-${kind}`} aria-labelledby="help-window-title">
    {kind === "settings" && <SettingsContent />}
    {kind === "device-info" && <DeviceInfoContent />}
    {kind === "workspace" && <WorkspaceContent />}
  </main>;
}
