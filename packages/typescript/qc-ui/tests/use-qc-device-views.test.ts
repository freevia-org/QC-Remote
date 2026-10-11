import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import type { QcDeviceViews, QcDeviceViewsOptions } from "../src/use-qc-device-views.ts";

// Load the hook only after the test loader shares React with react-test-renderer.
register("./support/react-singleton-loader.mjs", import.meta.url);
const { createElement } = (await import("react")).default;
const { useQcDeviceViews } = await import("../src/use-qc-device-views.ts");

interface World {
  view: string | null;
  demo: boolean;
  hardwareReady: boolean;
  sessionReady: boolean;
  showResult: boolean;
  setlistKey: string;
  presetList?: { setlistKey: string };
  ioFails: boolean;
  identityFails: boolean;
}

function mount(overrides: Partial<World> = {}) {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const log: string[] = [];
  const world: World = { view: null, demo: false, hardwareReady: true, sessionReady: true, showResult: true, setlistKey: "set-a", presetList: undefined, ioFails: false, identityFails: false, ...overrides };
  const options = (): QcDeviceViewsOptions => ({
    destination: { get view() { return world.view as never; }, setView: ((next: unknown) => { world.view = typeof next === "function" ? (next as (current: string | null) => string | null)(world.view) : next as string | null; log.push(`view:${world.view}`); }) as never },
    snapshot: { setlistKey: world.setlistKey, deviceName: "Rig" },
    session: { demo: world.demo, hardwareReady: world.hardwareReady, sessionReady: world.sessionReady },
    gateway: {
      identity: async () => { if (world.identityFails) throw new Error("identity failed"); return { serial: "S1", customName: "", appFwVersion: "3.1" } as never; },
      tapScreen: async (x: number, y: number) => { log.push(`tap:${x},${y}`); },
      longTapScreen: async (x: number, y: number) => { log.push(`long:${x},${y}`); },
      swipeScreen: async (x: number, y: number, toX: number, toY: number) => { log.push(`swipe:${x},${y},${toX},${toY}`); }
    } as never,
    workflows: {
      performance: { showDeviceView: async (view, visible = true) => { log.push(`show:${view}:${visible}`); return world.showResult; } },
      tuner: { openTuner: () => { log.push("tuner.open"); } },
      looper: { run: async (command) => { log.push(`looper:${command}`); } },
      ioEq: { refresh: async () => { log.push("io.refresh"); if (world.ioFails) throw new Error("io failed"); } },
      preset: { presetList: world.presetList, loadDirectory: (...args) => { log.push(`load:${args.join(",")}`); } }
    },
    notice: (message) => { log.push(`notice:${message}`); },
    fail: (error) => { log.push(`fail:${(error as Error).message}`); }
  });
  const seen: { current?: QcDeviceViews } = {};
  function Probe() { seen.current = useQcDeviceViews(options()); return null; }
  let renderer!: ReactTestRenderer;
  act(() => { renderer = create(createElement(Probe)); });
  const rerender = () => act(() => renderer.update(createElement(Probe)));
  return { log, world, rerender, get views() { return seen.current!; }, renderer };
}

test("opening Gig View goes through the shared command, then loads the setlist the Gig list needs", async () => {
  const host = mount();
  await act(async () => { await host.views.open("gig"); });
  assert.deepEqual(host.log, ["show:gig:true", "view:gig", "load:false,set-a,true"]);
  host.log.length = 0;
  host.world.view = null;
  host.world.presetList = { setlistKey: "set-a" };
  host.rerender();
  await act(async () => { await host.views.open("gig"); });
  assert.deepEqual(host.log, ["show:gig:true", "view:gig"], "a current list is not reloaded");
  host.log.length = 0;
  host.world.presetList = { setlistKey: "other" };
  host.rerender();
  await act(async () => { await host.views.open("gig"); });
  assert.ok(host.log.includes("load:false,set-a,true"), "a list from another setlist is reloaded");
  host.renderer.unmount();
});

test("a refused open changes nothing in the app", async () => {
  const host = mount({ showResult: false });
  await act(async () => { await host.views.open("gig"); await host.views.open("tuner"); });
  assert.deepEqual(host.log, ["show:gig:true", "show:tuner:true"]);
  assert.equal(host.world.view, null);
  host.renderer.unmount();
});

test("opening the Tuner starts the tuner workflow and shows its screen", async () => {
  const host = mount();
  await act(async () => { await host.views.open("tuner"); });
  assert.deepEqual(host.log, ["show:tuner:true", "tuner.open", "view:tuner"]);
  host.renderer.unmount();
});

test("Gig View toggles: tapping it again closes it on the unit and in the app", async () => {
  const host = mount();
  await act(async () => { await host.views.toggle("gig"); });
  assert.equal(host.world.view, "gig");
  host.log.length = 0;
  await act(async () => { await host.views.toggle("gig"); });
  assert.deepEqual(host.log, ["show:gig:false", "view:null"]);
  host.renderer.unmount();
});

test("closing keeps the app's screen when the unit refused, unless the unit is gone", async () => {
  const refused = mount({ view: "gig", showResult: false });
  await act(async () => { await refused.views.close(); });
  assert.equal(refused.world.view, "gig", "the app must not show the Grid while the unit still shows Gig View");
  refused.renderer.unmount();
  const gone = mount({ view: "tuner", showResult: false, hardwareReady: false, sessionReady: false });
  await act(async () => { await gone.views.close(); });
  assert.equal(gone.world.view, null, "with no unit to tell, the screen must still be able to close");
  gone.renderer.unmount();
  const demo = mount({ view: "gig", demo: true, hardwareReady: false });
  await act(async () => { await demo.views.close(); });
  assert.deepEqual(demo.log, ["show:gig:false", "view:null"], "a demo device is projected by the shared command");
  demo.renderer.unmount();
});

test("I/O toggles, opens only after the unit's settings were read, and closes the same way on every host", async () => {
  const host = mount();
  await act(async () => { await host.views.toggle("io"); });
  assert.deepEqual(host.log, ["io.refresh", "view:io-overview", "notice:I/O Settings opened."]);
  host.log.length = 0;
  await act(async () => { await host.views.toggle("io"); });
  assert.deepEqual(host.log, ["view:null"], "the unit has no I/O screen command; closing is app-only");
  host.renderer.unmount();
  const demo = mount({ demo: true, hardwareReady: false });
  await act(async () => { await demo.views.toggle("io"); });
  demo.log.push("--");
  await act(async () => { await demo.views.toggle("io"); });
  assert.deepEqual(demo.log, ["view:io-overview", "notice:Demo: I/O Settings opened.", "--", "view:null", "notice:Demo: I/O Settings closed."]);
  demo.renderer.unmount();
});

test("I/O is refused without a unit and reports a failed read without opening", async () => {
  const offline = mount({ hardwareReady: false });
  await act(async () => { await offline.views.open("io"); });
  assert.deepEqual(offline.log, ["notice:Connect the Quad Cortex before opening I/O Settings."]);
  offline.renderer.unmount();
  const failing = mount({ ioFails: true });
  await act(async () => { await failing.views.open("io"); });
  assert.deepEqual(failing.log, ["io.refresh", "fail:io failed"]);
  assert.equal(failing.world.view, null);
  failing.renderer.unmount();
});

test("toggling I/O from the Global EQ screen is not a close", async () => {
  const host = mount({ view: "global-eq" });
  await act(async () => { await host.views.toggle("io"); });
  assert.equal(host.world.view, "io-overview", "only the I/O screens themselves count as the open I/O view");
  host.renderer.unmount();
});

test("Looper X closes by telling the unit first, then clearing the app's screen", async () => {
  const host = mount({ view: "looper-editor" });
  await act(async () => { await host.views.close(); });
  assert.deepEqual(host.log, ["looper:close", "view:null"]);
  host.renderer.unmount();
});

test("Device Information needs a session, reads the unit's identity and falls back to the app's device name", async () => {
  const offline = mount({ sessionReady: false });
  await act(async () => { await offline.views.open("info"); });
  assert.deepEqual(offline.log, ["notice:Connect a Quad Cortex or start the Demo device before opening Device Information."]);
  offline.renderer.unmount();
  const host = mount();
  await act(async () => { await host.views.open("info"); });
  assert.equal(host.world.view, "settings-info");
  assert.deepEqual(host.views.deviceInformation, { serial: "S1", deviceName: "Rig", corOsVersion: "3.1" });
  host.renderer.unmount();
  const failing = mount({ identityFails: true });
  await act(async () => { await failing.views.open("info"); });
  assert.deepEqual(failing.log, ["view:settings-info", "fail:identity failed"]);
  failing.renderer.unmount();
});

test("gestures on a device screen reach the unit, are dropped for a demo, and are reported when the unit is gone", async () => {
  const host = mount();
  await act(async () => {
    await host.views.forwardGesture(1, 2, 0, 0, "tap");
    await host.views.forwardGesture(3, 4, 0, 0, "long-tap");
    await host.views.forwardGesture(5, 6, 7, 8, "swipe");
  });
  assert.deepEqual(host.log, ["tap:1,2", "long:3,4", "swipe:5,6,7,8"]);
  host.renderer.unmount();
  const demo = mount({ demo: true, hardwareReady: false });
  await act(async () => { await demo.views.forwardGesture(1, 2, 0, 0, "tap"); });
  assert.deepEqual(demo.log, []);
  demo.renderer.unmount();
  const gone = mount({ hardwareReady: false });
  await act(async () => { await gone.views.forwardGesture(1, 2, 0, 0, "tap"); });
  assert.deepEqual(gone.log, ["notice:The Quad Cortex disconnected before the screen gesture could be sent."]);
  gone.renderer.unmount();
});

test("the workflows are stable across renders and always act on the latest session", async () => {
  const host = mount();
  const first = host.views;
  host.rerender();
  assert.equal(host.views.open, first.open);
  assert.equal(host.views.close, first.close);
  assert.equal(host.views.toggle, first.toggle);
  host.world.hardwareReady = false;
  host.rerender();
  await act(async () => { await first.forwardGesture(1, 2, 0, 0, "tap"); });
  assert.deepEqual(host.log, ["notice:The Quad Cortex disconnected before the screen gesture could be sent."]);
  host.renderer.unmount();
});
