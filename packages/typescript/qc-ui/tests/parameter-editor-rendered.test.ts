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
const work = mkdtempSync(join(tmpdir(), "qc-editor-rendered-"));
const outfile = join(work, "editors.mjs");
let components: Record<string, any>;
try {
await build({
  stdin: { contents: `export { CorOsParameterEditor } from ${JSON.stringify(fileURLToPath(new URL("../src/parameter-editor.tsx", import.meta.url)))};
export { PresetMidiOutEditor } from ${JSON.stringify(fileURLToPath(new URL("../src/preset-midi-out-editor.tsx", import.meta.url)))};
export { useRotaryValue } from ${JSON.stringify(fileURLToPath(new URL("../src/use-rotary-gesture.ts", import.meta.url)))};
export { usePresetMidiOutWorkflow } from ${JSON.stringify(fileURLToPath(new URL("../src/use-preset-midi-out-workflow.ts", import.meta.url)))};
export { CorOsDirectory } from ${JSON.stringify(fileURLToPath(new URL("../src/coros-directory.tsx", import.meta.url)))};
export { ModesConfigurationScreen } from ${JSON.stringify(fileURLToPath(new URL("../src/modes-configuration-screen.tsx", import.meta.url)))};
export { QcSegmentBarHit } from ${JSON.stringify(fileURLToPath(new URL("../src/qc-brightness-panel.tsx", import.meta.url)))};
export { qcDeviceScreenContent } from ${JSON.stringify(fileURLToPath(new URL("../src/qc-device-screen-content.tsx", import.meta.url)))};
export { QuadCortexSurface, QcHardwareSwitch } from ${JSON.stringify(fileURLToPath(new URL("../src/quad-cortex-surface.tsx", import.meta.url)))};
export { IDLE_PLACEHOLDER_SNAPSHOT } from ${JSON.stringify(fileURLToPath(new URL("../src/device-placeholder.ts", import.meta.url)))};
export { formFactors, skins } from "@qc-remote/form-factors";`, resolveDir: fileURLToPath(new URL("../", import.meta.url)), loader: "tsx" },
  outfile, bundle: true, format: "esm", platform: "node", jsx: "automatic", logLevel: "error",
  loader: { ".ttf": "dataurl", ".svg": "dataurl", ".png": "dataurl", ".woff2": "dataurl" },
  plugins: [{ name: "renderer-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/jsx(?:-dev)?-runtime)?$/ }, ({ path }) => ({ path: pathToFileURL(rendererRequire.resolve(path)).href, external: true }));
  } }]
});
components = await import(pathToFileURL(outfile).href);
} catch (error) {
  rmSync(work, { recursive: true, force: true });
  throw error;
}
const { CorOsParameterEditor, PresetMidiOutEditor, usePresetMidiOutWorkflow, CorOsDirectory, QuadCortexSurface, IDLE_PLACEHOLDER_SNAPSHOT, formFactors, skins } = components;
test.after(() => rmSync(work, { recursive: true, force: true }));
const { QcSegmentBarHit, ModesConfigurationScreen } = components;
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

function hardwareSwitch(t: any, extra: object = {}) {
  const previousWindow = globalThis.window;
  (globalThis as any).window = globalThis;
  const actions: any[] = [];
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(components.QcHardwareSwitch, {
    role: "footswitch:B", label: "B", onAction: (action: unknown) => actions.push(action), ...extra,
  })); });
  t.after(() => { act(() => renderer.unmount()); (globalThis as any).window = previousWindow; });
  return { actions, button: () => renderer.root.findByType("button") };
}

const switchTap = [
  { kind: "switch", role: "footswitch:B", phase: "press" },
  { kind: "switch", role: "footswitch:B", phase: "release" },
];

test("AT-SPI click without pointer/key events activates a hardware footswitch exactly once", (t) => {
  const { actions, button } = hardwareSwitch(t);
  act(() => button().props.onClick({ detail: 0, defaultPrevented: false }));
  assert.deepEqual(actions, switchTap);
  act(() => button().props.onClick({ detail: 0, defaultPrevented: true }));
  assert.deepEqual(actions, switchTap, "an already consumed activation is not replayed");
});

function switchPointer(y = 100) {
  return { pointerId: 1, pointerType: "mouse", clientY: y, clientX: 100,
    currentTarget: { setPointerCapture() {}, hasPointerCapture: () => false } };
}

test("pointer taps activate once and pointer click echoes after taps, drags and cancellation do not activate", (t) => {
  const { actions, button } = hardwareSwitch(t);
  act(() => { button().props.onPointerDown(switchPointer()); button().props.onPointerUp(switchPointer()); });
  act(() => button().props.onClick({ detail: 1 }));
  assert.deepEqual(actions, switchTap);
  actions.length = 0;
  act(() => { button().props.onPointerDown(switchPointer()); button().props.onPointerMove(switchPointer(76)); button().props.onPointerUp(switchPointer(76)); });
  act(() => button().props.onClick({ detail: 1 }));
  assert.deepEqual(actions, [{ kind: "rotate", role: "footswitch:B", delta: 2 }]);
  actions.length = 0;
  act(() => { button().props.onPointerDown(switchPointer()); button().props.onPointerCancel(switchPointer()); });
  act(() => button().props.onClick({ detail: 1 }));
  assert.deepEqual(actions, []);
});

for (const key of ["Enter", " "]) {
  test(`${JSON.stringify(key)} footswitch tap prevents native activation and ignores repeated keydowns`, (t) => {
    const { actions, button } = hardwareSwitch(t);
    const event = { key, repeat: false, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } };
    act(() => button().props.onKeyDown(event));
    assert.equal(event.defaultPrevented, true, "suppress the browser's key-generated click");
    act(() => button().props.onClick({ detail: 0 }));
    assert.deepEqual(actions, [], "activation cannot interrupt a held key");
    act(() => button().props.onKeyDown({ ...event, repeat: true }));
    const release = { ...event, defaultPrevented: false };
    act(() => button().props.onKeyUp(release));
    assert.equal(release.defaultPrevented, true);
    assert.deepEqual(actions, switchTap);
  });
}

test("pointer/key holds and accessibility activation retain their distinct tap and hold paths", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const holds: string[] = [];
  const { actions, button } = hardwareSwitch(t, { onHold: () => holds.push("hold"), holdDelayMs: 20 });
  act(() => button().props.onPointerDown(switchPointer()));
  act(() => t.mock.timers.tick(20));
  act(() => { button().props.onPointerUp(switchPointer()); button().props.onClick({ detail: 1 }); });
  const key = { key: "Enter", repeat: false, preventDefault() {} };
  act(() => button().props.onKeyDown(key));
  act(() => t.mock.timers.tick(20));
  act(() => button().props.onKeyUp(key));
  assert.deepEqual(holds, ["hold", "hold"]);
  assert.deepEqual(actions, [], "holds never become taps");
  act(() => button().props.onClick({ detail: 0 }));
  assert.deepEqual(actions, switchTap, "later AT-SPI activation is not swallowed by an old hold");
});

test("accessibility activation honors the hardware switch's custom tap action", (t) => {
  const taps: string[] = [];
  const { actions, button } = hardwareSwitch(t, { onTap: () => taps.push("tap") });
  act(() => button().props.onClick({ detail: 0 }));
  assert.deepEqual(taps, ["tap"]);
  assert.deepEqual(actions, []);
});

test("brightness segment slider ignores keyboard clicks, accepts pointer coordinates and honors disabled handlers", (t) => {
  const changes: number[] = [];
  const props = { label: "Screen brightness", value: 16, max: 31, segments: 32, disabled: false, onChange: (value: number) => changes.push(value) };
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(QcSegmentBarHit, props)); });
  t.after(() => act(() => renderer.unmount()));
  const slider = () => renderer.root.findByProps({ role: "slider" });
  const pointer = { detail: 1, clientX: 175, currentTarget: { getBoundingClientRect: () => ({ left: 100, width: 100 }) } };
  act(() => slider().props.onClick({ ...pointer, detail: 0, clientX: 0 }));
  assert.deepEqual(changes, [], "Enter/Space click is not a tap at the far-left coordinate");
  act(() => slider().props.onClick(pointer));
  assert.equal(changes.at(-1), 24);
  act(() => slider().props.onKeyDown({ key: "ArrowRight", preventDefault() {} }));
  assert.equal(changes.at(-1), 17);
  changes.length = 0;
  act(() => renderer.update(React.createElement(QcSegmentBarHit, { ...props, disabled: true })));
  act(() => { slider().props.onClick(pointer); slider().props.onKeyDown({ key: "ArrowRight", preventDefault() {} }); });
  assert.deepEqual(changes, []);
});

function propsFor(name: string, commits: number[]) {
  const cab = name.includes("C30");
  const parameter = { index: cab ? 5 : 0, name: cab ? "POSITION" : name === "Parametric-8" ? "N1 GAIN" : "THRESHOLD",
    type: "float", options: [], steps: 101, normalizedValue: .2, writable: true, enabled: true, minimum: 0, maximum: 10, units: "", displayValue: "2.0" };
  return { details: { name, category: cab ? "Guitar Cab" : name === "Parametric-8" ? "EQ" : "Utility", modelId: 42, row: 0, column: 0, scene: 0, parameters: [parameter] },
    drafts: {}, accent: "white", activeScene: 0, scenes: ["A"], bypassed: false, page: 0,
    onPageChange() {}, onDraftChange() {}, onCommit(_parameter: unknown, value: number) { commits.push(value); }, onToggleSceneMode() {}, onToggleBypass() {}, onSceneSelect() {},
    onFootswitchAssignmentStart() {}, onUnassignFootswitch() {}, onContextAction() {}, onClose() {} };
}

test("live EQ graph edits only currently writable enabled operands and bypass follows that admission", (t) => {
  const writes: number[] = [], drafts: number[] = [];
  const base = propsFor("Parametric-8", []);
  const parameter = base.details.parameters[0];
  const parameters = [
    { ...parameter, index: 0, name: "N1 GAIN", enabled: false },
    { ...parameter, index: 1, name: "N1 FREQ", minimum: 20, maximum: 20000, units: "Hz", writable: false },
    { ...parameter, index: 2, name: "N1 BYPASS", enabled: false }
  ];
  const props = { ...base, details: { ...base.details, parameters }, onDraftChange: (item: { index: number }) => drafts.push(item.index), onCommit: (item: { index: number }) => writes.push(item.index) };
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(CorOsParameterEditor, props)); });
  t.after(() => act(() => renderer.unmount()));
  const event = (x: number, y: number) => ({ pointerId: 1, clientX: x, clientY: y, currentTarget: {
    setPointerCapture() {}, hasPointerCapture: () => false,
    closest: () => ({ getBoundingClientRect: () => ({ width: 200, height: 100 }) })
  } });
  const node = () => renderer.root.findByProps({ "aria-label": "Select EQ band 1" });
  act(() => { node().props.onPointerDown(event(10, 10)); node().props.onPointerMove(event(30, 25)); node().props.onPointerUp(event(30, 25)); });
  const bypass = renderer.root.findByProps({ "aria-label": "Bypass EQ band 1" });
  assert.equal(bypass.props.disabled, true);
  act(() => bypass.props.onClick());
  assert.deepEqual([drafts, writes], [[], []]);
  const current = { ...props, details: { ...props.details, parameters: parameters.map((item) => ({ ...item, writable: true, enabled: true })) } };
  act(() => renderer.update(React.createElement(CorOsParameterEditor, current)));
  act(() => { node().props.onPointerDown(event(10, 10)); node().props.onPointerMove(event(30, 25)); node().props.onPointerUp(event(30, 25)); });
  assert.deepEqual([drafts, writes], [[1, 0], [1, 0]], "current gain and frequency remain usable");
  act(() => node().props.onPointerDown(event(10, 10)));
  act(() => renderer.update(React.createElement(CorOsParameterEditor, props)));
  drafts.length = 0; writes.length = 0;
  act(() => { node().props.onPointerMove(event(30, 25)); node().props.onPointerUp(event(30, 25)); });
  assert.deepEqual([drafts, writes], [[], []], "a device disabling a parameter during the drag retires that operand");
});

for (const name of ["Simple Gate", "Parametric-8", "212 UK C30 65 (M)"]) {
  test(`${name} uses live values and commits through its real rendered control`, async (t) => {
    const commits: number[] = [];
    const props = propsFor(name, commits);
    let renderer!: ReturnType<typeof create>;
    act(() => { renderer = create(React.createElement(CorOsParameterEditor, props)); });
    t.after(() => act(() => renderer.unmount()));
    const dials = renderer.root.findAll((node) => typeof node.type === "function" && ["QcRotaryDial", "QcBarSlider"].includes(node.type.name));
    assert.ok(dials.some((dial) => dial.props.value === .2));
    const range = renderer.root.findAll((node) => node.type === "input" && node.props.type === "range")[0];
    if (range) act(() => range.props.onPointerUp({ currentTarget: { value: ".3" } }));
    else {
      const slider = renderer.root.findAll((node) => node.type === "button" && node.props.role === "slider")[0];
      assert.ok(slider);
      await act(async () => { slider.props.onKeyDown({ key: "ArrowUp", preventDefault() {} }); slider.props.onKeyUp({ key: "ArrowUp" }); });
    }
    assert.equal(commits.length, 1);
    if (name === "Parametric-8") {
      act(() => renderer.update(React.createElement(CorOsParameterEditor, { ...props, referenceOnly: true })));
      assert.ok(renderer.root.findAll((node) => typeof node.type === "function" && node.type.name === "QcRotaryDial").some((node) => node.props.value === .5));
      assert.equal(renderer.root.findAll((node) => node.type === "button" && node.props.role === "slider").length, 0);
    }
  });
}

test("UNKNOWN mode renders an honest Grid without a fabricated known mode glyph", (t) => {
  const formFactor = formFactors[0], skin = skins.find((item: { id: string }) => item.id === formFactor.defaultSkinId);
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(QuadCortexSurface, { formFactor, skin, snapshot: { ...IDLE_PLACEHOLDER_SNAPSHOT, mode: "UNKNOWN" }, presentation: "display", canUndo: false,
    onAction() {}, onOpenPreset() {}, onUndo() {}, onSave() {}, onOpenRouting() {}, onRefresh() {} })); });
  t.after(() => act(() => renderer.unmount()));
  assert.ok(renderer.root.findAllByType("text").some((node) => node.children.includes("UNKNOWN")));
  assert.equal(renderer.root.findAllByType("svg").filter((node) => String(node.props.className).startsWith("qc-mode-glyph")).length, 0);
});

type GridFontLoad = { spec: string; text: string; resolve: () => void };
async function withGridFonts(run: (h: {
  loads: GridFontLoad[]; measured: Array<{ spec: string; text: string }>;
  show: (name?: string, dirty?: boolean) => Promise<void>;
  title: () => any; renderer: () => ReturnType<typeof create>;
}) => Promise<void>, failure?: "throw" | "reject") {
  const originalDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const loads: GridFontLoad[] = [];
  const measured: Array<{ spec: string; text: string }> = [];
  const loaded = new Set<string>();
  const key = (spec: string, text: string) => `${spec}\n${text}`;
  let renderer: ReturnType<typeof create> | undefined;
  const formFactor = formFactors[0], skin = skins.find((item: { id: string }) => item.id === formFactor.defaultSkinId);
  Object.defineProperty(globalThis, "document", { configurable: true, value: {
    fonts: { load(spec: string, text: string) {
      let resolve!: () => void;
      const promise = new Promise<unknown[]>((done) => { resolve = () => { loaded.add(key(spec, text)); done([{}]); }; });
      loads.push({ spec, text, resolve });
      if (failure === "throw") throw new Error("font load unavailable");
      if (failure === "reject") return Promise.reject(new Error("font fetch failed"));
      return promise;
    } },
    createElement(tag: string) {
      assert.equal(tag, "canvas");
      const context = { font: "", measureText(text: string) {
        measured.push({ spec: this.font, text });
        const ready = loaded.has(key(this.font, text));
        return { width: text === "32H" ? (ready ? 120 : 80) : (ready ? 400 : 600) };
      } };
      return { getContext: () => context };
    }
  } });
  const show = async (name = "PUBLIC TITLE", dirty = false) => {
    const view = React.createElement(QuadCortexSurface, { formFactor, skin,
      snapshot: { ...IDLE_PLACEHOLDER_SNAPSHOT, presetLocation: "32H", presetName: name, dirty, mode: "PRESET" },
      presentation: "display", canUndo: false,
      onAction() {}, onOpenPreset() {}, onUndo() {}, onSave() {}, onOpenRouting() {}, onRefresh() {} });
    await act(async () => { if (renderer) renderer.update(view); else renderer = create(view); });
  };
  try {
    await run({ loads, measured, show, renderer: () => renderer!, title: () => renderer!.root.findAllByType("text")
      .find((node) => String(node.props.className).split(" ").includes("preset-title"))! });
  } finally {
    if (renderer) await act(async () => renderer!.unmount());
    if (originalDocument) Object.defineProperty(globalThis, "document", originalDocument);
    else delete (globalThis as any).document;
  }
}

test("Grid keeps its fallback header visible and refits after each exact measured face becomes ready", async () => {
  await withGridFonts(async (h) => {
    await h.show();
    assert.deepEqual(h.loads.map(({ spec, text }) => [spec, text]), h.measured.map(({ spec, text }) => [spec, text]),
      "the effect waits on the same bank/title specs and normalized strings that Canvas measures");
    assert.ok(h.loads.every(({ spec }) => spec.startsWith("800 70px ")));
    assert.deepEqual(h.title().children, ["PUBLIC TITLE"]);
    assert.deepEqual([h.title().props.x, h.title().props.fontSize], [107, 50], "fallback layout stays rendered");
    await act(async () => h.loads[0].resolve());
    assert.deepEqual([h.title().props.x, h.title().props.fontSize], [147, 50], "bank readiness repositions using its real advance");
    await act(async () => h.loads[1].resolve());
    assert.deepEqual([h.title().props.x, h.title().props.fontSize], [147, 70], "title readiness re-enters the existing size ladder");
    assert.equal(h.loads.length, 2, "readiness rerenders do not restart font loads");
  });
});

test("Grid re-arms the exact displayed title when dirty style or normalized text changes", async () => {
  await withGridFonts(async (h) => {
    await h.show("PUBLIC TITLE");
    await act(async () => { h.loads[0].resolve(); h.loads[1].resolve(); });
    await h.show("PUBLIC TITLE", true);
    assert.equal(h.loads.length, 3, "unchanged bank does not reload");
    assert.match(h.loads[2].spec, /^italic 800 70px /);
    assert.equal(h.loads[2].text, "PUBLIC TITLE*", "load covers the displayed dirty marker");
    assert.deepEqual(h.title().children, ["PUBLIC TITLE*"]);
    assert.equal(h.title().props.fontSize, 50, "new style uses visible fallback until its own load");
    await act(async () => h.loads[2].resolve());
    assert.equal(h.title().props.fontSize, 70);
    await h.show("RECALLED TITLE");
    assert.equal(h.loads.length, 4);
    assert.match(h.loads[3].spec, /^800 70px /);
    assert.equal(h.loads[3].text, "RECALLED TITLE");
    await act(async () => h.loads[3].resolve());
    assert.equal(h.title().props.fontSize, 70, "later text can also trigger refitting after readiness was already true");
  });
});

test("a superseded Grid title load cannot remeasure or replace the newly displayed title", async () => {
  await withGridFonts(async (h) => {
    await h.show("OLD TITLE");
    await act(async () => h.loads[0].resolve());
    await h.show("CURRENT TITLE");
    const measuredBefore = h.measured.length;
    await act(async () => h.loads[1].resolve());
    assert.equal(h.measured.length, measuredBefore, "obsolete title readiness cannot schedule a new measurement");
    assert.deepEqual(h.title().children, ["CURRENT TITLE"]);
    assert.equal(h.title().props.fontSize, 50);
    await act(async () => h.loads[2].resolve());
    assert.equal(h.title().props.fontSize, 70);
  });
});

for (const failure of ["throw", "reject"] as const) {
  test(`Grid font load ${failure} retains a usable visible header without an effect error or retry loop`, async () => {
    await withGridFonts(async (h) => {
      await h.show();
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
      assert.deepEqual(h.title().children, ["PUBLIC TITLE"]);
      assert.deepEqual([h.title().props.x, h.title().props.fontSize], [107, 50]);
      assert.equal(h.loads.length, 2);
      assert.equal(h.measured.length, 2, "failed readiness leaves the current nonzero fallback measurement intact");
    }, failure);
  });
}

test("StrictMode effect replay still permits a current Save modal", async (t) => {
  const saved: string[] = [];
  const props = { ...propsFor("Amp", []), onSaveDevicePreset(name: string) { saved.push(name); } };
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(React.StrictMode, null, React.createElement(CorOsParameterEditor, props))); });
  t.after(() => act(() => renderer.unmount()));
  const content = renderer.root.findAll((node) => typeof node.type === "function" && node.type.name === "CorOsParameterEditorContent")[0];
  act(() => content.props.onContextAction("save-device-preset"));
  act(() => renderer.root.findByProps({ placeholder: "Name" }).props.onChange({ target: { value: "Current" } }));
  await act(async () => renderer.root.findByProps({ className: "is-primary" }).props.onClick());
  assert.deepEqual(saved, ["Current"]);
});

test("live standard-layout controls respect the device's disabled parameter state", (t) => {
  const props = propsFor("Digital Flanger", []);
  props.details.parameters[0].enabled = false;
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(CorOsParameterEditor, props)); });
  t.after(() => act(() => renderer.unmount()));
  const ranges = renderer.root.findAll((node) => node.type === "input" && node.props.type === "range");
  assert.equal(ranges.length, 1);
  assert.equal(ranges[0].props.disabled, true);
});

test("same-address session retirement and an obsolete Looper load cannot revive or overwrite a Save modal", async (t) => {
  const saved: string[] = [];
  let resolve!: (values: number[]) => void;
  const oldLoad = new Promise<number[]>((done) => { resolve = done; });
  let props = { ...propsFor("Looper X", []), editorContextKey: "device-A:preset-lifetime-1",
    onSaveDevicePreset(name: string) { saved.push(name); }, onLoadLooperAssignments: () => oldLoad };
  let renderer!: ReturnType<typeof create>;
  const view = () => React.createElement(CorOsParameterEditor, props);
  act(() => { renderer = create(view()); });
  t.after(() => act(() => renderer.unmount()));
  const begin = (action: string) => renderer.root.findAll((node) => typeof node.type === "function" && node.type.name === "CorOsParameterEditorContent")[0].props.onContextAction(action);
  act(() => begin("save-device-preset"));
  act(() => renderer.root.findByProps({ placeholder: "Name" }).props.onChange({ target: { value: "Old" } }));
  const oldSave = renderer.root.findByProps({ className: "is-primary" }).props.onClick;
  act(() => { props = { ...props, editorContextKey: "device-A:preset-lifetime-2" }; renderer.update(view()); });
  assert.equal(renderer.root.findAllByProps({ "aria-label": "Save device preset" }).length, 0);
  await act(async () => oldSave());
  assert.deepEqual(saved, []);
  act(() => begin("assign-looper-actions"));
  act(() => begin("save-device-preset"));
  act(() => renderer.root.findByProps({ placeholder: "Name" }).props.onChange({ target: { value: "Current" } }));
  assert.equal(renderer.root.findByProps({ className: "is-primary" }).props.disabled, false);
  await act(async () => resolve([0, 1, 2, 3, 4, 5, 6, 7]));
  assert.equal(renderer.root.findAllByProps({ "aria-label": "Reassign Looper X actions" }).length, 0);
  await act(async () => renderer.root.findByProps({ className: "is-primary" }).props.onClick());
  assert.deepEqual(saved, ["Current"]);
});

test("Directory retires setlist selection, menu, and retained actions across session ABA", async (t) => {
  const arranged: unknown[] = [], actions: unknown[] = [], batches: unknown[] = [];
  const entries = [{position:0,location:"1A",name:"Source",instrument:0,occupied:true},{position:1,location:"1B",name:"Destination",instrument:0,occupied:true}];
  let directory = { interactionSessionKey: "transport-A", open:true, loading:false, disabled:false,
    list:{setlistKey:"A",setlistName:"A",presets:entries,folders:[]},
    onClose(){},onRefresh(){},onRecall(){},onSelectSetlist(){},onNewSetlist(){},
    onPresetAction(action: string, entry: unknown){actions.push([action,entry]);},
    onPresetBatchAction(action: string, rows: unknown[]){batches.push([action,rows]);},
    onArrangePreset(source: unknown, destination: unknown){arranged.push([source,destination]);} };
  const view = () => React.createElement(CorOsDirectory, {snapshot:IDLE_PLACEHOLDER_SNAPSHOT,directory});
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(view()); }); t.after(() => act(() => renderer.unmount()));
  const recall = (index: number) => renderer.root.findAll((node) => node.type === "button" && String(node.props.className).startsWith("preset-recall"))[index];
  const multi = () => renderer.root.findByProps({"aria-label":"Multi select presets"}).props.onClick();
  act(() => { multi(); }); act(() => { recall(0).props.onClick(); });
  act(() => { renderer.root.findAllByProps({className:"directory-batch-action"})[1].props.onClick(); });
  const staleDestination = recall(1).props.onClick;
  act(() => { directory={...directory,list:{...directory.list,setlistKey:"B"}}; renderer.update(view()); });
  act(() => { recall(1).props.onClick(); staleDestination(); });
  assert.deepEqual(arranged, []);
  act(() => { multi(); }); act(() => { recall(0).props.onClick(); });
  const staleDelete = renderer.root.findByProps({className:"directory-batch-action is-danger"}).props.onClick;
  act(() => { directory={...directory,interactionSessionKey:"transport-B"}; renderer.update(view()); });
  act(() => { directory={...directory,interactionSessionKey:"transport-A",list:{...directory.list,setlistKey:"A"}}; renderer.update(view()); });
  act(() => { staleDelete(); });
  assert.deepEqual(batches, []);
  assert.equal(renderer.root.findAllByProps({className:"directory-batch-action is-danger"}).length,0);
  act(() => { renderer.root.findByProps({"aria-label":"Open menu for 1A Source"}).props.onClick(); });
  const staleMenu = renderer.root.findAll((node) => node.type === "button" && node.props.role === "menuitem")[0].props.onClick;
  act(() => { directory={...directory,open:false}; renderer.update(view()); });
  act(() => { directory={...directory,open:true}; renderer.update(view()); });
  act(() => { staleMenu(); }); assert.deepEqual(actions, []);
  assert.equal(renderer.root.findAll((node) => node.props.role === "menu").length,0);
  act(() => { multi(); }); act(() => { recall(0).props.onClick(); });
  act(() => { renderer.root.findAllByProps({className:"directory-batch-action"})[1].props.onClick(); });
  act(() => { recall(1).props.onClick(); });
  assert.deepEqual(arranged, [[entries[0], entries[1]]], "current setlist remains usable");
});
test("MIDI Clear-all authorization belongs to its actual workflow host and preset lifetime", async (t) => {
  let writes = 0, epoch = 0;
  const gateway = { async setMidiOut() { writes++; return {detail:"Cleared"}; } };
  let snapshot = {...IDLE_PLACEHOLDER_SNAPSHOT, presetIdentity:"midi-original", midiOut:[{source:0,messages:[{type:1,channel:1,param1:1,param2:2,param3:0}]}]};
  const captureSessionCurrent = () => { const captured = epoch; return () => captured === epoch; };
  function Probe() {
    const workflow = usePresetMidiOutWorkflow({gateway, snapshot, captureSessionCurrent});
    return React.createElement(PresetMidiOutEditor, {workflow,onClose(){}});
  }
  let renderer: ReturnType<typeof create> | undefined;
  const oldFrame = Object.getOwnPropertyDescriptor(globalThis, "requestAnimationFrame");
  const oldCancel = Object.getOwnPropertyDescriptor(globalThis, "cancelAnimationFrame");
  const frames = new Set<ReturnType<typeof setTimeout>>();
  Object.defineProperty(globalThis, "requestAnimationFrame", {configurable:true,value:(callback: (time: number) => void) => {
    const timer = setTimeout(() => { frames.delete(timer); callback(0); },0); frames.add(timer); return timer;
  }});
  Object.defineProperty(globalThis, "cancelAnimationFrame", {configurable:true,value:(timer: ReturnType<typeof setTimeout>) => { clearTimeout(timer); frames.delete(timer); }});
  try {
  await act(async () => { renderer = create(React.createElement(React.StrictMode, null, React.createElement(Probe))); });
  const open = () => renderer!.root.findByProps({"aria-label":"Clear MIDI assignments"}).props.onClick();
  const confirm = () => renderer!.root.findByProps({confirmLabel:"Clear all"});
  await act(async () => { open(); }); const oldHost = confirm().props.onConfirm;
  epoch++; await act(async () => { oldHost(); }); assert.equal(writes,0,"host retirement refuses even before rerender");
  await act(async () => { renderer!.update(React.createElement(React.StrictMode, null, React.createElement(Probe))); });
  assert.equal(renderer!.root.findAllByProps({confirmLabel:"Clear all"}).length,0);
  await act(async () => { oldHost(); }); assert.equal(writes,0);
  await act(async () => { open(); }); const oldPreset = confirm().props.onConfirm;
  snapshot={...snapshot,presetIdentity:"midi-replacement"};
  await act(async () => { renderer!.update(React.createElement(React.StrictMode, null, React.createElement(Probe))); });
  await act(async () => { open(); }); const current = confirm().props.onConfirm;
  await act(async () => { oldPreset(); }); assert.equal(writes,0);
  assert.equal(renderer!.root.findAllByProps({confirmLabel:"Clear all"}).length,1,"old completion must not close new confirmation");
  await act(async () => { current(); current(); }); assert.equal(writes,1,"current confirmation dispatches once");
  await act(async () => { open(); }); const cancelled = confirm().props.onConfirm;
  await act(async () => { confirm().props.onCancel(); open(); });
  await act(async () => { cancelled(); }); assert.equal(writes,1,"cancelled token cannot authorize a reopened dialog");
  assert.equal(renderer!.root.findAllByProps({confirmLabel:"Clear all"}).length,1);
  const unmounted = confirm().props.onConfirm;
  await act(async () => { renderer!.unmount(); });
  await act(async () => { unmounted(); }); assert.equal(writes,1);
  } finally {
    if (renderer) await act(async () => { renderer!.unmount(); });
    for (const frame of frames) clearTimeout(frame);
    if (oldFrame) Object.defineProperty(globalThis,"requestAnimationFrame",oldFrame); else Reflect.deleteProperty(globalThis,"requestAnimationFrame");
    if (oldCancel) Object.defineProperty(globalThis,"cancelAnimationFrame",oldCancel); else Reflect.deleteProperty(globalThis,"cancelAnimationFrame");
  }
});

test("Modes Configuration refuses unrelated drop payloads and admits only a current valid mode index", (t) => {
  const commits: number[][] = [];
  const workflow = { slots: [0, 1, 2], loading: false, pending: false, refresh() {}, commit(next: number[]) { commits.push(next); } };
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(ModesConfigurationScreen, { workflow, onClose() {} })); });
  t.after(() => act(() => renderer.unmount()));
  const target = () => renderer.root.findAll((node) => node.type === "button" && node.props.draggable !== undefined)[1];
  const dragData = new Map<string, string>();
  act(() => renderer.root.findAll((node) => node.type === "button" && node.props.draggable !== undefined)[0].props.onDragStart({
    preventDefault() {}, dataTransfer: { setData(type: string, value: string) { dragData.set(type, value); }, effectAllowed: "" }
  }));
  const drop = (payload: string, context = dragData.get("text/qc-mode-context")) => target().props.onDrop({ preventDefault() {}, dataTransfer: { getData(type: string) {
    if (type === "text/qc-mode-context") return context ?? "";
    assert.equal(type, "text/qc-mode-index"); return payload;
  } } });
  act(() => { drop("0", ""); drop("0", "another editor session"); });
  assert.deepEqual(commits, [], "a valid index without this drag's editor context is refused");
  act(() => { for (const payload of ["", "text", "-1", "1.2", "3", "100000000000000000000"]) drop(payload); });
  assert.deepEqual(commits, [], "unrelated/file/text drops cannot silently select mode zero");
  act(() => drop("0"));
  assert.equal(commits.length, 1);
  assert.equal(commits[0].length, 2);
  assert.ok(commits[0].includes(2), "untouched third mode remains in the cycle");
  act(() => renderer.update(React.createElement(ModesConfigurationScreen, { workflow: { ...workflow, pending: true }, onClose() {} })));
  act(() => drop("0"));
  assert.equal(commits.length, 1, "direct disabled drop handler cannot commit");
});
test("specialized Cab and IR controls refuse disabled operands and settle current speaker gestures", (t) => {
  const drafts: number[] = [], commits: number[] = [], batches: unknown[] = [];
  const parameter = (index: number, enabled: boolean) => ({ index, name: `Parameter ${index}`, type: "float", options: index === 1 || index === 2 ? ["A", "B"] : [],
    steps: 101, normalizedValue: .2, writable: true, enabled, minimum: 0, maximum: 1, units: "", displayValue: ".2" });
  let props = { ...propsFor("IR Loader", []), details: { ...propsFor("IR Loader", []).details, name: "IR Loader", category: "IR Loader",
    parameters: [0,1,2].map((index) => parameter(index, false)) },
    onDraftChange(item: {index:number}) { drafts.push(item.index); }, onCommit(item: {index:number}) { commits.push(item.index); },
    onCommitBatch(changes: unknown[]) { batches.push(changes); } };
  const view = () => React.createElement(CorOsParameterEditor, props);
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(view()); }); t.after(() => act(() => renderer.unmount()));
  const click = (label: string) => renderer.root.findByProps({"aria-label":label}).props.onClick();
  act(() => { click("Disable impulse"); click("Invert impulse phase"); click("Next impulse response"); });
  assert.deepEqual(drafts, []); assert.deepEqual(commits, []);
  act(() => { props={...props,details:{...props.details,parameters:[0,1,2].map((index) => parameter(index,true))}}; renderer.update(view()); });
  act(() => { click("Disable impulse"); click("Invert impulse phase"); click("Next impulse response"); });
  assert.deepEqual(commits,[0,1,2]); drafts.length=0; commits.length=0;
  act(() => { props={...props,details:{...props.details,name:"Cab",category:"Guitar Cab",parameters:[0,1,2,3,4,5,6].map((index) => parameter(index,false))}}; renderer.update(view()); });
  const speaker = () => renderer.root.findAll((node) => node.type === "button" && node.props.className === "cab-speaker")[0];
  const pointer = { pointerId:1, clientX:70, clientY:35, currentTarget: {setPointerCapture(){},hasPointerCapture(){return false;},getBoundingClientRect(){return {left:0,top:0,width:100,height:100};}} };
  act(() => { click("Disable cab microphone 1"); click("Invert microphone 1 phase"); click("Next microphone for slot 1"); speaker().props.onPointerDown(pointer); speaker().props.onPointerUp(pointer); });
  assert.deepEqual(drafts,[]); assert.deepEqual(commits,[]); assert.deepEqual(batches,[]);
  act(() => { props={...props,details:{...props.details,parameters:props.details.parameters.map((item) => ({...item,enabled:true}))}}; renderer.update(view()); });
  act(() => { click("Disable cab microphone 1"); click("Invert microphone 1 phase"); click("Next microphone for slot 1"); });
  assert.deepEqual(commits,[0,6,1]); commits.length=0;
  act(() => { speaker().props.onPointerDown(pointer); speaker().props.onPointerCancel(pointer); });
  assert.equal(batches.length,1,"current cancellation settles the already-previewed value, as the rotary gesture contract requires");
  batches.length=0;
  act(() => speaker().props.onPointerDown(pointer)); drafts.length=0;
  act(() => { props={...props,details:{...props.details,parameters:props.details.parameters.map((item) => ({...item,enabled:false}))}}; renderer.update(view()); });
  act(() => { speaker().props.onPointerMove(pointer); speaker().props.onPointerUp(pointer); });
  assert.deepEqual(drafts,[]); assert.deepEqual(batches,[],"fresh disabled operands retire an active drag before its write");
  act(() => { props={...props,details:{...props.details,parameters:props.details.parameters.map((item) => ({...item,enabled:true}))}}; renderer.update(view()); });
  act(() => { speaker().props.onPointerDown(pointer); speaker().props.onPointerUp(pointer); });
  assert.equal(batches.length,1,"current writable speaker remains usable");
});
test("rotary value rejects a newly disabled operand without retaining hidden drag increments", (t) => {
  let rotary: any;
  const drafts: number[] = [];
  const commits: number[] = [];
  function Harness({ enabled }: { enabled: boolean }) {
    rotary = components.useRotaryValue({ value: .2, step: .01, quantize: (value: number) => Math.round(value * 100) / 100,
      pixelsPerStep: 7, disabled: !enabled,
      onDraftChange: (value: number) => { if (enabled) drafts.push(value); },
      onCommit: (value: number) => { if (enabled) commits.push(value); } });
    return null;
  }
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(Harness, { enabled: true })); });
  t.after(() => act(() => renderer.unmount()));
  const target = { setPointerCapture() {}, hasPointerCapture: () => false };
  const pointer = (y: number) => ({ pointerId: 1, clientY: y, pointerType: "touch", currentTarget: target });
  act(() => rotary.onPointerDown(pointer(100)));
  act(() => renderer.update(React.createElement(Harness, { enabled: false })));
  act(() => { rotary.onPointerMove(pointer(79)); rotary.onPointerUp(pointer(79)); });
  assert.deepEqual(drafts, [], "disabled operand rejects all three drag steps");
  act(() => renderer.update(React.createElement(Harness, { enabled: true })));
  act(() => { rotary.onKeyDown({ key: "ArrowRight", preventDefault() {} }); rotary.onKeyUp({ key: "ArrowRight" }); });
  assert.equal(drafts.at(-1), .21, "one healthy step starts at the displayed .2, not a rejected hidden .23");
  assert.equal(commits.at(-1), .21);
});
test("unit mirror retires the old owner picture and delayed capture across physical sessions", async (t) => {
  const header = Buffer.alloc(24); header.write("PNG", 1); header.writeUInt32BE(800, 16); header.writeUInt32BE(480, 20);
  const frame = (name: string) => ({ pngBase64: Buffer.concat([header, Buffer.from(name)]).toString("base64") });
  let physical = "A", completeOld!: (value: ReturnType<typeof frame>) => void;
  const captureSessionCurrent = () => { const owner = physical; return () => owner === physical; };
  const options = (key: string, captureScreen: () => Promise<ReturnType<typeof frame>>) => ({
    screenGestureContextKey: key, captureSessionCurrent, mirror: { captureScreen },
    destination: { view: "unit-mirror", captureCurrent: captureSessionCurrent }, workflows: { deviceSettings: { open: false }, tempo: { open: false } }
  });
  const show = (key: string, captureScreen: () => Promise<ReturnType<typeof frame>>) => components.qcDeviceScreenContent(options(key, captureScreen));
  let renderer!: ReturnType<typeof create>;
  await act(async () => { renderer = create(show("A", async () => frame("first"))); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  const images = () => renderer.root.findAllByType("img").map((image) => image.props.src);
  assert.deepEqual(images(), [`data:image/png;base64,${frame("first").pngBase64}`]);
  await act(async () => { renderer.update(show("A", () => new Promise((resolve) => { completeOld = resolve; }))); });
  physical = "B";
  await act(async () => { renderer.update(show("B", async () => { throw new Error("new device capture unavailable"); })); });
  assert.deepEqual(images(), [], "a failed current-owner read cannot retain the old owner's picture");
  await act(async () => { completeOld(frame("late-old")); });
  assert.deepEqual(images(), [], "old in-flight capture cannot publish into the replacement owner");
  await act(async () => { renderer.update(show("B", async () => frame("current"))); });
  assert.deepEqual(images(), [`data:image/png;base64,${frame("current").pngBase64}`]);
  await act(async () => { renderer.update(show("B", async () => frame("refresh"))); });
  assert.deepEqual(images(), [`data:image/png;base64,${frame("refresh").pngBase64}`], "same-owner refreshed captures remain usable");
});
test("Tempo wheel settlement cannot cross preset, scene, mode or physical owners", async (t) => {
  const writes: string[] = [];
  let physical = "A";
  const captureSessionCurrent = () => { const owner = physical; return () => owner === physical; };
  const show = (preset: string, scene = 0, mode = "PRESET", physicalKey = physical) => components.qcDeviceScreenContent({
    screenGestureContextKey: physicalKey, captureSessionCurrent,
    snapshot: { ...IDLE_PLACEHOLDER_SNAPSHOT, presetIdentity: preset, activeScene: scene, tempo: 120 },
    destination: { view: null }, workflows: { deviceSettings: { open: false }, tempo: {
      open: true, settings: { mode, globalBpm: 90 }, pending: false, loading: false,
      setGlobalBpm: (value: number) => writes.push(`global:${value}`)
    } }, queueTempo: (value: number) => writes.push(`${preset}:${scene}:${value}`), selectScene() {}
  });
  let renderer!: ReturnType<typeof create>;
  await act(async () => { renderer = create(show("preset-A")); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  const wheel = () => renderer.root.findByProps({ role: "slider", "aria-label": "Tempo" }).props.onWheel({ deltaY: -1, preventDefault() {} });
  const settle = async () => { await act(async () => { await new Promise((resolve) => setTimeout(resolve, 240)); }); };
  act(wheel);
  await act(async () => renderer.update(show("preset-B")));
  await settle();
  assert.deepEqual(writes, [], "old wheel draft must not become a write to the recalled preset");
  act(wheel);
  await act(async () => renderer.update(show("preset-B", 1)));
  await settle();
  assert.deepEqual(writes, [], "scene replacement retires the old draft");
  act(wheel);
  await act(async () => renderer.update(show("preset-B", 1, "GLOBAL")));
  await settle();
  assert.deepEqual(writes, [], "PRESET draft cannot settle through a new GLOBAL callback");
  act(wheel);
  await settle();
  assert.deepEqual(writes, ["global:91"], "healthy same-owner wheel still settles normally");
  writes.length = 0;
  act(wheel);
  physical = "B"; // Host transport guard retires before the next React render.
  await settle();
  assert.deepEqual(writes, [], "physical replacement refuses a pending settle even before rerender");
  await act(async () => renderer.update(show("preset-B", 1, "GLOBAL")));
  act(wheel);
  await settle();
  assert.deepEqual(writes, ["global:91"], "new physical owner gets a fresh usable draft");
});
