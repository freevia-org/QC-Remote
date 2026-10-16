import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { register } from "node:module";
import { act, create } from "react-test-renderer";
import { classifyQcGraphicsTree } from "../src/use-adaptive-screen-polling.ts";
import { QC_SETTINGS_ROWS_NOT_CAPTURED, QC_SETTINGS_UNIT_PANES, settingsSelectionFromTree, settingsSelectionKey } from "../src/settings-unit-selection.ts";
import { DEVICE_SETTINGS_ACCOUNT_ROWS, DEVICE_SETTINGS_DEVICE_ROWS, DEVICE_SETTINGS_SUPPORT_ROWS, DEVICE_SETTINGS_SYSTEM_ROWS } from "../src/use-device-settings-workflow.ts";

register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const { useAdaptiveQcScreenPolling, qcGraphicsTreeStructuralSignature } = await import("../src/use-adaptive-screen-polling.ts");

const FIXTURE = readFileSync(new URL("./support/device-information-unit-tree.txt", import.meta.url), "utf8");
const classes = (...names: string[]) => ["zenUI::RootGraphicsItem", ...names.map((name) => `  zenUI::${name}`)].join("\n");

test("the Device Information screen is Support > Device Information", () => {
  assert.equal(classifyQcGraphicsTree(FIXTURE), "settings");
  assert.deepEqual(settingsSelectionFromTree(FIXTURE), { section: "support", row: "info" });
  assert.equal(settingsSelectionFromTree(FIXTURE.replace(/\n/g, "\r\n"))?.row, "info", "CRLF trees read the same");
});

test("the fixture is sanitized and the selection carries no text from the screen", () => {
  assert.doesNotMatch(FIXTURE, /[0-9A-Fa-f]{2}(:[0-9A-Fa-f]{2}){5}/, "a MAC address");
  assert.doesNotMatch(FIXTURE, /Linux \d|Linux [0-9a-f]{3,}|[0-9a-f]{7,}/i, "a kernel string or hash");
  for (const value of FIXTURE.match(/text : '([^']*)'/g) ?? []) {
    assert.match(value, /'(?:[A-Za-z ]+:?|X+|[X:]+|\d+\.\d+\.\d+|Linux X+)'$/, `unexpected text ${value}`);
  }
  assert.deepEqual(Object.keys(settingsSelectionFromTree(FIXTURE)!).sort(), ["row", "section"]);
});

test("only the Settings dialog names a pane, and only by a registered class", () => {
  assert.equal(settingsSelectionFromTree(classes("Grid", "Chain")), undefined);
  assert.equal(settingsSelectionFromTree(classes("DeviceInformation")), undefined, "the pane class without the dialog is not Settings");
  assert.equal(settingsSelectionFromTree(classes("SettingsDialog", "SettingsMenu")), undefined, "a pane not yet registered is left alone, not guessed");
  assert.equal(settingsSelectionFromTree(classes("SettingsDialog", "DeviceInformationExtra")), undefined, "class names match whole");
  assert.deepEqual(settingsSelectionFromTree(classes("SettingsDialog", "SettingsMenu", "UpdaterSettings")), { section: "system", row: "updates" });
  assert.equal(settingsSelectionFromTree(""), undefined);
});

test("every registered pane selects a section and row the app has, once", () => {
  const rows = { account: DEVICE_SETTINGS_ACCOUNT_ROWS, system: DEVICE_SETTINGS_SYSTEM_ROWS, device: DEVICE_SETTINGS_DEVICE_ROWS, support: DEVICE_SETTINGS_SUPPORT_ROWS } as const;
  const seen = new Set<string>();
  for (const [name, pane] of Object.entries(QC_SETTINGS_UNIT_PANES)) {
    assert.ok((rows[pane.selection.section] as readonly string[]).includes(pane.selection.row), `${name}: ${settingsSelectionKey(pane.selection)} is not an app row`);
    assert.ok(!seen.has(settingsSelectionKey(pane.selection)), `${name} duplicates another pane's row`);
    seen.add(settingsSelectionKey(pane.selection));
    assert.ok(pane.evidence.length > 0, `${name} states its evidence`);
    assert.match(name, /^[A-Za-z0-9_]+$/);
  }
});

test("a selection has a stable key and none has an empty one", () => {
  assert.equal(settingsSelectionKey({ section: "support", row: "info" }), "support/info");
  assert.equal(settingsSelectionKey(undefined), "");
});

test("the poller hands the pane to the follower, and a pane change under one structure is an observation of its own", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const updates = classes("SettingsDialog", "SettingsMenu", "UpdaterSettings");
  let tree = FIXTURE;
  const seen: Array<{ kind: string; pane: string }> = [];
  const gateway = { graphicsTree: async () => ({ tree }) };
  function Probe() {
    useAdaptiveQcScreenPolling({ gateway, enabled: true, connected: true, onObservation: (o) => seen.push({ kind: o.kind, pane: settingsSelectionKey(o.settingsSelection) }) });
    return null;
  }
  let renderer!: ReturnType<typeof create>;
  await act(async () => { renderer = create(React.createElement(Probe)); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  assert.deepEqual(seen, [{ kind: "settings", pane: "support/info" }]);
  assert.notEqual(qcGraphicsTreeStructuralSignature(FIXTURE), qcGraphicsTreeStructuralSignature(updates));
  tree = updates;
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 400)); });
  assert.deepEqual(seen.at(-1), { kind: "settings", pane: "system/updates" });
});

/** Measured on a CorOS 4.1.0 unit: the content class that is the last child of zenUI::SettingsDialog for each row. */
const MEASURED: ReadonlyArray<readonly [string, string, string, string]> = [
  ["MyAccountMain", "account", "user", "My Account"],
  ["Backups", "account", "backup", "Backups"],
  ["WifiSettings", "system", "wifi", "Connection"],
  ["UpdaterSettings", "system", "updates", "Updates"],
  ["BrightnessSettings", "system", "brightness", "Brightness"],
  ["PowerButtonSensitivitySettings", "system", "power", "Power Functions"],
  ["MasterVolumeSettings", "system", "volume", "Master Volume Knob"],
  ["DiskSpaceSettings", "system", "storage", "Device Storage"],
  ["ResetMenu", "system", "factory", "Factory Reset"],
  ["GlobalBypassSettings", "device", "bypass", "Global Bypass"],
  ["BypassSettings", "device", "scene-bypass", "Scene Bypass Behavior"],
  ["StompModeBypassAssignmentSettings", "device", "stomp-bypass", "Stomp Mode Bypass"],
  ["HoldTimeSettings", "device", "hold", "Hold Timing"],
  ["SwapTempoAndTunerAccessSettings", "device", "swap", "Swap Tempo and Tuner"],
  ["GigViewStompAccessSettings", "device", "gig-access", "Gig View Access"],
  ["DynamicDelayCompensationSetting", "device", "latency", "Latency Compensation"],
  ["MidiSettings", "device", "midi", "MIDI"],
  ["ContactUsMenu", "support", "about", "About and Contact"],
  ["DeviceInformation", "support", "info", "Device Information"],
  ["SendReport", "support", "report", "Send Report"],
  ["DiagnosticsMenu", "support", "diagnostics", "Diagnostics"],
  ["ThirdPartyLicenses", "support", "licenses", "3rd Party Licenses"]
];
/** A synthetic Settings tree: class names only, the shape of the sanitized fixture, with the row's content class last. */
const settingsTree = (content: string) => [
  "zenUI::RootGraphicsItem", "  zenUI::GraphicsItem", "    zenUI::Grid", "      zenUI::Chain",
  "  zenUI::SettingsDialog", "    zenUI::SettingsMenu", "      zenUI::ComboBoxWithSelector", "      zenUI::Table", "        zenUI::TableCell",
  `    zenUI::${content}`, "    zenUI::Button", "      zenUI::PushButton"
].join("\n");

test("each measured Settings pane selects its section and row, and the registry holds exactly those", () => {
  for (const [cls, section, row, label] of MEASURED) {
    const tree = settingsTree(cls);
    assert.equal(classifyQcGraphicsTree(tree), "settings", `${label} is a Settings screen`);
    assert.deepEqual(settingsSelectionFromTree(tree), { section, row }, `${label} (${cls})`);
    assert.deepEqual(settingsSelectionFromTree(tree.replace(/\n/g, "\r\n")), { section, row }, `${label} reads the same with CRLF`);
  }
  assert.deepEqual(Object.keys(QC_SETTINGS_UNIT_PANES).sort(), MEASURED.map(([cls]) => cls).sort());
});

test("every app row is either registered or listed as not captured, never both and never neither", () => {
  const registered = new Set(Object.values(QC_SETTINGS_UNIT_PANES).map((pane) => settingsSelectionKey(pane.selection)));
  const notCaptured = new Set(QC_SETTINGS_ROWS_NOT_CAPTURED.map(settingsSelectionKey));
  const all = [
    ...DEVICE_SETTINGS_ACCOUNT_ROWS.map((row) => `account/${row}`), ...DEVICE_SETTINGS_SYSTEM_ROWS.map((row) => `system/${row}`),
    ...DEVICE_SETTINGS_DEVICE_ROWS.map((row) => `device/${row}`), ...DEVICE_SETTINGS_SUPPORT_ROWS.map((row) => `support/${row}`)
  ];
  for (const key of all) assert.notEqual(registered.has(key), notCaptured.has(key), `${key} must be exactly one of registered / not captured`);
  assert.equal(registered.size + notCaptured.size, all.length);
  assert.deepEqual([...notCaptured], [], "every row of the unit's four sections has been read from a tree");
});

test("the app lists each section's measured rows in the unit's order", () => {
  const order = (section: string) => MEASURED.filter((entry) => entry[1] === section).map((entry) => entry[2]);
  for (const [section, rows] of [["account", DEVICE_SETTINGS_ACCOUNT_ROWS], ["system", DEVICE_SETTINGS_SYSTEM_ROWS], ["device", DEVICE_SETTINGS_DEVICE_ROWS], ["support", DEVICE_SETTINGS_SUPPORT_ROWS]] as const) {
    const present = new Set(order(section));
    assert.deepEqual((rows as readonly string[]).filter((row) => present.has(row)), order(section), `${section} order`);
  }
});

test("a pane whose class was never read selects nothing, and a registered class buried in text does not count", () => {
  for (const guess of ["FactoryResetSettings", "SendReportSettings", "DiagnosticsSettings", "AccountSettings", "UserSettings", "BackupSettings", "WifiSettingsExtra", "BackupsExtra", "ResetMenuItem", "MyAccount"]) {
    assert.equal(settingsSelectionFromTree(settingsTree(guess)), undefined, `${guess} is not guessed`);
  }
  assert.equal(settingsSelectionFromTree(settingsTree("GraphicsItem") + "\n      text : 'zenUI::WifiSettings'"), undefined);
});

test("the synthetic trees carry class names only", () => {
  for (const [cls] of MEASURED) assert.doesNotMatch(settingsTree(cls), /text :|[0-9A-Fa-f]{2}(:[0-9A-Fa-f]{2}){5}|[0-9a-f]{7,}|Linux/i);
});

test("the poller follows the unit from pane to pane across sections, and an unregistered pane yields no selection", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  let tree = settingsTree("WifiSettings");
  const seen: string[] = [];
  const gateway = { graphicsTree: async () => ({ tree }) };
  function Probe() {
    useAdaptiveQcScreenPolling({ gateway, enabled: true, connected: true, onObservation: (o) => seen.push(settingsSelectionKey(o.settingsSelection)) });
    return null;
  }
  let renderer!: ReturnType<typeof create>;
  await act(async () => { renderer = create(React.createElement(Probe)); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  for (const cls of ["MidiSettings", "ThirdPartyLicenses", "FactoryResetSettings"]) {
    tree = settingsTree(cls);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 400)); });
  }
  assert.deepEqual(seen, ["system/wifi", "device/midi", "support/licenses", ""]);
});

test("the account section has exactly the unit's two rows, My Account then Backups, under the app's own ids", () => {
  assert.deepEqual([...DEVICE_SETTINGS_ACCOUNT_ROWS], ["user", "backup"]);
  assert.deepEqual(settingsSelectionFromTree(settingsTree("MyAccountMain")), { section: "account", row: "user" });
  assert.deepEqual(settingsSelectionFromTree(settingsTree("Backups")), { section: "account", row: "backup" });
});

/**
 * Following the unit is view-only. The panes that carry an action (reset, factory reset, send report, unlink, back up,
 * diagnostics) must be reached by selection alone: following moves the app's row and touches nothing else.
 */
const ACTION_PANES: ReadonlyArray<readonly [string, string, string]> = [
  ["ResetMenu", "system", "factory"], ["SendReport", "support", "report"], ["DiagnosticsMenu", "support", "diagnostics"],
  ["MyAccountMain", "account", "user"], ["Backups", "account", "backup"]
];

test("following the unit onto an action pane selects the row and calls nothing on the gateway", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const { useDeviceSettingsWorkflow } = await import("../src/use-device-settings-workflow.ts");
  const calls: string[] = [];
  const gateway = new Proxy({}, { get: (_target, name) => (...args: unknown[]) => { calls.push(String(name)); return Promise.reject(new Error(`unexpected gateway call ${String(name)}`)); } });
  const notices: string[] = [];
  let value!: ReturnType<typeof useDeviceSettingsWorkflow>;
  function Harness() {
    value = useDeviceSettingsWorkflow({ gateway: gateway as never, sessionKey: "usb-A", connected: true, pending: false, setPending: () => {}, notice: (m: string) => notices.push(m), fail: (e: unknown) => { throw e; } } as never);
    return null;
  }
  let renderer!: ReturnType<typeof create>;
  await act(async () => { renderer = create(React.createElement(Harness)); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  const rowOf = (section: string) => ({ account: value.accountRow, system: value.systemRow, device: value.deviceRow, support: value.supportRow })[section as "account"];
  for (const [cls, section, row] of ACTION_PANES) {
    const selection = settingsSelectionFromTree(settingsTree(cls))!;
    await act(async () => { value.followUnitSelection(selection); await new Promise((resolve) => setTimeout(resolve, 20)); });
    assert.equal(value.section, section, `${cls} section`);
    assert.equal(rowOf(section), row, `${cls} row`);
  }
  assert.deepEqual(calls, [], "no gateway call of any kind (no read, no write, no backup, no report, no reset)");
  assert.deepEqual(notices, [], "nothing was announced as done");
});

test("the panes behind those rows are inert in the app: no handler, no effect, no gateway, no scheduled work", () => {
  const read = (name: string) => readFileSync(new URL(`../src/${name}`, import.meta.url), "utf8");
  const code = (text: string) => text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
  const reference = code(read("coros-reference-settings-pane.tsx"));
  assert.ok(!/on[A-Z][A-Za-z]*\s*=/.test(reference), "the reference panes (reset, send report, diagnostics) draw buttons with no handler");
  assert.ok(!/useEffect|useLayoutEffect|useGateway|gateway|fetch\(|setTimeout|setInterval/.test(reference), "and start nothing on mount");
  const screen = read("device-settings-screen.tsx");
  const account = screen.slice(screen.indexOf('if (section === "account")'), screen.indexOf('} else if (section === "system")'));
  assert.match(account, /onSelect: select\(unavailableMessage\(ACCOUNT_ROW_LABELS\[row\]\), \(\) => setAccountRow\(row\)\)/, "an Account row only selects itself");
  assert.match(account, /<UnavailableDetail /, "and its pane is the plain notice, with no account, backup or sign-out control");
  assert.doesNotMatch(account, /workflow\./, "the Account section never calls the workflow");
  const detail = screen.slice(screen.indexOf("function UnavailableDetail"), screen.indexOf("function fraction"));
  assert.doesNotMatch(detail, /<button|onClick|useEffect/, "the notice has no control");
  const nav = screen.slice(screen.indexOf('referenceView = REFERENCE_PANES.system[systemRow]'), screen.indexOf("// A device screen, not a window-level dialog"));
  assert.doesNotMatch(nav.replace(/workflow=\{workflow\}/g, ""), /workflow\.(?!identity|screenCorOsVersion|identityReadAt)/, "System and Support rows reach no workflow action");
});
