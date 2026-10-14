import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { register } from "node:module";
import { act, create } from "react-test-renderer";
import { classifyQcGraphicsTree } from "../src/use-adaptive-screen-polling.ts";
import { QC_SETTINGS_UNIT_PANES, settingsSelectionFromTree, settingsSelectionKey } from "../src/settings-unit-selection.ts";
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
