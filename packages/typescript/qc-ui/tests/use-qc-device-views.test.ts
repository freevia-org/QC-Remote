import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { createQcScreenLatch } from "../src/qc-unit-screens.ts";
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
  /** The class at the root of the unit's graphics tree: what the unit is really showing. */
  unit: "Grid" | "CheatSheetDialog" | "ParameterEditor" | "TunerDialog";
  /** The unit ignores the gesture (a wrong state or a lost frame). */
  unitIgnoresGestures: boolean;
  /** Extra text the unit's tree carries (a label on the screen). */
  treeText?: string;
}

function mount(overrides: Partial<World> = {}) {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const log: string[] = [];
  const world: World = { view: null, demo: false, hardwareReady: true, sessionReady: true, showResult: true, setlistKey: "set-a", presetList: undefined, ioFails: false, identityFails: false, unit: "Grid", unitIgnoresGestures: false, ...overrides };
  const latch = createQcScreenLatch();
  const options = (): QcDeviceViewsOptions => ({
    destination: { latch, get view() { return world.view as never; }, setView: ((next: unknown) => { world.view = typeof next === "function" ? (next as (current: string | null) => string | null)(world.view) : next as string | null; log.push(`view:${world.view}`); }) as never },
    snapshot: { setlistKey: world.setlistKey, deviceName: "Rig" },
    session: { demo: world.demo, hardwareReady: world.hardwareReady, sessionReady: world.sessionReady },
    gateway: {
      identity: async () => { if (world.identityFails) throw new Error("identity failed"); return { serial: "S1", customName: "", appFwVersion: "3.1" } as never; },
      graphicsTree: async () => ({ tree: `zenUI::RootGraphicsItem\nzenUI::${world.unit}${world.treeText ? ` text '${world.treeText}'` : ""}` }),
      tapScreen: async (x: number, y: number) => { log.push(`tap:${x},${y}`); if (!world.unitIgnoresGestures && world.unit === "CheatSheetDialog") world.unit = "Grid"; },
      longTapScreen: async (x: number, y: number) => { log.push(`long:${x},${y}`); },
      swipeScreen: async (x: number, y: number, toX: number, toY: number) => { log.push(`swipe:${x},${y},${toX},${toY}`); if (!world.unitIgnoresGestures && world.unit === "Grid" && Math.abs(toY - y) > 245) world.unit = "CheatSheetDialog"; }
    } as never,
    screenPoll: { intervalMs: 1, timeoutMs: 6 },
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
  return { log, world, latch, rerender, get views() { return seen.current!; }, renderer };
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
  assert.equal(host.latch.takeUnit("gig"), false, "a refused command withdraws what it expected the unit to show");
  assert.equal(host.latch.takeUnit("tuner"), false);
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

test("I/O toggles through the unit: a long drag from the Grid opens it, a tap on its check button closes it", async () => {
  const host = mount();
  await act(async () => { await host.views.toggle("io"); });
  assert.deepEqual(host.log, ["swipe:400,100,400,440", "io.refresh", "view:io-overview", "notice:I/O Settings opened."]);
  assert.equal(host.world.unit, "CheatSheetDialog");
  host.log.length = 0;
  await act(async () => { await host.views.toggle("io"); });
  assert.deepEqual(host.log, ["tap:743,30", "view:null"]);
  assert.equal(host.world.unit, "Grid");
  host.renderer.unmount();
  const demo = mount({ demo: true, hardwareReady: false });
  await act(async () => { await demo.views.toggle("io"); });
  demo.log.push("--");
  await act(async () => { await demo.views.toggle("io"); });
  assert.deepEqual(demo.log, ["view:io-overview", "notice:Demo: I/O Settings opened.", "--", "view:null", "notice:Demo: I/O Settings closed."], "a demo device is never sent a gesture");
  demo.renderer.unmount();
});

test("the unit is only ever sent the I/O gesture from its Grid, and only the check tap while it shows I/O", async () => {
  const already = mount({ unit: "CheatSheetDialog" });
  await act(async () => { await already.views.open("io"); });
  assert.deepEqual(already.log, ["io.refresh", "view:io-overview", "notice:I/O Settings opened."], "an I/O dialog the unit already shows needs no gesture");
  already.renderer.unmount();
  for (const unit of ["ParameterEditor", "TunerDialog"] as const) {
    const busy = mount({ unit });
    await act(async () => { await busy.views.open("io"); });
    assert.deepEqual(busy.log, ["notice:Close the screen open on the Quad Cortex before opening I/O Settings."], `${unit}: no gesture over another screen`);
    assert.equal(busy.world.view, null);
    assert.equal(busy.latch.takeUnit("io"), false, `${unit}: a refused open leaves no expectation that would swallow the unit's own I/O Settings`);
    busy.renderer.unmount();
  }
  const closed = mount({ view: "io-overview", unit: "Grid" });
  await act(async () => { await closed.views.close(); });
  assert.deepEqual(closed.log, ["view:null"], "the unit already left I/O (it was closed on the unit): no tap");
  closed.renderer.unmount();
});

test("I/O reports a unit that ignores the gesture and keeps the app's screen in step with the unit", async () => {
  const ignored = mount({ unitIgnoresGestures: true });
  await act(async () => { await ignored.views.open("io"); });
  assert.deepEqual(ignored.log, ["swipe:400,100,400,440", "notice:The Quad Cortex did not show I/O Settings."]);
  assert.equal(ignored.world.view, null, "the app never shows I/O while the unit does not");
  ignored.renderer.unmount();
  const stuck = mount({ view: "io-overview", unit: "CheatSheetDialog", unitIgnoresGestures: true });
  await act(async () => { await stuck.views.close(); });
  assert.deepEqual(stuck.log, ["tap:743,30", "tap:743,30", "notice:The Quad Cortex did not close I/O Settings."], "one more tap, then it is reported");
  assert.equal(stuck.world.view, "io-overview", "the app does not claim a close the unit did not make");
  stuck.renderer.unmount();
});

test("the I/O family includes Global EQ: toggling from it closes the unit's I/O dialog", async () => {
  const host = mount({ view: "global-eq", unit: "CheatSheetDialog" });
  await act(async () => { await host.views.toggle("io"); });
  assert.deepEqual(host.log, ["tap:743,30", "view:null"]);
  host.renderer.unmount();
});

test("I/O is refused without a unit and reports a failed read without opening", async () => {
  const offline = mount({ hardwareReady: false });
  await act(async () => { await offline.views.open("io"); });
  assert.deepEqual(offline.log, ["notice:Connect the Quad Cortex before opening I/O Settings."]);
  offline.renderer.unmount();
  const failing = mount({ ioFails: true });
  await act(async () => { await failing.views.open("io"); });
  assert.deepEqual(failing.log, ["swipe:400,100,400,440", "io.refresh", "fail:io failed"]);
  assert.equal(failing.world.view, null);
  failing.renderer.unmount();
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
  assert.deepEqual({ ...host.views.deviceInformation }, { serial: "S1", deviceName: "Rig", macAddress: undefined, corOsVersion: undefined, linuxKernel: undefined, uBoot: undefined, zenjackApp: "3.1", zenjackBootloader: undefined, zencoderApp: undefined, zencoderBootloader: undefined, zenwireless: undefined }, "the application firmware build is the Zenjack row, not CorOS");
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

test("the Tuner toggles like Gig View: tapping it again closes it on the unit and in the app", async () => {
  const host = mount();
  await act(async () => { await host.views.toggle("tuner"); });
  assert.deepEqual(host.log, ["show:tuner:true", "tuner.open", "view:tuner"]);
  host.log.length = 0;
  await act(async () => { await host.views.toggle("tuner"); });
  assert.deepEqual(host.log, ["show:tuner:false", "view:null"]);
  host.renderer.unmount();
});

test("asking the unit for a screen tells the loop guard to expect it, so following the answer does not move the app again", async () => {
  const host = mount();
  await act(async () => { await host.views.open("gig"); await host.views.open("tuner"); });
  assert.equal(host.latch.takeUnit("gig"), true);
  assert.equal(host.latch.takeUnit("tuner"), true);
  await act(async () => { await host.views.open("io"); });
  assert.equal(host.latch.takeUnit("io"), true);
  host.renderer.unmount();
});

test("the app sends no tap while the unit shows an update screen", async () => {
  const host = mount();
  host.world.unit = "UpdaterDialog" as never;
  host.world.treeText = "DOWNLOAD UPDATE";
  host.rerender();
  await act(async () => { await host.views.forwardGesture(100, 100, 0, 0, "tap"); });
  assert.deepEqual(host.log, ["notice:The Quad Cortex is showing an update screen. Update screens are display only in the app; use the unit."]);
  host.renderer.unmount();
});
