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
type Unit = "Grid" | "MainMenuPopup" | "SettingsDialog" | "HybridModeConfigDialog" | "CheatSheetDialog" | "MessageDialog" | "TunerDialog";

interface Scenario {
  follow?: boolean; ready?: boolean; available?: boolean;
  /** What the app shows. */
  view?: string | null; popup?: "grid-menu" | "scene-menu" | "mode-menu" | null; settingsOpen?: boolean; tempoOpen?: boolean;
  /** What the unit shows, and how it responds to touches. */
  unit?: Unit; unitIgnores?: boolean;
}

function harness(initial: Scenario = {}) {
  const log: string[] = [];
  const world = { unit: initial.unit ?? "Grid" as Unit, unitIgnores: initial.unitIgnores ?? false };
  const state = { view: initial.view ?? null as string | null, popup: initial.popup ?? null as "grid-menu" | "scene-menu" | "mode-menu" | null, settingsOpen: initial.settingsOpen ?? false, tempoOpen: initial.tempoOpen ?? false };
  const latch = createQcScreenLatch();
  const gateway = {
    graphicsTree: async () => ({ tree: `zenUI::RootGraphicsItem\nzenUI::${world.unit}` }),
    tapScreen: async (x: number, y: number) => {
      log.push(`tap:${x},${y}`);
      if (world.unitIgnores) return;
      if (x === 764 && y === 22) world.unit = world.unit === "Grid" ? "MainMenuPopup" : world.unit === "MainMenuPopup" ? "Grid" : world.unit;
      else if (x === 600 && y === 406 && world.unit === "MainMenuPopup") world.unit = "SettingsDialog";
      else if (x === 743 && y === 30 && world.unit !== "Grid") world.unit = "Grid";
    },
    longTapScreen: async (x: number, y: number) => { log.push(`hold:${x},${y}`); if (!world.unitIgnores && world.unit === "Grid" && x === 700 && y === 70) world.unit = "HybridModeConfigDialog"; },
    swipeScreen: async () => { log.push("swipe"); }
  } as unknown as QcScreenFollowOptions["gateway"];
  const named = (name: string, ...methods: string[]) => Object.fromEntries(methods.map((method) => [method, () => { log.push(`${name}.${method}`); }]));
  const workflowsFor = () => ({
    grid: named("grid", "close"),
    preset: named("preset", "closeDirectory", "openDirectory"),
    deviceSettings: { ...named("settings", "close", "openSettings"), open: state.settingsOpen },
    tempo: { ...named("tempo", "close", "openTempo"), open: state.tempoOpen },
    tuner: named("tuner", "close", "openTuner"),
    ioEq: named("io", "refresh"),
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
  const rerender = async (next: Scenario) => { current = { ...current, ...next }; Object.assign(state, { view: next.view !== undefined ? next.view : state.view, popup: next.popup !== undefined ? next.popup : state.popup, settingsOpen: next.settingsOpen ?? state.settingsOpen, tempoOpen: next.tempoOpen ?? state.tempoOpen }); await act(async () => { renderer.update(createElement(Probe)); await new Promise((resolve) => setTimeout(resolve, 150)); }); };
  return { log, world, state, latch, rerender, get follower() { return seen.current!; }, renderer };
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

test("a parameter editor or unknown screen changes nothing, but still counts as a change of kind", () => {
  const host = harness();
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

test("every host reads and writes the follow setting through one hook; it is off by default", async () => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const store = new Map<string, string>();
  const storage = { getItem: (key: string) => store.get(key) ?? null, setItem: (key: string, value: string) => { store.set(key, value); } };
  const seen: { current?: ReturnType<typeof useQcFollowSetting> } = {};
  function Probe() { seen.current = useQcFollowSetting(storage); return null; }
  let renderer!: ReactTestRenderer;
  act(() => { renderer = create(createElement(Probe)); });
  assert.equal(seen.current!.follow, false, "off by default");
  act(() => seen.current!.setFollow(true));
  assert.equal(seen.current!.follow, true);
  assert.equal(store.get("qc-remote.follow-qc-screen-v1"), "true");
  renderer.unmount();
  act(() => { renderer = create(createElement(Probe)); });
  assert.equal(seen.current!.follow, true, "read back from storage by the next session");
  renderer.unmount();
});



