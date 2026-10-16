import assert from "node:assert/strict";
import test from "node:test";
import { createRequire, register } from "node:module";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { act, create } from "react-test-renderer";

/*
 * The unit never draws a negative zero ("0.0 dB", not "-0.0 dB"), and with no signal its output meter reads its -40.0 dB
 * floor and its limiter reads 0.0 dB. The host publishes settings only - no meter or limiter data - so the app draws the idle
 * readings; they are not derived from the port's level setting. All values here are synthetic.
 */

register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const { formatDb, formatFixed } = await import("../src/format-number.ts");
const { ioSelectionFromTree } = await import("../src/io-unit-selection.ts");
const { useIoEqWorkflows } = await import("../src/use-io-eq-workflows.ts");

const rendererRequire = createRequire(createRequire(import.meta.url).resolve("react-test-renderer"));
const { build } = await import("esbuild");
const work = mkdtempSync(join(tmpdir(), "qc-io-readouts-"));
const { CorOsIoSettingsScreen } = await (async () => {
  try {
    const outfile = join(work, "io.mjs");
    await build({
      stdin: { contents: `export { CorOsIoSettingsScreen } from ${JSON.stringify(fileURLToPath(new URL("../src/io-global-eq-screens.tsx", import.meta.url)))};`, resolveDir: fileURLToPath(new URL("../", import.meta.url)), loader: "tsx" },
      outfile, bundle: true, format: "esm", platform: "node", jsx: "automatic", logLevel: "error",
      loader: { ".ttf": "dataurl", ".svg": "dataurl", ".png": "dataurl", ".woff2": "dataurl" },
      plugins: [{ name: "renderer-react", setup(builder) {
        builder.onResolve({ filter: /^react(?:\/jsx(?:-dev)?-runtime)?$/ }, ({ path }) => ({ path: pathToFileURL(rendererRequire.resolve(path)).href, external: true }));
      } }]
    });
    return await import(pathToFileURL(outfile).href);
  } catch (error) {
    rmSync(work, { recursive: true, force: true });
    throw error;
  }
})();
test.after(() => rmSync(work, { recursive: true, force: true }));
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

const OUTPUT_12_TREE = readFileSync(new URL("./support/io-unit-tree.txt", import.meta.url), "utf8");

test("a fixed-decimal reading never prints a negative zero", () => {
  assert.equal(formatFixed(-0), "0.0");
  assert.equal(formatFixed(-0.04), "0.0");
  assert.equal(formatFixed(-0.0004, 2), "0.00");
  assert.equal(formatFixed(0.04), "0.0");
  assert.equal(formatDb(-0.04), "0.0 dB");
  assert.equal(formatDb(-0.05), "-0.1 dB", "a value that rounds away from zero keeps its sign");
  assert.equal(formatDb(-12.34), "-12.3 dB");
  assert.equal(formatDb(6), "6.0 dB");
});

async function renderOutput12(levels: [number, number]) {
  const ioSettings = { inputs: [], outputs: [4, 5].map((outputPortId, i) => ({ outputPortId, levelDb: levels[i], groundLift: 0, muted: false })), expressionPorts: [], xlr12Linked: false, out34Linked: false };
  const options = { gateway: { ioSettings: async () => ioSettings, globalEq: async () => ({ parameters: [] }), setOutputPort: async () => ({ detail: "ok" }) }, connected: true, sessionKey: "usb-A", pending: false, setPending() {}, notice() {}, fail(error: unknown) { throw error; } };
  const state: { workflow?: ReturnType<typeof useIoEqWorkflows> } = {};
  let renderer!: ReturnType<typeof create>;
  function Host() {
    const workflow = useIoEqWorkflows(options as never);
    state.workflow = workflow;
    return React.createElement(CorOsIoSettingsScreen, { workflow, onGlobalEq() {}, onClose() {} } as never);
  }
  await act(async () => { renderer = create(React.createElement(Host)); });
  await act(async () => { await state.workflow!.refresh(); });
  await act(async () => state.workflow!.followUnitSelection(ioSelectionFromTree(OUTPUT_12_TREE)));
  return renderer;
}
const text = (node: { children: unknown[] }): string => node.children.map((child) => typeof child === "string" ? child : text(child as never)).join("");

test("Output 1/2: knob values show 0.0 dB, the meter its idle floor and the limiter its idle 0.0 dB", async (t) => {
  // Slightly negative and exact negative zero: both must read like the unit's "0.0 dB".
  const renderer = await renderOutput12([-0.04, -0]);
  t.after(async () => { await act(async () => renderer.unmount()); });
  const sections = renderer.root.findAll((node) => node.type === "section" && node.findAll((n) => n.props.className === "io-readout").length === 2);
  assert.equal(sections.length, 2, "one row per output");
  const dials = sections.map((section) => text(section.findAll((n) => n.props.className === "io-dial-wrap")[0] as never));
  assert.deepEqual(dials, ["0.0 dB", "0.0 dB"], "no negative zero on the OUT LEVEL knobs");
  for (const section of sections) {
    const [meter, limiter] = section.findAll((n) => n.props.className === "io-readout").map((node) => text(node as never));
    assert.equal(meter, "OUT LEVEL-40.0 dB", "the meter reads its idle floor, not the knob's level setting");
    assert.equal(limiter, "LIMITER0.0 dB", "the limiter reads its idle value rather than a dash");
  }
});

test("the OUT LEVEL meter does not follow the level knob", async (t) => {
  const renderer = await renderOutput12([6, -12.5]);
  t.after(async () => { await act(async () => renderer.unmount()); });
  const readouts = renderer.root.findAll((n) => n.props.className === "io-readout").map((node) => text(node as never));
  assert.deepEqual(readouts, ["OUT LEVEL-40.0 dB", "LIMITER0.0 dB", "OUT LEVEL-40.0 dB", "LIMITER0.0 dB"]);
  const dials = renderer.root.findAll((n) => n.props.className === "io-dial-wrap").map((node) => text(node as never));
  assert.deepEqual(dials, ["6.0 dB", "-12.5 dB"]);
});
