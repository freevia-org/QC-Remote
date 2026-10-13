import assert from "node:assert/strict";
import test from "node:test";
import { createRequire, register } from "node:module";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { act, create } from "react-test-renderer";

/*
 * The app's I/O screen follows the unit's own I/O screen: which port or pair is selected, and which jacks hold a plug,
 * are read from the unit's graphics tree on every open and while the screen is open. The unit's real tree (4.1.0,
 * Output 1/2 selected) is the fixture; the other selections are the same tree with the indicators moved.
 */

register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const { ioSelectionFromTree, ioSelectionKey } = await import("../src/io-unit-selection.ts");
const { classifyQcGraphicsTree, qcGraphicsTreeStructuralSignature, useAdaptiveQcScreenPolling } = await import("../src/use-adaptive-screen-polling.ts");
const { useIoEqWorkflows } = await import("../src/use-io-eq-workflows.ts");

const rendererRequire = createRequire(createRequire(import.meta.url).resolve("react-test-renderer"));
const { build } = await import("esbuild");
const work = mkdtempSync(join(tmpdir(), "qc-io-selection-"));
const loaded = await (async () => {
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
const { CorOsIoSettingsScreen } = loaded;
test.after(() => rmSync(work, { recursive: true, force: true }));
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

/** The unit's tree with Output 1/2 selected (OUT 1/L and OUT 2/R are `xlr/active`), headphones and USB plugged. */
const OUTPUT_12_TREE = readFileSync(new URL("./support/io-unit-tree.txt", import.meta.url), "utf8");
/** The same screen with `family`'s indicator for the port labelled `label` moved to `state`. */
function withIndicator(tree: string, label: string, state: string): string {
  const lines = tree.split("\n");
  const at = lines.findIndex((line) => line.trim() === `text : '${label}'`);
  const image = lines.findLastIndex((line, index) => index < at && /image: 'icons\/cheatsheet\//.test(line));
  lines[image] = lines[image].replace(/\/[a-z]+\.png/, `/${state}.png`);
  return lines.join("\n");
}
const output34Tree = ["OUT 1/L", "OUT 2/R"].reduce((tree, label) => withIndicator(tree, label, "free"), withIndicator(withIndicator(OUTPUT_12_TREE, "OUT 3/L", "active"), "OUT 4/R", "active"));

test("the unit's tree names the selected port or pair and the plugged jacks", () => {
  assert.equal(classifyQcGraphicsTree(OUTPUT_12_TREE), "io");
  assert.deepEqual(ioSelectionFromTree(OUTPUT_12_TREE), { view: "output", plugged: ["phones", "usb"] });
  assert.deepEqual(ioSelectionFromTree(output34Tree), { view: "overview", plugged: ["phones", "usb"] }, "OUT 3/L and OUT 4/R active is Output 3/4");
  const input2 = withIndicator(withIndicator(OUTPUT_12_TREE, "OUT 1/L", "free"), "INPUT 2", "active");
  assert.deepEqual(ioSelectionFromTree(withIndicator(input2, "OUT 2/R", "free")), { view: "input", inputId: 2, plugged: ["phones", "usb"] });
  const quiet = ["OUT 1/L", "OUT 2/R"].reduce((tree, label) => withIndicator(tree, label, "free"), OUTPUT_12_TREE);
  assert.deepEqual(ioSelectionFromTree(quiet), { plugged: ["phones", "usb"] }, "a frame with no active port selects nothing");
  assert.equal(ioSelectionFromTree("zenUI::RootGraphicsItem\n  zenUI::Grid\n"), undefined, "no I/O screen, no selection");
  assert.notEqual(ioSelectionKey(ioSelectionFromTree(OUTPUT_12_TREE)), ioSelectionKey(ioSelectionFromTree(output34Tree)));
});

test("Output 1/2 and Output 3/4 differ only in an indicator image, so the poller's signature must carry the selection", async (t) => {
  assert.equal(qcGraphicsTreeStructuralSignature(OUTPUT_12_TREE), qcGraphicsTreeStructuralSignature(output34Tree), "the structure alone cannot tell them apart");
  let tree = OUTPUT_12_TREE;
  const seen: Array<{ kind: string; view?: string }> = [];
  const gateway = { graphicsTree: async () => ({ tree }) };
  function Probe() {
    useAdaptiveQcScreenPolling({ gateway, enabled: true, connected: true, onObservation: (observation) => seen.push({ kind: observation.kind, view: observation.ioSelection?.view }) });
    return null;
  }
  let renderer!: ReturnType<typeof create>;
  await act(async () => { renderer = create(React.createElement(Probe)); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  assert.deepEqual(seen, [{ kind: "io", view: "output" }]);
  tree = output34Tree;
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 400)); });
  assert.deepEqual(seen.at(-1), { kind: "io", view: "overview" }, "the unit moving to Output 3/4 is an observation of its own");
  tree = OUTPUT_12_TREE;
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 400)); });
  assert.deepEqual(seen.at(-1), { kind: "io", view: "output" }, "and so is moving back");
});

function mount(initial: { gateway?: object; sessionKey?: string } = {}) {
  const writes: unknown[] = [];
  const ioSettings = { inputs: [], outputs: [4, 5, 6, 7].map((outputPortId) => ({ outputPortId, levelDb: 0, groundLift: 0, muted: false })), expressionPorts: [], xlr12Linked: false, out34Linked: false };
  const options = { gateway: initial.gateway ?? { ioSettings: async () => ioSettings, globalEq: async () => ({ parameters: [] }), setOutputPort: async (...args: unknown[]) => { writes.push(args); return { detail: "ok" }; } }, connected: true, sessionKey: initial.sessionKey ?? "usb-A", pending: false, setPending() {}, notice() {}, fail(error: unknown) { throw error; } };
  const state: { workflow?: ReturnType<typeof useIoEqWorkflows>; shown: boolean } = { shown: true };
  let renderer!: ReturnType<typeof create>;
  function Host() {
    const workflow = useIoEqWorkflows(options as never);
    state.workflow = workflow;
    return state.shown ? React.createElement(CorOsIoSettingsScreen, { workflow, onGlobalEq() {}, onClose() {} } as never) : null;
  }
  return {
    writes, state, options,
    async start() { await act(async () => { renderer = create(React.createElement(Host)); }); return renderer; },
    async update() { await act(async () => renderer.update(React.createElement(Host))); },
    /** What the unit's tree says, delivered the way the host does it. */
    async unit(tree: string | undefined) { await act(async () => state.workflow!.followUnitSelection(tree === undefined ? undefined : ioSelectionFromTree(tree))); },
    async show(shown: boolean) { state.shown = shown; await this.update(); },
    title: () => renderer.root.findByType("header").findByType("strong").children.join("")
  };
}
const heading = (host: ReturnType<typeof mount>) => host.title();

test("the screen shows the unit's selection on open, follows it while open, and never resurrects an old one", async (t) => {
  const host = mount();
  const renderer = await host.start();
  t.after(async () => { await act(async () => renderer.unmount()); });
  assert.equal(heading(host), "Output 3/4", "before the unit says anything the screen keeps its own default");
  // The unit opens I/O on Output 1/2 after the app last showed Output 3/4.
  await host.unit(OUTPUT_12_TREE);
  assert.equal(heading(host), "Output 1/2");
  // Output 3/4 is selected on the unit while the screen is open.
  await host.unit(output34Tree);
  assert.equal(heading(host), "Output 3/4");
  await host.unit(OUTPUT_12_TREE);
  assert.equal(heading(host), "Output 1/2");
  // The unit leaves I/O: the screen closes and the old selection is gone.
  await host.unit(undefined);
  await host.show(false);
  assert.equal(host.state.workflow!.unitSelection, undefined, "leaving the I/O screen clears the unit's selection");
  // Reopened by the unit on Output 1/2: the first render already shows it, never the earlier Output 3/4.
  await host.unit(OUTPUT_12_TREE);
  await host.show(true);
  assert.equal(heading(host), "Output 1/2", "a reopened screen starts on what the unit shows");
  // Reopened again with nothing known yet and then Output 3/4 from the unit.
  await host.unit(undefined);
  await host.show(false);
  await host.show(true);
  assert.equal(heading(host), "Output 3/4", "no unit selection: the screen's own default, not the selection of the visit before");
  await host.unit(output34Tree);
  assert.equal(heading(host), "Output 3/4");
});

test("a port the app's own tap selects stays selected until the unit says otherwise, and the plugged jacks come from the unit", async (t) => {
  const host = mount();
  const renderer = await host.start();
  t.after(async () => { await act(async () => renderer.unmount()); });
  await host.unit(OUTPUT_12_TREE);
  assert.equal(heading(host), "Output 1/2");
  const out3 = renderer.root.findByProps({ "data-io-socket": "out3" });
  await act(async () => out3.parent!.props.onClick({ target: { closest() { return null; }, tagName: "I" } }));
  assert.equal(heading(host), "Output 3/4", "the app's own selection is shown at once (and forwarded to the unit by the display's tap)");
  // The unit repeats what it showed before the tap: the very same selection must not undo the app's choice.
  await host.update();
  assert.equal(heading(host), "Output 3/4");
  await host.unit(output34Tree);
  assert.equal(heading(host), "Output 3/4", "the unit confirms the app's choice");
  await host.unit(OUTPUT_12_TREE);
  assert.equal(heading(host), "Output 1/2", "a change on the unit wins");
  const state = (socket: string) => renderer.root.findByProps({ "data-io-socket": socket }).findAll((node) => node.props["data-state"] !== undefined)[0].props["data-state"] as string;
  assert.equal(state("usb"), "plugged");
  assert.equal(state("phones"), "plugged", "the unit's headphones jack holds a plug");
  assert.equal(state("capture-out"), "idle");
  assert.equal(state("out1"), "active");
  assert.equal(state("out3"), "idle");
});

test("a new device session forgets the previous unit's selection", async (t) => {
  const host = mount();
  const renderer = await host.start();
  t.after(async () => { await act(async () => renderer.unmount()); });
  await host.unit(OUTPUT_12_TREE);
  assert.equal(host.state.workflow!.unitSelection?.view, "output");
  host.options.sessionKey = "usb-B";
  await host.update();
  assert.equal(host.state.workflow!.unitSelection, undefined);
  assert.equal(host.writes.length, 0, "following the unit sends nothing to it");
});
