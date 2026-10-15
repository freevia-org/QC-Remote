import type { DeviceSettingsAccountRow, DeviceSettingsDeviceRow, DeviceSettingsSupportRow, DeviceSettingsSystemRow } from "./use-device-settings-workflow.ts";

/**
 * Which Settings section and row the unit's own Settings dialog shows, read from its graphics tree, so the app's Settings
 * screen can follow it (unit to app only; the app never drives a section change on the unit).
 *
 * The section selector (`zenUI::ComboBoxWithSelector`) and the row table (`zenUI::SettingsMenu`) carry no usable text for
 * the section, so the pane is named by the class of the content node drawn beside the menu, exactly as the CorOS version
 * is found by the class `zenUI::DeviceInformation` (cor-os-version.ts). Only class names are read: no `text :` line is
 * ever looked at, so nothing the pane displays (serial number, MAC address, kernel, hashes) can reach the result.
 *
 * A Settings dialog whose content class is not in {@link QC_SETTINGS_UNIT_PANES} yields no selection: the app is left on
 * whatever it shows rather than moved to a guess. A pane is added here only once its class has been read from a tree.
 */
export type QcSettingsUnitSelection =
  | { section: "account"; row: DeviceSettingsAccountRow }
  | { section: "system"; row: DeviceSettingsSystemRow }
  | { section: "device"; row: DeviceSettingsDeviceRow }
  | { section: "support"; row: DeviceSettingsSupportRow };

export interface QcSettingsUnitPane {
  selection: QcSettingsUnitSelection;
  /** What is known about the class and where. */
  evidence: string;
}

/** The content class of each Settings pane the app can follow, with the section and row it selects in the app. */
export const QC_SETTINGS_UNIT_PANES: Readonly<Record<string, QcSettingsUnitPane>> = {
  DeviceInformation: { selection: { section: "support", row: "info" }, evidence: "Settings > Support > Device Information, read from a 4.1.0 tree (tests/support/device-information-unit-tree.txt)" },
  UpdaterSettings: { selection: { section: "system", row: "updates" }, evidence: "the Updates pane's class, seen under zenUI::SettingsDialog (qc-unit-screens.ts treats it as display only)" },
  WifiSettings: { selection: { section: "system", row: "wifi" }, evidence: "Settings > System > Connection, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  BrightnessSettings: { selection: { section: "system", row: "brightness" }, evidence: "Settings > System > Brightness, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  PowerButtonSensitivitySettings: { selection: { section: "system", row: "power" }, evidence: "Settings > System > Power Functions, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  MasterVolumeSettings: { selection: { section: "system", row: "volume" }, evidence: "Settings > System > Master Volume Knob, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  DiskSpaceSettings: { selection: { section: "system", row: "storage" }, evidence: "Settings > System > Device Storage, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  GlobalBypassSettings: { selection: { section: "device", row: "bypass" }, evidence: "Settings > Device > Global Bypass, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  BypassSettings: { selection: { section: "device", row: "scene-bypass" }, evidence: "Settings > Device > Scene Bypass Behavior, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  StompModeBypassAssignmentSettings: { selection: { section: "device", row: "stomp-bypass" }, evidence: "Settings > Device > Stomp Mode Bypass, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  HoldTimeSettings: { selection: { section: "device", row: "hold" }, evidence: "Settings > Device > Hold Timing, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  SwapTempoAndTunerAccessSettings: { selection: { section: "device", row: "swap" }, evidence: "Settings > Device > Swap Tempo and Tuner, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  GigViewStompAccessSettings: { selection: { section: "device", row: "gig-access" }, evidence: "Settings > Device > Gig View Access, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  DynamicDelayCompensationSetting: { selection: { section: "device", row: "latency" }, evidence: "Settings > Device > Latency Compensation, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  MidiSettings: { selection: { section: "device", row: "midi" }, evidence: "Settings > Device > MIDI, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  ContactUsMenu: { selection: { section: "support", row: "about" }, evidence: "Settings > Support > About and Contact, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" },
  ThirdPartyLicenses: { selection: { section: "support", row: "licenses" }, evidence: "Settings > Support > 3rd Party Licenses, measured on a CorOS 4.1.0 unit: the last child of zenUI::SettingsDialog when the row is selected" }
};

/**
 * Rows the unit has that are deliberately not registered, because their pane class has not been read from a tree (the row
 * was avoided on the unit): Account (leads to sign-in), System > Factory Reset, Support > Send Report, Support > Diagnostics.
 * They stay unregistered, so the app is left where it is on them; a class is never guessed.
 */
export const QC_SETTINGS_ROWS_NOT_CAPTURED: ReadonlyArray<QcSettingsUnitSelection> = [
  { section: "account", row: "user" }, { section: "account", row: "backup" },
  { section: "system", row: "factory" }, { section: "support", row: "report" }, { section: "support", row: "diagnostics" }
];

const CLASS_LINE = /^\s*zenUI::([A-Za-z0-9_]+)\s*$/;

/** The unit's Settings pane named by the tree, or undefined when the tree is not the Settings dialog or shows a pane not yet registered. */
export function settingsSelectionFromTree(tree: string): QcSettingsUnitSelection | undefined {
  if (!/^\s*zenUI::SettingsDialog\s*$/m.test(tree)) return undefined;
  for (const line of tree.split(/\r?\n/)) {
    const name = line.match(CLASS_LINE)?.[1];
    if (name !== undefined && Object.hasOwn(QC_SETTINGS_UNIT_PANES, name)) return { ...QC_SETTINGS_UNIT_PANES[name].selection };
  }
  return undefined;
}

/** A stable text for a selection, for comparing observations. */
export const settingsSelectionKey = (selection: QcSettingsUnitSelection | undefined): string =>
  selection ? `${selection.section}/${selection.row}` : "";
