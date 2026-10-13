import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { CONNECTING_PLACEHOLDER_SNAPSHOT, IDLE_PLACEHOLDER_SNAPSHOT, connectingPlaceholderSnapshot, isConnectingPlaceholder } from "../src/device-placeholder.ts";
import { qcParameterEditorLeds } from "../src/qc-parameter-editor-leds.ts";
import type { QcSurfaceActionOptions } from "../src/use-qc-surface-actions.ts";

// Load the hook only after the test loader shares React with react-test-renderer.
register("./support/react-singleton-loader.mjs", import.meta.url);
const { createElement } = (await import("react")).default;
const { useQcSurfaceActions } = await import("../src/use-qc-surface-actions.ts");

test("the connecting placeholder carries one wording per phase, and is told apart from a real preset", () => {
  assert.equal(connectingPlaceholderSnapshot("opening").presetName, "Opening the Quad Cortex…");
  assert.equal(connectingPlaceholderSnapshot("handshaking").presetName, "Handshaking with the Quad Cortex…");
  assert.equal(connectingPlaceholderSnapshot("synchronizing").presetName, "Synchronizing the Quad Cortex…");
  for (const snapshot of [CONNECTING_PLACEHOLDER_SNAPSHOT, connectingPlaceholderSnapshot("opening"), connectingPlaceholderSnapshot("synchronizing")]) assert.ok(isConnectingPlaceholder(snapshot));
  assert.ok(!isConnectingPlaceholder(IDLE_PLACEHOLDER_SNAPSHOT), "the idle placeholder is not a connection in progress");
  assert.ok(!isConnectingPlaceholder({ presetName: "Opening the Quad Cortex…", presetLocation: "1A" }), "a real preset that happens to share the name is not a placeholder");
});

const editor = (category: string, page: number, parameters = 10) => ({
  details: { name: "Block", category, parameters: Array.from({ length: parameters }, (_, index) => ({ index, name: `P${index}`, normalizedValue: 0.5 })) },
  accent: "#123456",
  page
}) as unknown as Parameters<typeof qcParameterEditorLeds>[0];

test("the parameter-editor LEDs have one derivation: ten lamps, lit where the page has a control", () => {
  assert.equal(qcParameterEditorLeds(undefined), undefined);
  const leds = qcParameterEditorLeds(editor("Amp", 0))!;
  assert.equal(leds.length, 10);
  assert.ok(leds.every((led) => led.active === led.assigned));
  assert.ok(leds.some((led) => led.active), "at least one control is lit");
  assert.equal(new Set(leds.map((led) => led.color)).size, 1, "a plain block is one accent");
  const cab = qcParameterEditorLeds(editor("Cab", 0))!;
  assert.equal(new Set([5, 6, 7, 8].map((index) => cab[index].color)).size, 1);
  assert.notEqual(cab[5].color, cab[0].color, "the cab's microphone controls are drawn in their own colour");
  assert.equal(qcParameterEditorLeds(editor("Cab", 1))![2].color, cab[5].color);
});

function mountActions(demo: boolean, role: string, kind: "rotary" | "switch" = "switch") {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const notices: string[] = [];
  const options = {
    snapshot: { blocks: [] }, selectedBlockId: "", blockDetails: undefined,
    grid: {}, performance: {},
    openBlock() {}, closeBlock() {},
    rotation: { editor: () => false, tempo() {}, masterVolume() {} },
    demo, notice: (message: string) => { notices.push(message); }
  } as unknown as QcSurfaceActionOptions;
  let handler!: ReturnType<typeof useQcSurfaceActions>;
  let renderer!: ReactTestRenderer;
  function Probe() { handler = useQcSurfaceActions(options); return null; }
  act(() => { renderer = create(createElement(Probe)); });
  return { notices, handler, renderer, role, kind };
}

test("controls without a verified action explain themselves in one wording on every host", () => {
  const live = mountActions(false, "bank:up");
  live.handler({ kind: "switch", role: "pedal:x", phase: "release" } as never);
  live.handler({ kind: "switch", role: "power", phase: "release" } as never);
  live.handler({ kind: "switch", role: "pedal:x", phase: "press" } as never);
  assert.deepEqual(live.notices, ["pedal:x has no verified action on the current screen.", "Power and lock actions are intentionally available only on the physical Quad Cortex."]);
  live.renderer.unmount();
  const demo = mountActions(true, "x");
  demo.handler({ kind: "switch", role: "pedal:x", phase: "release" } as never);
  assert.deepEqual(demo.notices, ["Demo switch: pedal:x. Hardware was not changed."]);
  demo.renderer.unmount();
});

test("the assembled Looper screen receives the host session probe before opening its editor", async () => {
  const { createQcDeviceDisplay } = await import("../src/qc-device-display.tsx");
  const log: string[] = [];
  let current = true;
  let resolve!: (accepted: boolean) => void;
  const command = new Promise<boolean>((done) => { resolve = done; });
  const host = {
    captureSessionCurrent: () => () => current,
    snapshot: { blocks: [{ modelId: 27_000, name: "Looper X" }], scenes: [], activeScene: 0 },
    destination: { view: "looper-editor", setView: () => log.push("view"), setPopup() {}, latch: {} },
    workflows: {
      deviceSettings: { open: false }, tempo: { open: false }, looper: { run: async () => command },
      grid: { openBlock: async () => log.push("editor") }, scene: {}, preset: {}, routing: {}
    },
    views: {}, cpu: { monitor: () => null }, undo: {},
    setParameterMenuInitiallyOpen: () => log.push("menu"), notice: () => log.push("notice")
  } as unknown as Parameters<typeof createQcDeviceDisplay>[0];
  const display = createQcDeviceDisplay(host);
  const screen = display.surface.screenContent as { props: { onParameters(): Promise<void> } };
  const pending = screen.props.onParameters();
  current = false;
  resolve(true);
  await pending;
  assert.deepEqual(log, [], "the production display passes its probe to the delayed Looper action");
});
