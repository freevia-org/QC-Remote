import assert from "node:assert/strict";
import test from "node:test";
import { createRequire, register } from "node:module";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { act, create } from "react-test-renderer";

register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const rendererRequire = createRequire(createRequire(import.meta.url).resolve("react-test-renderer"));
const { build } = await import("esbuild");
const work = mkdtempSync(join(tmpdir(), "qc-options-menu-"));
const outfile = join(work, "editor.mjs");
let CorOsParameterEditor: any;
try {
  await build({
    stdin: { contents: `export { CorOsParameterEditor } from ${JSON.stringify(fileURLToPath(new URL("../src/parameter-editor.tsx", import.meta.url)))};`, resolveDir: fileURLToPath(new URL("../", import.meta.url)), loader: "tsx" },
    outfile, bundle: true, format: "esm", platform: "node", jsx: "automatic", logLevel: "error",
    loader: { ".ttf": "dataurl", ".svg": "dataurl", ".png": "dataurl", ".woff2": "dataurl", ".css": "empty" },
    plugins: [{ name: "renderer-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/jsx(?:-dev)?-runtime)?$/ }, ({ path }) => ({ path: pathToFileURL(rendererRequire.resolve(path)).href, external: true }));
    } }]
  });
  ({ CorOsParameterEditor } = await import(pathToFileURL(outfile).href));
} catch (error) { rmSync(work, { recursive: true, force: true }); throw error; }
test.after(() => rmSync(work, { recursive: true, force: true }));
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

/*
 * The options menu keeps the semantics the app's mirror of the unit relies on: its open state is reported (so the unit's own
 * menu can follow: the kebab there, a tap on an empty area to close), a tap on the dim closes it, and a row runs the one
 * existing action and closes it.
 */

function props(log: string[], extra: object = {}) {
  const parameter = { index: 0, name: "GAIN", type: "float", options: [], steps: 101, normalizedValue: .5, writable: true, enabled: true, minimum: 0, maximum: 10, units: "", displayValue: "5.0" };
  return { details: { name: "Brit 2203", category: "Amp", modelId: 42, row: 0, column: 0, scene: 0, parameters: [parameter] },
    drafts: {}, accent: "white", activeScene: 0, scenes: ["A"], bypassed: false, page: 0,
    onPageChange() {}, onDraftChange() {}, onCommit() {}, onToggleSceneMode() {}, onToggleBypass() {}, onSceneSelect() {},
    onFootswitchAssignmentStart() {}, onUnassignFootswitch() {}, onContextAction: (action: string) => log.push(`action:${action}`), onClose() {},
    onMenuOpenChange: (open: boolean) => log.push(`menu:${open}`), contextActionEnabled: { "paste-device": false, "copy-device": true, expression: true, "reset-defaults": false }, ...extra };
}

test("the kebab opens the menu as a screen layer, the dim closes it, and a row runs its action once", (t) => {
  const log: string[] = [];
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(CorOsParameterEditor, props(log))); });
  t.after(() => act(() => renderer.unmount()));
  const scrims = () => renderer.root.findAll((node) => node.type === "button" && "data-qc-options-scrim" in node.props);
  assert.equal(scrims().length, 0);
  assert.equal(renderer.root.findAllByProps({ role: "menu" }).length, 0);
  act(() => renderer.root.findByProps({ "aria-label": "Device contextual menu", className: "parameter-more" }).props.onClick());
  assert.equal(scrims().length, 1);
  const menu = renderer.root.findAllByProps({ role: "menu" })[0];
  const rows = menu.findAll((node) => node.type === "button" && node.props.role === "menuitem");
  assert.deepEqual(rows.map((row) => row.children.filter((child) => typeof child === "string").join("")), ["Change device", "Copy device", "Paste device", "Reset to defaults", "Save Current Parameters as...", "Assign expression pedal", "Mute/bypass", "Model update available", "Change to legacy version", "Remove block from the grid"]);
  assert.deepEqual(rows.map((row) => row.props.disabled).slice(0, 4), [false, false, true, true], "Paste device and a gated action are not clickable");
  // The menu is a sibling of the editor, never inside it: the editor section holds no menu.
  assert.equal(renderer.root.findAll((node) => node.type === "section" && node.findAllByProps({ role: "menu" }).length > 0).length, 0);
  act(() => scrims()[0].props.onClick());
  assert.equal(scrims().length, 0);
  act(() => renderer.root.findByProps({ className: "parameter-more" }).props.onClick());
  act(() => renderer.root.findAllByProps({ role: "menuitem" }).find((row: any) => row.children.includes("Copy device"))!.props.onClick());
  assert.equal(scrims().length, 0, "a row closes the menu");
  assert.deepEqual(log.filter((entry, index) => entry !== log[index - 1]), ["menu:false", "menu:true", "menu:false", "menu:true", "action:copy-device", "menu:false"]);
  assert.equal(log.filter((entry) => entry === "action:copy-device").length, 1);
});

test("a menu opened by the app's route starts open and closes with the editor", (t) => {
  const log: string[] = [];
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(CorOsParameterEditor, props(log, { initialMenuOpen: true }))); });
  assert.equal(renderer.root.findAllByProps({ role: "menu" }).length > 0, true);
  act(() => renderer.unmount());
  assert.deepEqual(log, ["menu:true", "menu:false"]);
  t.diagnostic("closing the editor reports the menu closed, so the unit's follows");
});
