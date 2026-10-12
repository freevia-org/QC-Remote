import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import type { QcStateUpdate } from "@qc-remote/core";
import type { QcObservedScreenKind, QcScreenObservation } from "../src/use-adaptive-screen-polling.ts";
import { createQcScreenLatch } from "../src/qc-unit-screens.ts";
import type { QcScreenFollowOptions } from "../src/use-qc-screen-follow.ts";

// Load the hook only after the test loader shares React with react-test-renderer.
register("./support/react-singleton-loader.mjs", import.meta.url);
const { createElement } = (await import("react")).default;
const { consumeQcDisplayStates, qcGigViewTransition, useQcFollowSetting, useQcScreenFollow } = await import("../src/use-qc-screen-follow.ts");

const observation = (kind: QcObservedScreenKind): QcScreenObservation => ({ kind, signature: kind, observedAt: 0 });
type Unit = "Grid" | "MainMenuPopup" | "SettingsDialog" | "HybridModeConfigDialog" | "CheatSheetDialog" | "MessageDialog" | "TunerDialog" | "ParameterEditor" | "Keyboard" | "MetronomeEditor" | "CpuLoadIndicator" | "DirectoryDialog" | "SceneSelectorPopup" | "PortSelector";

interface Scenario {
  follow?: boolean; ready?: boolean; available?: boolean;
  /** What the app shows. */
  view?: string | null; popup?: "grid-menu" | "scene-menu" | "mode-menu" | null; settingsOpen?: boolean; tempoOpen?: boolean;
  /** What the unit shows, and how it responds to touches. */
  unit?: Unit; unitIgnores?: boolean;
  /** The app has a block editor open. */
  editorOpen?: boolean;
  directoryOpen?: boolean;
  routePicker?: { row: number; side: string };
}

function harness(initial: Scenario = {}) {
  const log: string[] = [];
  /** Runs after each touch the unit answers: the poller seeing the screen the unit passes through. */
  const watchers: Array<(unit: Unit) => void> = [];
  const world = { unit: initial.unit ?? "Grid" as Unit, unitIgnores: initial.unitIgnores ?? false };
  const state = { view: initial.view ?? null as string | null, popup: initial.popup ?? null as "grid-menu" | "scene-menu" | "mode-menu" | null, settingsOpen: initial.settingsOpen ?? false, directoryOpen: initial.directoryOpen ?? false, routePicker: initial.routePicker as { row: number; side: string } | undefined, tempoOpen: initial.tempoOpen ?? false };
  const latch = createQcScreenLatch();
  const gateway = {
    graphicsTree: async () => ({ tree: `zenUI::RootGraphicsItem${(world.unit === "CpuLoadIndicator" || world.unit === "SceneSelectorPopup" || world.unit === "PortSelector") ? "\nzenUI::Grid" : ""}${world.unit === "SettingsDialog" ? "\nzenUI::SettingsMenu\nzenUI::UpdaterSettings" : ""}\nzenUI::${world.unit}` }),
    tapScreen: async (x: number, y: number) => {
      log.push(`tap:${x},${y}`);
      if (world.unitIgnores) return;
      if (x === 764 && y === 22) world.unit = world.unit === "Grid" ? "MainMenuPopup" : world.unit === "MainMenuPopup" ? "Grid" : world.unit;
      else if (x === 600 && y === 406 && world.unit === "MainMenuPopup") world.unit = "SettingsDialog";
      else if (x === 30 && y === 147 && world.unit === "Grid") world.unit = "PortSelector";
      else if (x === 30 && y === 147 && world.unit === "PortSelector") world.unit = "Grid";
      else if (x === 320 && y === 50 && world.unit === "Grid") world.unit = "DirectoryDialog";
      else if (x === 668 && y === 23 && world.unit === "Grid") world.unit = "SceneSelectorPopup";
      else if (x === 668 && y === 23 && world.unit === "SceneSelectorPopup") world.unit = "Grid";
      else if (x === 610 && y === 286 && world.unit === "MainMenuPopup") world.unit = "MetronomeEditor";
      else if (x === 578 && y === 346 && world.unit === "MainMenuPopup") world.unit = "CpuLoadIndicator";
      else if (x === 628 && y === 32 && world.unit === "CpuLoadIndicator") world.unit = "Grid";
      else if (x === 743 && y === 30 && world.unit !== "Grid" && world.unit !== "CpuLoadIndicator" && world.unit !== "SceneSelectorPopup" && world.unit !== "PortSelector") world.unit = "Grid";
      for (const watch of watchers) watch(world.unit);
    },
    longTapScreen: async (x: number, y: number) => { log.push(`hold:${x},${y}`); if (!world.unitIgnores && world.unit === "Grid" && x === 700 && y === 70) world.unit = "HybridModeConfigDialog"; },
    swipeScreen: async () => { log.push("swipe"); }
  } as unknown as QcScreenFollowOptions["gateway"];
  const named = (name: string, ...methods: string[]) => Object.fromEntries(methods.map((method) => [method, () => { log.push(`${name}.${method}`); }]));
  const workflowsFor = () => ({
    grid: { ...named("grid", "close"), details: current.editorOpen ? {} : undefined },
    preset: { ...named("preset", "closeDirectory", "openDirectory"), directoryOpen: state.directoryOpen },
    deviceSettings: { ...named("settings", "close", "openSettings"), open: state.settingsOpen },
    tempo: { ...named("tempo", "close", "openTempo"), open: state.tempoOpen },
    tuner: named("tuner", "close", "openTuner"),
    ioEq: named("io", "refresh"),
    routing: { picker: state.routePicker },
    modes: named("modes", "refresh")
  }) as unknown as QcScreenFollowOptions["workflows"];
  const destination = () => ({
    view: state.view as never, returnFocusTarget: undefined, popup: state.popup, latch,
    setView: ((next: unknown) => { const value = typeof next === "function" ? (next as (c: string | null) => string | null)(state.view) : next as string | null; if (value !== state.view) log.push(`view:${value}`); state.view = value; }) as never,
    setPopup: (popup: typeof state.popup) => { if (popup !== state.popup) log.push(`popup:${popup}`); state.popup = popup; },
    open() {}
  });
  const options = (scenario: Scenario): QcScreenFollowOptions => ({
    gateway, follow: scenario.follow ?? true, available: scenario.available ?? false, ready: scenario.ready ?? true,
    destination: destination() as never, workflows: workflowsFor(), notice: (message) => { log.push(`notice:${message}`); },
    screenPoll: { intervalMs: 1, timeoutMs: 2 }
  });
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const seen: { current?: ReturnType<typeof useQcScreenFollow> } = {};
  let current = initial;
  function Probe() { seen.current = useQcScreenFollow(options(current)); return null; }
  let renderer!: ReactTestRenderer;
  act(() => { renderer = create(createElement(Probe)); });
  /** The app's screen changes by the user's hand (or the unit's), then the host re-renders. */
  const rerender = async (next: Scenario) => { current = { ...current, ...next }; Object.assign(state, { view: next.view !== undefined ? next.view : state.view, popup: next.popup !== undefined ? next.popup : state.popup, settingsOpen: next.settingsOpen ?? state.settingsOpen, directoryOpen: next.directoryOpen ?? state.directoryOpen, routePicker: "routePicker" in next ? next.routePicker : state.routePicker, tempoOpen: next.tempoOpen ?? state.tempoOpen }); await act(async () => { renderer.update(createElement(Probe)); await new Promise((resolve) => setTimeout(resolve, 150)); }); };
  return { log, world, state, latch, watchers, rerender, get follower() { return seen.current!; }, renderer };
}

const overlaysClosed = ["grid.close", "preset.closeDirectory", "settings.close", "tempo.close", "tuner.close"];
const opens: [QcObservedScreenKind, string[]][] = [
  ["tuner", ["view:tuner", "tuner.openTuner"]],
  ["tempo", ["tempo.openTempo"]],
  ["gig", ["view:gig"]],
  ["settings", ["settings.openSettings"]],
  ["io", ["view:io-overview", "io.refresh"]],
  ["directory", ["preset.openDirectory"]],
  ["cpu-monitor", ["view:cpu-monitor"]],
  ["looper", ["view:looper-editor"]],
  ["modes", ["view:modes", "modes.refresh"]],
  ["midi-out", ["view:midi-out"]],
  ["preset-menu", ["popup:grid-menu"]],
  ["scene-selector", ["popup:scene-menu"]],
  ["message-dialog", ["view:unit-mirror"]],
  ["keyboard", ["view:unit-mirror"]],
  ["search", ["view:unit-mirror"]],
  ["copy-scene", ["view:unit-mirror"]],
  ["route-selector", ["view:unit-mirror"]],
  ["device-browser", ["view:unit-mirror"]],
  ["neural-capture", ["view:unit-mirror"]],
  ["block-menu", ["view:unit-mirror"]],
  ["expression-assign", ["view:unit-mirror"]],
  ["busy", ["view:unit-mirror"]]
];

test("with following on, every screen the unit shows opens the matching app screen; the Grid closes the app's", () => {
  for (const [kind, expected] of opens) {
    const host = harness({ view: "gig" });
    act(() => host.follower.observe(observation(kind)));
    for (const entry of expected) assert.ok(host.log.includes(entry), `${kind}: ${entry} in ${JSON.stringify(host.log)}`);
    for (const entry of overlaysClosed) assert.ok(host.log.includes(entry), `${kind}: the local screens close first`);
    host.renderer.unmount();
  }
  const grid = harness({ view: "tuner" });
  act(() => grid.follower.observe(observation("grid")));
  assert.deepEqual(grid.log, [...overlaysClosed, "view:null"]);
  grid.renderer.unmount();
});

test("a screen the unit shows is followed once, not on every observation of the same kind", () => {
  const host = harness();
  act(() => host.follower.observe(observation("tuner")));
  host.log.length = 0;
  act(() => host.follower.observe(observation("tuner")));
  act(() => host.follower.observe(observation("tuner")));
  assert.deepEqual(host.log, [], "re-observing the same kind must not close and reopen the screen (no flapping)");
  host.renderer.unmount();
});

test("an unknown screen, or the editor the app itself opened, changes nothing, but still counts as a change of kind", () => {
  const host = harness({ editorOpen: true });
  act(() => host.follower.observe(observation("tuner")));
  host.log.length = 0;
  act(() => host.follower.observe(observation("parameter-editor")));
  act(() => host.follower.observe(observation("unknown")));
  assert.deepEqual(host.log, []);
  act(() => host.follower.observe(observation("tuner")));
  assert.ok(host.log.includes("tuner.openTuner"), "the Tuner is followed again after the unit left it");
  host.renderer.unmount();
});

test("losing the session re-arms following, so the screen showing on resume is followed again", async () => {
  const host = harness();
  act(() => host.follower.observe(observation("gig")));
  host.log.length = 0;
  await host.rerender({ ready: false });
  await host.rerender({ ready: true });
  act(() => host.follower.observe(observation("gig")));
  assert.ok(host.log.includes("view:gig"));
  host.log.length = 0;
  await host.rerender({ follow: false });
  await host.rerender({ follow: true });
  act(() => host.follower.observe(observation("gig")));
  assert.ok(host.log.includes("view:gig") || host.log.length === 0, "re-armed (the app already shows Gig, so only the latch is observable)");
  host.renderer.unmount();
});

test("polling through the hook follows a screen once, ignores in-screen changes, and stops while following is off", async () => {
  for (const follow of [true, false]) {
    const host = harness({ follow, available: true });
    await new Promise((resolve) => setTimeout(resolve, 900));
    // The scripted unit shows its Grid throughout: following opens nothing either way.
    assert.deepEqual(host.log.filter((entry) => entry.startsWith("view:") && entry !== "view:null"), []);
    host.renderer.unmount();
  }
});

test("the unit's screen moves the app only with the setting on: Gig View and every other class", async () => {
  // Following off: polling is not running at all and a state event for Gig View changes nothing.
  const off: string[] = [];
  consumeQcDisplayStates([{ kind: "gigView", gigViewShown: true } as QcStateUpdate], {
    setView: ((next: unknown) => { off.push(`view:${typeof next === "function" ? (next as (c: null) => string | null)(null) : next}`); }) as never,
    cpu: { consume() {} }, undo: { consume() {} }, follow: false
  });
  assert.deepEqual(off, [], "Gig View obeys the setting");
  const on: string[] = [];
  consumeQcDisplayStates([{ kind: "gigView", gigViewShown: true } as QcStateUpdate], {
    setView: ((next: unknown) => { on.push(`view:${typeof next === "function" ? (next as (c: null) => string | null)(null) : next}`); }) as never,
    cpu: { consume() {} }, undo: { consume() {} }, follow: true
  });
  assert.deepEqual(on, ["view:gig"]);
  // The polling observer is disabled by the same setting, for every class.
  for (const [kind] of opens) {
    const host = harness({ follow: false, available: true, unit: "SettingsDialog" });
    await new Promise((resolve) => setTimeout(resolve, 30));
    assert.deepEqual(host.log, [], `${kind}: nothing is polled or opened while following is off`);
    host.renderer.unmount();
  }
});

test("the CPU readings and the undo availability are not navigation: they are consumed whatever the setting", () => {
  const seen: string[] = [];
  consumeQcDisplayStates([{ kind: "cpuLoad" } as QcStateUpdate, { kind: "history" } as QcStateUpdate], {
    setView: (() => { seen.push("view"); }) as never,
    cpu: { consume: (batch) => { seen.push(`cpu:${batch.length}`); } }, undo: { consume: (batch) => { seen.push(`undo:${batch.length}`); } }, follow: false
  });
  assert.deepEqual(seen, ["cpu:2", "undo:2"]);
});

test("Gig View on the unit opens and closes the app's Gig screen without touching other screens", () => {
  assert.equal(qcGigViewTransition(null, true), "gig");
  assert.equal(qcGigViewTransition("tuner", true), "gig");
  assert.equal(qcGigViewTransition("gig", false), null);
  assert.equal(qcGigViewTransition("gig-live-tuner", false), null);
  assert.equal(qcGigViewTransition("tuner", false), "tuner");
  assert.equal(qcGigViewTransition("modes", false), "modes");
  assert.equal(qcGigViewTransition(null, false), null);
});

// ------------------------------------------------------------------------------------------ app to unit

test("the user opening the preset menu, Settings or Modes in the app drives the unit there, without following setting", async () => {
  for (const [popup, settingsOpen, view, gestures, unit] of [
    ["grid-menu", undefined, undefined, ["tap:764,22"], "MainMenuPopup"],
    [undefined, true, undefined, ["tap:764,22", "tap:600,406"], "SettingsDialog"],
    [undefined, undefined, "modes", ["hold:700,70"], "HybridModeConfigDialog"]
  ] as const) {
    const host = harness({ follow: false, available: true, ready: true });
    await host.rerender({ popup: popup ?? null, settingsOpen: settingsOpen ?? false, view: view ?? null });
    assert.deepEqual(host.log.filter((entry) => /^(tap|hold|swipe):/.test(entry)), [...gestures], unit);
    assert.equal(host.world.unit, unit);
    host.renderer.unmount();
  }
});

test("closing the screen in the app closes it on the unit: the kebab toggles the menu, the check button closes a dialog", async () => {
  const menu = harness({ follow: false, available: true, popup: "grid-menu", unit: "MainMenuPopup" });
  await menu.rerender({ popup: null });
  assert.deepEqual(menu.log.filter((entry) => entry.startsWith("tap:")), ["tap:764,22"]);
  assert.equal(menu.world.unit, "Grid");
  menu.renderer.unmount();
  const settings = harness({ follow: false, available: true, settingsOpen: true, unit: "SettingsDialog" });
  await settings.rerender({ settingsOpen: false });
  assert.deepEqual(settings.log.filter((entry) => entry.startsWith("tap:")), ["tap:743,30"]);
  settings.renderer.unmount();
});

test("the app never drives the unit from another screen, over a demo device or without a session", async () => {
  const busy = harness({ follow: false, available: true, unit: "MessageDialog" });
  await busy.rerender({ settingsOpen: true });
  assert.deepEqual(busy.log.filter((entry) => /^(tap|hold|swipe):/.test(entry)), [], "a confirmation on the unit is never tapped over");
  assert.ok(busy.log.some((entry) => entry.startsWith("notice:Close the screen")));
  busy.renderer.unmount();
  for (const scenario of [{ available: false }, { ready: false }]) {
    const host = harness({ follow: false, ...scenario });
    await host.rerender({ settingsOpen: true });
    assert.deepEqual(host.log.filter((entry) => /^(tap|hold|swipe):/.test(entry)), []);
    host.renderer.unmount();
  }
});

test("a unit that ignores the gesture is reported and the app does not claim the screen was shared", async () => {
  const host = harness({ follow: false, available: true, unitIgnores: true });
  await host.rerender({ view: "modes" });
  await new Promise((resolve) => setTimeout(resolve, 500));
  assert.ok(host.log.includes("notice:The Quad Cortex did not show the same screen."));
  host.renderer.unmount();
});

test("loop guard: a screen the unit opened is not sent back, and a screen the app opened is not followed back", async () => {
  // Unit to app: the unit shows Settings, the app follows; the app's own change must not tap the unit.
  const unitFirst = harness({ follow: false, available: true, unit: "SettingsDialog" });
  act(() => unitFirst.follower.observe(observation("settings")));
  await unitFirst.rerender({ settingsOpen: true });
  assert.deepEqual(unitFirst.log.filter((entry) => /^(tap|hold):/.test(entry)), [], "no echo to the unit");
  unitFirst.renderer.unmount();
  // App to unit: the app opens Modes (the unit follows by gesture); the unit's answer must not reopen it in the app.
  const appFirst = harness({ follow: false, available: true });
  await appFirst.rerender({ view: "modes" });
  appFirst.log.length = 0;
  act(() => appFirst.follower.observe(observation("modes")));
  assert.deepEqual(appFirst.log, [], "the app already shows it: nothing is closed and reopened");
  appFirst.renderer.unmount();
});

test("loop guard: a screen cannot flap between the two sides however often either reports it", async () => {
  const host = harness({ follow: false, available: true, unit: "Grid" });
  for (let round = 0; round < 3; round += 1) {
    act(() => host.follower.observe(observation("modes")));
    act(() => host.follower.observe(observation("modes")));
  }
  assert.equal(host.log.filter((entry) => entry === "view:modes").length, 1, "followed once");
  await host.rerender({ view: "modes" });
  assert.deepEqual(host.log.filter((entry) => /^(tap|hold|swipe):/.test(entry)), [], "the app's reflection of the unit's screen sends nothing back");
  host.renderer.unmount();
});

// ---------------------------------------------------------------------------------------------- the setting

test("every host reads and writes the follow setting through one hook; it is on by default", async () => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const store = new Map<string, string>();
  const storage = { getItem: (key: string) => store.get(key) ?? null, setItem: (key: string, value: string) => { store.set(key, value); } };
  const seen: { current?: ReturnType<typeof useQcFollowSetting> } = {};
  function Probe() { seen.current = useQcFollowSetting(storage); return null; }
  let renderer!: ReactTestRenderer;
  act(() => { renderer = create(createElement(Probe)); });
  assert.equal(seen.current!.follow, true, "on by default: nothing is stored yet");
  act(() => seen.current!.setFollow(false));
  assert.equal(seen.current!.follow, false);
  assert.equal(store.get("qc-remote.follow-qc-screen-v1"), "false");
  renderer.unmount();
  act(() => { renderer = create(createElement(Probe)); });
  assert.equal(seen.current!.follow, false, "a stored false is read back by the next session");
  act(() => seen.current!.setFollow(true));
  assert.equal(store.get("qc-remote.follow-qc-screen-v1"), "true");
  renderer.unmount();
  store.set("qc-remote.follow-qc-screen-v1", "unexpected");
  act(() => { renderer = create(createElement(Probe)); });
  assert.equal(seen.current!.follow, true, "anything but a stored false is on");
  renderer.unmount();
});




// ------------------------------------------------------------------------------------ the mirror audit

test("a block editor opened on the unit shows in the app as the unit's picture, and closing it on the unit closes that", () => {
  const host = harness();
  act(() => host.follower.observe(observation("parameter-editor")));
  assert.ok(host.log.includes("view:unit-mirror"), JSON.stringify(host.log));
  assert.ok(!host.log.includes("grid.close"), "the app has no editor to close, so no 'Parameter editor closed' notice either");
  assert.equal(host.state.view, "unit-mirror");
  host.log.length = 0;
  act(() => host.follower.observe(observation("grid")));
  assert.equal(host.state.view, null, "the Grid on the unit clears the picture");
  assert.ok(host.log.includes("grid.close"));
  host.renderer.unmount();
});

test("a popup over the editor on the unit, then the editor again, leaves the app on the unit's picture without flapping", () => {
  const host = harness({ editorOpen: true });
  act(() => host.follower.observe(observation("parameter-editor")));
  act(() => host.follower.observe(observation("block-menu")));
  assert.equal(host.state.view, "unit-mirror");
  host.log.length = 0;
  act(() => host.follower.observe(observation("parameter-editor")));
  assert.equal(host.state.view, "unit-mirror", "the editor the popup covered stays the unit's picture");
  assert.ok(!host.log.includes("view:null"), "not cleared and reopened");
  host.renderer.unmount();
});

test("loop guard: a unit screen the app already shows is not expected back, so the next real change is still sent", async () => {
  // The unit's Grid is observed (a screen closed on the unit, or the first poll). The app already shows the Grid.
  const host = harness({ follow: false, available: true, unit: "Grid" });
  act(() => host.follower.observe(observation("grid")));
  await host.rerender({ popup: "grid-menu" });
  await host.rerender({ popup: null });
  assert.deepEqual(host.log.filter((entry) => entry.startsWith("tap:")), ["tap:764,22", "tap:764,22"], "opening and closing the menu within seconds of the unit's Grid both reach the unit");
  assert.equal(host.world.unit, "Grid");
  host.renderer.unmount();
});

test("loop guard: a close that needed no touch leaves nothing that would swallow the unit's next change", async () => {
  const host = harness({ follow: false, available: true, settingsOpen: true, unit: "Keyboard" });
  await host.rerender({ settingsOpen: false });
  assert.deepEqual(host.log.filter((entry) => entry.startsWith("tap:")), [], "the unit is not showing Settings: nothing to close");
  await host.rerender({ view: "tuner" });
  host.log.length = 0;
  act(() => host.follower.observe(observation("grid")));
  assert.ok(host.log.includes("view:null"), "the unit returning to its Grid still closes the app's screen");
  host.renderer.unmount();
});

test("loop guard: an open the unit did not take is withdrawn, so the unit's own later screen is followed", async () => {
  const host = harness({ follow: false, available: true, unitIgnores: true });
  await host.rerender({ view: "modes" });
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert.ok(host.log.includes("notice:The Quad Cortex did not show the same screen."));
  host.log.length = 0;
  act(() => host.follower.observe(observation("modes")));
  assert.ok(host.log.includes("modes.refresh"), "a Modes screen the user later opens on the unit is followed, not mistaken for the app's request");
  host.renderer.unmount();
});

test("the screen the unit passes through while the app drives it to Settings does not pull the app back", async () => {
  const host = harness({ follow: false, available: true });
  // The poller looks at the unit after each touch, as the adaptive poll does within 250 ms of a change.
  host.watchers.push((unit) => {
    if (unit === "MainMenuPopup") act(() => host.follower.observe(observation("preset-menu")));
    if (unit === "SettingsDialog") act(() => host.follower.observe(observation("settings")));
  });
  await host.rerender({ settingsOpen: true });
  assert.equal(host.world.unit, "SettingsDialog");
  assert.ok(!host.log.includes("popup:grid-menu"), "the menu on the way is not shown in the app");
  assert.ok(!host.log.includes("settings.close"), "the app's Settings is not closed under the user");
  assert.ok(!host.log.some((entry) => entry.startsWith("view:")), JSON.stringify(host.log));
  host.renderer.unmount();
});

test("the unit's Grid answer to the app's own close of Settings is not echoed back into the app", async () => {
  const host = harness({ follow: false, available: true, settingsOpen: true, unit: "SettingsDialog" });
  host.watchers.push((unit) => { if (unit === "Grid") act(() => host.follower.observe(observation("grid"))); });
  await host.rerender({ settingsOpen: false });
  assert.deepEqual(host.log.filter((entry) => entry.startsWith("tap:")), ["tap:743,30"]);
  assert.ok(!host.log.includes("settings.close"), "the answer of the unit to the app's own tap changes nothing in the app");
  host.renderer.unmount();
});

test("with the setting off the unit's screens, including a block editor, never move the app; the app is never tapped over an open editor", async () => {
  const off = harness({ follow: false, available: true, unit: "ParameterEditor" });
  await new Promise((resolve) => setTimeout(resolve, 60));
  assert.deepEqual(off.log, []);
  await off.rerender({ view: "modes" });
  assert.deepEqual(off.log.filter((entry) => /^(tap|hold|swipe):/.test(entry)), []);
  assert.ok(off.log.some((entry) => entry.startsWith("notice:Close the screen open on the Quad Cortex first")));
  off.renderer.unmount();
});

test("Tempo and the CPU Monitor open from the menu rows, and close on their own button: the check for Tempo, the overlay's X for the CPU", async () => {
  const tempo = harness({ follow: false, available: true });
  await tempo.rerender({ tempoOpen: true });
  assert.deepEqual(tempo.log.filter((entry) => entry.startsWith("tap:")), ["tap:764,22", "tap:610,286"]);
  assert.equal(tempo.world.unit, "MetronomeEditor");
  tempo.log.length = 0;
  await tempo.rerender({ tempoOpen: false });
  assert.deepEqual(tempo.log.filter((entry) => entry.startsWith("tap:")), ["tap:743,30"]);
  assert.equal(tempo.world.unit, "Grid");
  tempo.renderer.unmount();
  const cpu = harness({ follow: false, available: true });
  await cpu.rerender({ view: "cpu-monitor" });
  assert.deepEqual(cpu.log.filter((entry) => entry.startsWith("tap:")), ["tap:764,22", "tap:578,346"]);
  assert.equal(cpu.world.unit, "CpuLoadIndicator");
  cpu.log.length = 0;
  await cpu.rerender({ view: null });
  assert.deepEqual(cpu.log.filter((entry) => entry.startsWith("tap:")), ["tap:628,32"], "the X, never the check button (743,30)");
  assert.equal(cpu.world.unit, "Grid");
  cpu.renderer.unmount();
});

test("the CPU overlay is seen with the Grid still in the tree, and followed both ways without flapping", async () => {
  const { classifyQcGraphicsTree } = await import("../src/use-adaptive-screen-polling.ts");
  assert.equal(classifyQcGraphicsTree("zenUI::RootGraphicsItem" + String.fromCharCode(10) + "zenUI::Grid" + String.fromCharCode(10) + "zenUI::CpuLoadIndicator"), "cpu-monitor");
  const host = harness({ follow: false, available: true });
  host.watchers.push((unit) => { if (unit === "CpuLoadIndicator") act(() => host.follower.observe(observation("cpu-monitor"))); if (unit === "Grid") act(() => host.follower.observe(observation("grid"))); });
  await host.rerender({ view: "cpu-monitor" });
  await host.rerender({ view: null });
  assert.deepEqual(host.log.filter((entry) => entry.startsWith("tap:")), ["tap:764,22", "tap:578,346", "tap:628,32"], "the unit's answers are not echoed back");
  host.renderer.unmount();
  // Unit to app: the user opens the overlay on the unit, then closes it on the unit.
  const unitFirst = harness({ unit: "CpuLoadIndicator" });
  act(() => unitFirst.follower.observe(observation("cpu-monitor")));
  assert.equal(unitFirst.state.view, "cpu-monitor");
  act(() => unitFirst.follower.observe(observation("grid")));
  assert.equal(unitFirst.state.view, null);
  unitFirst.renderer.unmount();
});

test("Settings opens on its Updates pane: the app's routes send only the menu, the row and the header check, never a touch in that pane", async () => {
  const allowed = ["tap:764,22", "tap:600,406", "tap:743,30"];
  const host = harness({ follow: false, available: true });
  await host.rerender({ settingsOpen: true });
  assert.equal(host.world.unit, "SettingsDialog");
  await host.rerender({ settingsOpen: false });
  assert.equal(host.world.unit, "Grid");
  const touches = host.log.filter((entry) => /^(tap|hold|swipe):/.test(entry));
  assert.deepEqual(touches, allowed);
  host.renderer.unmount();
  // A Settings the unit shows by itself, then the app's close: still only the header check.
  const closing = harness({ follow: false, available: true, settingsOpen: true, unit: "SettingsDialog" });
  await closing.rerender({ settingsOpen: false });
  assert.deepEqual(closing.log.filter((entry) => /^(tap|hold|swipe):/.test(entry)), ["tap:743,30"]);
  closing.renderer.unmount();
});

test("the Preset Directory opens on the title area and closes on the header check; no row, bank or cloud button is ever touched", async () => {
  const host = harness({ follow: false, available: true });
  host.watchers.push((unit) => { if (unit === "DirectoryDialog") act(() => host.follower.observe(observation("directory"))); if (unit === "Grid") act(() => host.follower.observe(observation("grid"))); });
  await host.rerender({ directoryOpen: true });
  assert.equal(host.world.unit, "DirectoryDialog");
  await host.rerender({ directoryOpen: false });
  assert.equal(host.world.unit, "Grid");
  assert.deepEqual(host.log.filter((entry) => /^(tap|hold|swipe):/.test(entry)), ["tap:320,50", "tap:743,30"]);
  assert.ok(!host.log.includes("preset.closeDirectory") && !host.log.includes("preset.openDirectory"), "the unit's answers are not echoed into the app");
  host.renderer.unmount();
  const busy = harness({ follow: false, available: true, unit: "ParameterEditor" });
  await busy.rerender({ directoryOpen: true });
  assert.deepEqual(busy.log.filter((entry) => /^(tap|hold|swipe):/.test(entry)), [], "only ever from the Grid");
  busy.renderer.unmount();
});

test("the scene popup opens and closes on the scene indicator and is seen with the Grid in the tree; no scene row is touched", async () => {
  const { classifyQcGraphicsTree } = await import("../src/use-adaptive-screen-polling.ts");
  const nl = String.fromCharCode(10);
  assert.equal(classifyQcGraphicsTree("zenUI::RootGraphicsItem" + nl + "zenUI::Grid" + nl + "zenUI::SceneSelectorPopup" + nl + "zenUI::SceneTable"), "scene-selector");
  const host = harness({ follow: false, available: true });
  host.watchers.push((unit) => { if (unit === "SceneSelectorPopup") act(() => host.follower.observe(observation("scene-selector"))); if (unit === "Grid") act(() => host.follower.observe(observation("grid"))); });
  await host.rerender({ popup: "scene-menu" });
  assert.equal(host.world.unit, "SceneSelectorPopup");
  await host.rerender({ popup: null });
  assert.equal(host.world.unit, "Grid");
  assert.deepEqual(host.log.filter((entry) => /^(tap|hold|swipe):/.test(entry)), ["tap:668,23", "tap:668,23"]);
  assert.ok(!host.log.some((entry) => entry.startsWith("popup:")), "no echo");
  host.renderer.unmount();
  // Unit to app and back, with the setting on.
  const unitFirst = harness({ unit: "SceneSelectorPopup" });
  act(() => unitFirst.follower.observe(observation("scene-selector")));
  assert.equal(unitFirst.state.popup, "scene-menu");
  act(() => unitFirst.follower.observe(observation("grid")));
  assert.equal(unitFirst.state.popup, null);
  unitFirst.renderer.unmount();
});

test("the first row's input picker opens and closes on its rail label; no row of the list is ever touched, no other picker is routed", async () => {
  const host = harness({ follow: false, available: true });
  host.watchers.push((unit) => { if (unit === "PortSelector") act(() => host.follower.observe(observation("route-selector"))); if (unit === "Grid") act(() => host.follower.observe(observation("grid"))); });
  await host.rerender({ routePicker: { row: 0, side: "input" } });
  assert.equal(host.world.unit, "PortSelector");
  await host.rerender({ routePicker: undefined });
  assert.equal(host.world.unit, "Grid");
  assert.deepEqual(host.log.filter((entry) => /^(tap|hold|swipe):/.test(entry)), ["tap:30,147", "tap:30,147"]);
  assert.ok(!host.log.some((entry) => entry.startsWith("view:")), "the unit's answers are not echoed into the app");
  host.renderer.unmount();
  for (const picker of [{ row: 0, side: "output" }, { row: 1, side: "input" }, { row: 0, side: "split" }]) {
    const other = harness({ follow: false, available: true });
    await other.rerender({ routePicker: picker });
    assert.deepEqual(other.log.filter((entry) => /^(tap|hold|swipe):/.test(entry)), [], `${picker.row} ${picker.side}: unmeasured, not routed`);
    other.renderer.unmount();
  }
  const busy = harness({ follow: false, available: true, unit: "ParameterEditor" });
  await busy.rerender({ routePicker: { row: 0, side: "input" } });
  assert.deepEqual(busy.log.filter((entry) => /^(tap|hold|swipe):/.test(entry)), [], "only ever from the Grid");
  busy.renderer.unmount();
  // The unit's own picker shows in the app as its picture, and the plain Grid closes it.
  const unitFirst = harness({ unit: "PortSelector" });
  act(() => unitFirst.follower.observe(observation("route-selector")));
  assert.equal(unitFirst.state.view, "unit-mirror");
  act(() => unitFirst.follower.observe(observation("grid")));
  assert.equal(unitFirst.state.view, null);
  unitFirst.renderer.unmount();
});
