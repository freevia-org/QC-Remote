import { closeUserOpenedTarget, escapeUserOpenedTarget } from "./user-open-focus";
import { useRef, useState, type ReactNode } from "react";
import { CorOsReferenceSettingsPane, type ReferenceSettingsView } from "./coros-reference-settings-pane";
import { QcDeviceCategoryGlyph } from "./device-category-glyph";
import { QcBrightnessInstructions, QcBrightnessModeRow, QcSegmentBarHit } from "./qc-brightness-panel";
import { QcPillToggle } from "./qc-pill-toggle";
import { QcPillToggleLabels } from "./qc-shared-controls";
import { QcRadioButton } from "./qc-radio-button";
import { QcSegmentBar } from "./qc-segment-bar";
import { QcScreenGlyph, QcSettingsIcon, QcUiIcon, type QcScreenGlyphName, type QcSettingsIconName } from "./theme-icons";
import type { DeviceSettingsAccountRow, DeviceSettingsSection, DeviceSettingsDeviceRow, DeviceSettingsSupportRow, DeviceSettingsSystemRow, DeviceSettingsWorkflow, MidiClockOutValue } from "./use-device-settings-workflow";
import { DEVICE_SETTINGS_ACCOUNT_ROWS, DEVICE_SETTINGS_DEVICE_ROWS, DEVICE_SETTINGS_SUPPORT_ROWS, DEVICE_SETTINGS_SYSTEM_ROWS, HOLD_TIMING_STEPS_MS, MIDI_CLOCK_OUT_VALUES } from "./use-device-settings-workflow";
import { BRIGHTNESS_BODY_LINES, DEVICE_SETTINGS_COPY, GLOBAL_BYPASS_BODY_LINES, GLOBAL_BYPASS_FOOTNOTE_LINES, HOLD_TIMING_BODY, LATENCY_COMPENSATION_BODY, MASTER_VOLUME_LEAD, SCENE_BYPASS_BODY_LINES, SCENE_BYPASS_OPTION_TEXT, STOMP_MODE_BYPASS_BODY, SWAP_TEMPO_TUNER_BODY } from "./device-reference/device-settings-text";
import "./official-settings.css";

export interface DeviceSettingsScreenProps {
  workflow: DeviceSettingsWorkflow;
  onClose(): void;
}

const ACCOUNT_ROW_LABELS: Record<DeviceSettingsAccountRow, string> = { user: "My Account", backup: "Backups" };
const SYSTEM_ROW_LABELS: Record<DeviceSettingsSystemRow, string> = {
  wifi: "Connection", updates: "Updates", brightness: "Brightness", power: "Power Functions",
  volume: "Master Volume Knob", storage: "Device Storage", factory: "Factory Reset"
};
const DEVICE_ROW_LABELS: Record<DeviceSettingsDeviceRow, string> = {
  bypass: "Global Bypass", "scene-bypass": "Scene Bypass Behavior", "stomp-bypass": "Stomp Mode Bypass",
  hold: "Hold Timing", swap: "Swap Tempo and Tuner", "gig-access": "Gig View Access",
  latency: "Latency Compensation", midi: "MIDI"
};
const SUPPORT_ROW_LABELS: Record<DeviceSettingsSupportRow, string> = {
  about: "About and Contact", info: "Device Information", report: "Send Report",
  diagnostics: "Diagnostics", licenses: "3rd Party Licenses"
};
const SUPPORT_ROW_GLYPHS: Record<DeviceSettingsSupportRow, QcScreenGlyphName> = {
  about: "about", info: "info", report: "report", diagnostics: "diagnostics", licenses: "licenses"
};
/** SETTINGS_DEVICE_ROWS / REFERENCE_SETTINGS_ROW_GLYPHS in coros-screen-
 * fixtures.tsx - the same registry name each Device row's mark traces from. */
const DEVICE_ROW_GLYPHS: Record<DeviceSettingsDeviceRow, QcScreenGlyphName> = {
  bypass: "global-bypass", "scene-bypass": "scene-bypass", "stomp-bypass": "stomp-bypass",
  hold: "hold", swap: "swap", "gig-access": "gig-view", latency: "latency", midi: "device-midi"
};
const SYSTEM_ROW_ICONS: Record<DeviceSettingsSystemRow, QcSettingsIconName> = {
  wifi: "connection", updates: "updates", brightness: "brightness", power: "power",
  volume: "volume", storage: "storage", factory: "factory-reset"
};

/** Rows with no backend call anywhere in qc-client's typed gateway. Shown and
 * disabled rather than removed, per the convention of never deleting a row
 * the device actually has - the same one coros-ui.ts's own
 * corOsUnavailableContextActionMessage established for a whole Grid action. */
function unavailableMessage(label: string): string {
  return `${label} is shown in the device's own Settings screen, but this setting is not exposed by the current USB gateway.`;
}

function UnavailableDetail({ label }: { label: string }) {
  return <>
    <h1>{label}</h1>
    <div className="settings-unavailable-detail"><p>{unavailableMessage(label)}</p></div>
  </>;
}

function fraction(current?: number, total?: number): string {
  if (current === undefined || total === undefined || total <= 0) return "—";
  return `${Math.round((current / total) * 100)}%`;
}

/** A single Device row's two-option pane (Stomp Mode Bypass/Swap Tempo and
 * Tuner/Gig View Access/Latency Compensation) - one QcPillToggle and its two
 * captions, ported verbatim from CorOsReferenceSettings' own per-view table
 * (coros-screen-fixtures.tsx). Real backing per row is a single boolean, so
 * `activeWhenTrue` says which of the two options (index 0) means `true`. */
function TwoOptionDetail(props: { title: string; body: readonly string[]; label?: string; options: readonly [string, string]; value: boolean | undefined; activeWhenTrue: 0 | 1; pending: boolean; onChange(next: boolean): void }) {
  const { title, body, label, options, value, activeWhenTrue, pending, onChange } = props;
  const boolValue = value ?? false;
  const active = boolValue ? activeWhenTrue : (activeWhenTrue === 0 ? 1 : 0);
  const toggle = () => { if (!pending) onChange(!boolValue); };
  return <>
    <h1>{title}</h1>
    {body.map((text, index) => <p key={index} className={`settings-body is-${index}`}>{text}</p>)}
    {label && <span className="settings-field-label">{label}</span>}
    <button type="button" className={`settings-toggle${pending ? " is-pending" : ""}`} disabled={pending} aria-pressed={active === 0} onClick={toggle}>
      <QcPillToggle index={active} />
      {options.map((option, index) => <span key={option} className={index === active ? "is-active" : ""}>{option}</span>)}
    </button>
  </>;
}

const DEVICE_COPY = DEVICE_SETTINGS_COPY;

function DeviceDetail({ row, workflow }: { row: DeviceSettingsDeviceRow; workflow: DeviceSettingsWorkflow }) {
  const { settings, pending } = workflow;
  if (row === "bypass") {
    const categories = [["cab", "Cab"], ["ir", "IR Loader"]] as const;
    return <>
      <h1>Global Bypass</h1>
      <p>
        {GLOBAL_BYPASS_BODY_LINES[0]}
        <br />
        {GLOBAL_BYPASS_BODY_LINES[1]}<br />
        {GLOBAL_BYPASS_BODY_LINES[2]}
      </p>
      <small>
        {GLOBAL_BYPASS_FOOTNOTE_LINES[0]}
        <br />
        {GLOBAL_BYPASS_FOOTNOTE_LINES[1]}
      </small>
      {categories.map(([key, categoryLabel]) => {
        const rows = key === "cab" ? settings?.globalBypassCab : settings?.globalBypassIr;
        return <div key={key}>
          <b><QcDeviceCategoryGlyph label={categoryLabel} /></b>
          {([1, 2, 3, 4] as const).map((rowNumber) => {
            const field = (`row${rowNumber}` as const);
            const bypassed = Boolean(rows?.[field]);
            return <button type="button" key={rowNumber} className={pending ? "is-pending" : undefined} disabled={pending} aria-pressed={bypassed} onClick={() => void workflow.setGlobalBypass(key, rowNumber, !bypassed)}>
              <span>ROW {rowNumber}</span>
              <i className={bypassed ? "is-bypassed" : undefined}><QcUiIcon kind="power-toggle" /></i>
            </button>;
          })}
        </div>;
      })}
    </>;
  }
  if (row === "scene-bypass") {
    const behaviors = ["alwaysOverwrite", "nonstompOverwrite", "neverOverwrite"] as const;
    const options: Array<[string, typeof behaviors[number]]> = [
      [SCENE_BYPASS_OPTION_TEXT.alwaysOverwrite, "alwaysOverwrite"],
      [SCENE_BYPASS_OPTION_TEXT.nonstompOverwrite, "nonstompOverwrite"],
      [DEVICE_COPY.sceneBypassThird, "neverOverwrite"]
    ];
    return <>
      <h1>Scene Bypass State Behavior</h1>
      <p className="settings-body is-0">
        {SCENE_BYPASS_BODY_LINES[0]}
        <br />
        {SCENE_BYPASS_BODY_LINES[1]}
        <br />
        {SCENE_BYPASS_BODY_LINES[2]}
      </p>
      <div className="scene-bypass-options">
        {options.map(([text, behavior], index) => {
          const selected = settings?.sceneBypassBehavior === behavior;
          return <button type="button" key={index} role="radio" aria-checked={selected} disabled={pending} onClick={() => void workflow.setSceneBypassBehavior(behavior)}>
            <p>{text}</p>
            <QcRadioButton selected={selected} plate />
          </button>;
        })}
      </div>
    </>;
  }
  if (row === "stomp-bypass") return <TwoOptionDetail
    title="Stomp Mode Bypass Assignment"
    body={[STOMP_MODE_BYPASS_BODY]}
    label="AUTO-ASSIGN" options={["Enabled", "Disabled (Factory Default)"]} value={settings?.stompModeAutoAssign} activeWhenTrue={0} pending={pending}
    onChange={(next) => void workflow.setStompModeAutoAssign(next)} />;
  if (row === "swap") return <TwoOptionDetail
    title="Swap Tempo and Tuner Access"
    body={[SWAP_TEMPO_TUNER_BODY]}
    label="SWAP TEMPO AND TUNER" options={["Yes", "No (Factory Default)"]} value={settings?.swapTempoTunerAccess} activeWhenTrue={0} pending={pending}
    onChange={(next) => void workflow.setSwapTempoTunerAccess(next)} />;
  if (row === "gig-access") return <TwoOptionDetail
    title="Gig View Footswitch Access"
    body={DEVICE_COPY.gigView.split("\n\n")}
    options={["On", "Off (Factory Default)"]} value={settings?.gigViewStompAccess} activeWhenTrue={0} pending={pending}
    onChange={(next) => void workflow.setGigViewStompAccess(next)} />;
  if (row === "latency") return <TwoOptionDetail
    title="Dynamic Latency Compensation"
    body={[LATENCY_COMPENSATION_BODY]}
    options={["Enabled (Factory Default)", "Disabled"]} value={settings?.dynamicDelayCompensation} activeWhenTrue={0} pending={pending}
    onChange={(next) => void workflow.setDynamicDelayCompensation(next)} />;
  if (row === "hold") {
    const index = settings?.holdTimingIndex ?? HOLD_TIMING_STEPS_MS.findIndex((ms) => ms === settings?.holdTimingMs);
    const active = index >= 0 ? index : 3;
    return <>
      <h1>Hold Timing</h1>
      <p className="settings-body is-0">{HOLD_TIMING_BODY}</p>
      <div className="hold-scale">
        {HOLD_TIMING_STEPS_MS.map((ms, stepIndex) => <button type="button" key={ms} className={stepIndex === active ? "is-active" : undefined} disabled={pending} aria-pressed={stepIndex === active} onClick={() => void workflow.setHoldTimingIndex(stepIndex)}>{ms}ms</button>)}
      </div>
      <QcSegmentBar className="hold-bar" segments={6} filled={active + 1} label="Hold timing" />
    </>;
  }
  // row === "midi"
  const midiChannel = settings?.midiChannel ?? 1;
  return <div className="settings-midi-detail">
    <h1>MIDI Settings</h1>
    <div>
      <b>i</b><span>MIDI Channel</span>
      <select value={midiChannel} disabled={pending} onChange={(event) => void workflow.setMidiChannel(Number(event.target.value))}>
        {Array.from({ length: 17 }, (_, index) => index + 1).map((channel) => <option key={channel} value={channel}>{channel === 17 ? "OMNI" : channel}</option>)}
      </select>
    </div>
    <div>
      <b>i</b><span>MIDI Thru</span>
      <button type="button" className="settings-midi-toggle" disabled={pending} aria-pressed={workflow.midiThru} onClick={() => void workflow.setMidiThru(!workflow.midiThru)}>
        {/* generalSettings() reports no readable `midiThru` field at all - the
            pill shows this hook's own last-written intent
            (use-device-settings-workflow.ts's `midiThru`), not an
            authoritative device readback, until the first write here. */}
        <QcPillToggle index={workflow.midiThru ? 0 : 1} />
        <QcPillToggleLabels labels={["On", "Off"]} index={workflow.midiThru ? 0 : 1} />
      </button>
    </div>
    {([
      ["MIDI Over USB", settings?.midiOverUsb, (next: boolean) => workflow.setMidiOverUsb(next)],
      ["Ignore Duplicate PC", settings?.ignoreDuplicatePc, (next: boolean) => workflow.setIgnoreDuplicatePc(next)]
    ] as const).map(([label, value, onChange]) => <div key={label}>
      <b>i</b><span>{label}</span>
      <button type="button" className="settings-midi-toggle" disabled={pending} aria-pressed={Boolean(value)} onClick={() => void onChange(!value)}>
        <QcPillToggle index={value ? 0 : 1} />
        <QcPillToggleLabels labels={["On", "Off"]} index={value ? 0 : 1} />
      </button>
    </div>)}
    <div>
      <b>i</b><span>MIDI Clock Out</span>
      <select value={settings?.midiClockOut ?? "off"} disabled={pending} onChange={(event) => void workflow.setMidiClockOut(event.target.value as MidiClockOutValue)}>
        {MIDI_CLOCK_OUT_VALUES.map((value) => <option key={value} value={value}>{value === "off" ? "OFF" : value === "midiDinOnly" ? "MIDI DIN Only" : value === "usbMidiOnly" ? "USB MIDI Only" : "Both USB and DIN MIDI"}</option>)}
      </select>
    </div>
    <div>
      <b>i</b><span>MIDI Clock In</span>
      <button type="button" className="settings-midi-toggle" disabled={pending} aria-pressed={Boolean(settings?.midiClockIn)} onClick={() => void workflow.setMidiClockIn(!settings?.midiClockIn)}>
        <QcPillToggle index={settings?.midiClockIn ? 0 : 1} />
        <QcPillToggleLabels labels={["On", "Off"]} index={settings?.midiClockIn ? 0 : 1} />
      </button>
    </div>
  </div>;
}

/** Screen/LED/Dimmed LED brightness: settings-system-detail's own three-row
 * layout (CorOsOfficialSettings), each row a real setGeneralInteger call. The
 * unit changes these with its footswitches, so the bars show no buttons; an
 * invisible hit area over each bar (QcSegmentBarHit) keeps them operable from
 * the app. The three mode marks at the bottom toggle per-mode LED dimming. */
function BrightnessDetail({ workflow }: { workflow: DeviceSettingsWorkflow }) {
  const { settings, pending } = workflow;
  const rows: Array<[string, number | undefined, number, (value: number) => void]> = [
    ["Screen", settings?.screenBrightness, 32, (value) => void workflow.setScreenBrightness(value)],
    ["LEDs", settings?.ledBrightness, 32, (value) => void workflow.setLedBrightness(value)],
    ["Dimmed LEDs", settings?.dimmedLedBrightness, 31, (value) => void workflow.setDimmedLedBrightness(value)]
  ];
  return <>
    <h1>Brightness</h1>
    <QcBrightnessInstructions lines={BRIGHTNESS_BODY_LINES} />
    {rows.map(([label, value, max, onChange]) => <div key={label}>
      <span>{label}</span>
      <strong>{value ?? "—"}</strong>
      <QcSegmentBar segments={32} filled={value ?? 0} label={`${label} brightness`} />
      <QcSegmentBarHit label={`${label} brightness`} value={value} max={max} segments={32} disabled={pending} onChange={onChange} />
    </div>)}
    <QcBrightnessModeRow
      dimmed={{ PRESET: settings?.presetDimmed, SCENE: settings?.sceneDimmed, STOMP: settings?.stompDimmed }}
      disabled={pending}
      onToggle={(mode, next) => void workflow.setModeDimmed(mode, next)} />
  </>;
}

function MasterVolumeDetail({ workflow }: { workflow: DeviceSettingsWorkflow }) {
  const { settings, pending } = workflow;
  const assignment = settings?.masterVolumeAssignment;
  const fields: Array<[string, boolean | undefined, keyof NonNullable<typeof assignment>]> = [
    ["OUT 1/2", assignment?.out12, "out12"],
    ["OUT 3/4", assignment?.out34, "out34"],
    ["SEND 1/2", assignment?.send12, "send12"],
    ["Headphones", assignment?.headphones, "headphones"]
  ];
  const toggle = (field: keyof NonNullable<typeof assignment>) => {
    if (!assignment || pending) return;
    const next = { ...assignment, [field]: !assignment[field] };
    void workflow.setMasterVolumeAssignment(next.out12, next.out34, next.send12, next.headphones);
  };
  return <>
    <h1>Master Volume Knob Assignment</h1>
    <p className="volume-lead">{MASTER_VOLUME_LEAD}</p>
    <hr />
    <div className="volume-assignments">
      {fields.map(([label, value, field]) => <span key={label}>
        <em className={field === "headphones" ? "is-headphones" : undefined} aria-label={field === "headphones" ? "Headphones" : undefined}>{field === "headphones" ? null : label}</em>
        <button type="button" disabled={pending || assignment === undefined} aria-pressed={Boolean(value)} aria-label={`Assign master volume to ${label}`} onClick={() => toggle(field)}>{value && <QcUiIcon kind="check" />}</button>
      </span>)}
    </div>
  </>;
}

function StorageDetail({ workflow }: { workflow: DeviceSettingsWorkflow }) {
  const { settings } = workflow;
  const available = settings?.availableDiskSpace;
  const total = settings?.totalDiskSpace;
  const used = available !== undefined && total !== undefined ? Math.max(0, total - available) : undefined;
  return <>
    <h1>Device Storage</h1>
    <p>Read-only: the current USB gateway reports total and available space, not the per-library breakdown the device's own screen draws.</p>
    <div className="settings-storage-readout">
      <span><span>Used</span><strong>{fraction(used, total)}</strong></span>
      <b><i style={{ width: used !== undefined && total ? `${Math.min(100, (used / total) * 100)}%` : "0%" }} /></b>
      <span><span>Available</span><strong>{available === undefined ? "—" : `${available}`}</strong></span>
      <span><span>Total</span><strong>{total === undefined ? "—" : `${total}`}</strong></span>
    </div>
  </>;
}

function DeviceInformationDetail({ workflow }: { workflow: DeviceSettingsWorkflow }) {
  const identity = workflow.identity;
  return <>
    <h1>Device information</h1>
    <div className="information-table">
      <span><b>Serial number:</b><i>{identity?.serial ?? "Not exposed"}</i></span>
      <span><b>Device name:</b><i>{identity?.customName || "Quad Cortex"}</i></span>
      <span><b>MAC address:</b><i>Not exposed</i></span>
    </div>
    <hr />
    <h1>Software information</h1>
    <div className="information-table">
      <span><b>CorOS:</b><i>{identity?.appFwVersion ?? "Not exposed"}</i></span>
    </div>
  </>;
}

/**
 * The header glyph next to the section name. Always the Device mark for the
 * Device section, even while browsing its own MIDI row: CorOsOfficialSettings'
 * `SettingsSectionGlyph` is only ever called with no `label` for the header,
 * so its `view === "settings-midi" && label === "MIDI"` branch (the one place
 * "menu-midi" appears) never actually fires there - only a *row*, which
 * always passes its own label, can reach it. See DEVICE_ROW_GLYPHS/the "midi"
 * row in DeviceDetail above for that.
 */
function AccentGlyph({ view }: { view: DeviceSettingsSection }) {
  if (view === "account") return <QcScreenGlyph kind="account-cloud" />;
  if (view === "system") return <QcUiIcon kind="settings" />;
  if (view === "support") return <QcScreenGlyph kind="contact" />;
  return <QcScreenGlyph kind="device" />;
}

/** The order and names of the unit's category list (the open selector on a physical unit: Account, System, Device, Support). */
export const DEVICE_SETTINGS_SECTIONS: ReadonlyArray<readonly [DeviceSettingsSection, string]> = [["account", "Account"], ["system", "System"], ["device", "Device"], ["support", "Support"]];

/**
 * The reference pane the unit draws for a row that has no gateway call, or
 * absent when the row is stateful (Connection lists the owner's networks,
 * Backups and My Account the owner's cloud account) and so keeps the plain
 * "not exposed" notice instead of a drawing of somebody else's state.
 */
const REFERENCE_PANES: { system: Partial<Record<DeviceSettingsSystemRow, ReferenceSettingsView>>; support: Partial<Record<DeviceSettingsSupportRow, ReferenceSettingsView>> } = {
  system: { updates: "settings-update-idle", power: "settings-system-power", factory: "settings-system-reset" },
  support: { about: "settings-support", report: "settings-support-report", diagnostics: "settings-diagnostics", licenses: "settings-support-licenses" }
};

/** The list the unit opens from the section selector; the selected section is green with a tick. */
function SectionMenu({ current, onSelect }: { current: DeviceSettingsSection; onSelect(section: DeviceSettingsSection): void }) {
  return <div className="settings-section-menu" role="listbox" aria-label="Settings section">
    {DEVICE_SETTINGS_SECTIONS.map(([key, label]) => <button key={key} type="button" role="option" aria-selected={key === current} className={key === current ? "is-active" : undefined} onClick={() => onSelect(key)}>
      <b><AccentGlyph view={key} /></b>
      {label}
      {key === current && <i className="settings-section-check"><QcUiIcon kind="check" /></i>}
    </button>)}
  </div>;
}

/**
 * The device's own Settings screen - Account/System/Device/Support, with
 * Device's eighth row reaching MIDI - ported from coros-screen-fixtures.tsx's
 * CorOsOfficialSettings/CorOsReferenceSettings. Reached from the Grid's own
 * context menu (coros-ui.ts GRID_CONTEXT_MENU, action "settings") - never the
 * generic app-preferences window the MenuBar's own separate "Settings…" item
 * opens (App.tsx, untouched here).
 *
 * The fixture was a static single-view snapshot per row; this component adds
 * the real navigation (the section selector opens the unit's own list, row
 * selection, active-row highlight) and binds every row with a real backend
 * call to useDeviceSettingsWorkflow. A row without one is drawn as the unit
 * draws it, inert, where that pane holds no state of the owner's (see
 * REFERENCE_PANES), and otherwise keeps the plain notice; selecting either
 * says so in the status line. No row is removed - see unavailableMessage above.
 */
export function CorOsDeviceSettings({ workflow, onClose }: DeviceSettingsScreenProps) {
  const { section, setSection, accountRow, setAccountRow, systemRow, setSystemRow, deviceRow, setDeviceRow, supportRow, setSupportRow, loading, announce } = workflow;
  const [menuOpen, setMenuOpen] = useState(false);
  const sectionSelector = useRef<HTMLButtonElement>(null);
  const sectionTitle = DEVICE_SETTINGS_SECTIONS.find(([key]) => key === section)?.[1] ?? "Device";
  const info = { serial: workflow.identity?.serial, deviceName: workflow.identity?.customName || undefined, corOsVersion: workflow.identity?.appFwVersion };
  const select = (message: string | undefined, apply: () => void) => () => { apply(); if (message) announce(message); };

  let nav: { key: string; label: string; active: boolean; unavailable: boolean; icon: () => ReactNode; onSelect(): void }[];
  let detail: ReactNode;
  let referenceView: ReferenceSettingsView | undefined;
  if (section === "account") {
    nav = DEVICE_SETTINGS_ACCOUNT_ROWS.map((row) => ({
      key: row, label: ACCOUNT_ROW_LABELS[row], active: row === accountRow, unavailable: true,
      icon: () => <QcScreenGlyph kind={row === "backup" ? "backup" : "user"} />,
      onSelect: select(unavailableMessage(ACCOUNT_ROW_LABELS[row]), () => setAccountRow(row))
    }));
    detail = <UnavailableDetail label={ACCOUNT_ROW_LABELS[accountRow]} />;
  } else if (section === "system") {
    referenceView = REFERENCE_PANES.system[systemRow];
    nav = DEVICE_SETTINGS_SYSTEM_ROWS.map((row) => {
      const readOnly = row !== "brightness" && row !== "volume" && row !== "storage";
      return {
        key: row, label: SYSTEM_ROW_LABELS[row], active: row === systemRow, unavailable: readOnly && !REFERENCE_PANES.system[row],
        icon: () => <QcSettingsIcon kind={SYSTEM_ROW_ICONS[row]} />,
        onSelect: select(readOnly ? unavailableMessage(SYSTEM_ROW_LABELS[row]) : undefined, () => setSystemRow(row))
      };
    });
    detail = systemRow === "brightness" ? <BrightnessDetail workflow={workflow} />
      : systemRow === "volume" ? <MasterVolumeDetail workflow={workflow} />
      : systemRow === "storage" ? <StorageDetail workflow={workflow} />
      : referenceView ? <CorOsReferenceSettingsPane view={referenceView} deviceInformation={info} stateUnknown />
      : <UnavailableDetail label={SYSTEM_ROW_LABELS[systemRow]} />;
  } else if (section === "device") {
    nav = DEVICE_SETTINGS_DEVICE_ROWS.map((row) => ({
      key: row, label: DEVICE_ROW_LABELS[row], active: row === deviceRow, unavailable: false,
      icon: () => row === "midi" ? <QcUiIcon kind="menu-midi" /> : <QcScreenGlyph kind={DEVICE_ROW_GLYPHS[row]} />,
      onSelect: () => setDeviceRow(row)
    }));
    detail = <DeviceDetail row={deviceRow} workflow={workflow} />;
  } else {
    referenceView = REFERENCE_PANES.support[supportRow];
    nav = DEVICE_SETTINGS_SUPPORT_ROWS.map((row) => {
      const readOnly = row !== "info";
      return {
        key: row, label: SUPPORT_ROW_LABELS[row], active: row === supportRow, unavailable: readOnly && !REFERENCE_PANES.support[row],
        icon: () => <QcScreenGlyph kind={SUPPORT_ROW_GLYPHS[row]} />,
        onSelect: select(readOnly ? unavailableMessage(SUPPORT_ROW_LABELS[row]) : undefined, () => setSupportRow(row))
      };
    });
    detail = supportRow === "info" ? <DeviceInformationDetail workflow={workflow} />
      : referenceView ? <CorOsReferenceSettingsPane view={referenceView} deviceInformation={info} />
      : <UnavailableDetail label={SUPPORT_ROW_LABELS[supportRow]} />;
  }

  // A device screen, not a window-level dialog: the host passes it as the
  // surface's screenContent so it fills the bezel's own 800x480 canvas.
  return <>
    <section className={`qc-screen coros-settings-official settings-${section}${section === "support" && supportRow === "info" ? " settings-info" : ""}${referenceView ? ` coros-settings-reference ${referenceView}` : ""}${menuOpen ? " is-menu-open" : ""}`} aria-label={`${sectionTitle} Settings`} data-qc-open-target="settings" tabIndex={-1} onKeyDown={(event) => {
      if (event.key === "Escape" && !event.defaultPrevented && menuOpen) {
        event.preventDefault();
        event.stopPropagation();
        setMenuOpen(false);
        sectionSelector.current?.focus();
        return;
      }
      escapeUserOpenedTarget(event, onClose, workflow.returnFocusTarget);
    }}>
    <header>
      <button type="button" className="settings-section" ref={sectionSelector} aria-haspopup="listbox" aria-expanded={menuOpen} onClick={() => setMenuOpen((open) => !open)} aria-label={`Settings section: ${sectionTitle}. Activate to choose a section.`}>
        <b><AccentGlyph view={section} /></b>
        {sectionTitle}
        <i />
      </button>
      <button type="button" className="settings-done" aria-label="Close Settings" onClick={(event) => closeUserOpenedTarget(event.currentTarget, onClose, workflow.returnFocusTarget)}><QcUiIcon kind="check" /></button>
    </header>
    <main>
      <nav>
        {nav.map((item) => <button key={item.key} type="button" className={`${item.active ? "is-active" : ""}${item.unavailable ? " is-unavailable" : ""}`.trim()} disabled={loading} onClick={item.onSelect}>
          <b>{item.icon()}</b>
          {item.label}
        </button>)}
      </nav>
      <section className={
        referenceView ? "reference-settings-detail"
          : section === "account" ? "settings-account-detail"
          : section === "device" && deviceRow !== "midi" ? "settings-device-detail"
          : section === "system" && (systemRow === "brightness") ? "settings-system-detail"
          : "reference-settings-detail"
      } {...(referenceView ? { inert: true, "aria-disabled": true } : {})}>
        {loading ? <p>Reading device settings…</p> : detail}
      </section>
    </main>
    {menuOpen && <>
      <button type="button" className="settings-menu-wash" aria-label="Close section list" onClick={() => setMenuOpen(false)} />
      <SectionMenu current={section} onSelect={(next) => { setSection(next); setMenuOpen(false); }} />
    </>}
  </section>
  </>;
}
