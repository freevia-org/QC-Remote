import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import type { QcStateUpdate } from "@qc-remote/core";
import type { QcScreenObservation } from "../src/use-adaptive-screen-polling.ts";
import type { QcScreenFollowOptions } from "../src/use-qc-screen-follow.ts";

// Load the hook only after the test loader shares React with react-test-renderer.
register("./support/react-singleton-loader.mjs", import.meta.url);
const { createElement } = (await import("react")).default;
const { consumeQcDisplayStates, qcGigViewTransition, useQcScreenFollow } = await import("../src/use-qc-screen-follow.ts");

const tree = (...classes: string[]) => classes.map((name) => `zenUI::${name}`).join("\n");
const observation = (kind: QcScreenObservation["kind"]): QcScreenObservation => ({ kind, signature: kind, observedAt: 0 });

function harness(overrides: Partial<QcScreenFollowOptions> = {}, trees: string[] = []) {
  const log: string[] = [];
  let polls = 0;
  const gateway = { graphicsTree: async () => ({ tree: trees[Math.min(polls++, trees.length - 1)] ?? tree("Grid") }) } as unknown as QcScreenFollowOptions["gateway"];
  const named = (name: string, ...methods: string[]) => Object.fromEntries(methods.map((method) => [method, () => { log.push(`${name}.${method}`); }]));
  const workflows = {
    grid: named("grid", "close"),
    preset: named("preset", "closeDirectory", "openDirectory"),
    deviceSettings: named("settings", "close", "openSettings"),
    tempo: named("tempo", "close", "openTempo"),
    tuner: named("tuner", "close", "openTuner"),
    ioEq: named("io", "refresh")
  } as unknown as QcScreenFollowOptions["workflows"];
  const base: QcScreenFollowOptions = { gateway, follow: true, available: false, ready: true, setView: ((next: unknown) => { log.push(`view:${typeof next === "function" ? "fn" : next}`); }) as QcScreenFollowOptions["setView"], workflows };
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const seen: { current?: ReturnType<typeof useQcScreenFollow> } = {};
  function Probe({ options }: { options: QcScreenFollowOptions }) { seen.current = useQcScreenFollow(options); return null; }
  let renderer!: ReactTestRenderer;
  act(() => { renderer = create(createElement(Probe, { options: { ...base, ...overrides } })); });
  const update = (next: Partial<QcScreenFollowOptions>) => act(() => renderer.update(createElement(Probe, { options: { ...base, ...overrides, ...next } })));
  return { log, update, get follower() { return seen.current!; }, renderer };
}
const overlaysClosed = ["grid.close", "preset.closeDirectory", "settings.close", "tempo.close", "tuner.close", "view:null"];

test("a screen the unit shows is followed once, not on every observation of the same kind", () => {
  const host = harness();
  act(() => host.follower.observe(observation("tuner")));
  assert.deepEqual(host.log, [...overlaysClosed, "view:tuner", "tuner.openTuner"]);
  host.log.length = 0;
  act(() => host.follower.observe(observation("tuner")));
  act(() => host.follower.observe(observation("tuner")));
  assert.deepEqual(host.log, [], "re-observing the same kind must not close and reopen the screen (no flapping)");
  host.renderer.unmount();
});

test("each kind opens the one app screen that mirrors it", () => {
  const expected: [QcScreenObservation["kind"], string[]][] = [
    ["tempo", ["tempo.openTempo"]],
    ["gig", ["view:gig"]],
    ["settings", ["settings.openSettings"]],
    ["io", ["view:io-overview", "io.refresh"]],
    ["directory", ["preset.openDirectory"]],
    ["cpu-monitor", ["view:cpu-monitor"]],
    ["looper", ["view:looper-editor"]],
    ["grid", []]
  ];
  for (const [kind, opens] of expected) {
    const host = harness();
    act(() => host.follower.observe(observation(kind)));
    assert.deepEqual(host.log, [...overlaysClosed, ...opens], kind);
    host.renderer.unmount();
  }
});

test("a parameter editor or unknown screen changes nothing, but still counts as a change of kind", () => {
  const host = harness();
  act(() => host.follower.observe(observation("tuner")));
  host.log.length = 0;
  act(() => host.follower.observe(observation("parameter-editor")));
  act(() => host.follower.observe(observation("unknown")));
  assert.deepEqual(host.log, [], "the app's own screens are left alone");
  act(() => host.follower.observe(observation("tuner")));
  assert.deepEqual(host.log, [...overlaysClosed, "view:tuner", "tuner.openTuner"], "the Tuner is followed again after the unit left it");
  host.renderer.unmount();
});

test("losing the session re-arms following, so the screen showing on resume is followed again", () => {
  const host = harness();
  act(() => host.follower.observe(observation("gig")));
  host.log.length = 0;
  host.update({ ready: false });
  host.update({ ready: true });
  act(() => host.follower.observe(observation("gig")));
  assert.deepEqual(host.log, [...overlaysClosed, "view:gig"]);
  // Switching following off re-arms it the same way.
  host.log.length = 0;
  host.update({ follow: false });
  host.update({ follow: true });
  act(() => host.follower.observe(observation("gig")));
  assert.deepEqual(host.log, [...overlaysClosed, "view:gig"]);
  host.renderer.unmount();
});

test("polling through the shared hook follows a screen once, ignores in-screen changes, and stops while the session is not ready", async () => {
  for (const ready of [true, false]) {
    const host = harness({ available: true, ready }, [tree("TunerDialog"), tree("TunerDialog", "Meter"), tree("Grid")]);
    await new Promise((resolve) => setTimeout(resolve, 900));
    if (!ready) assert.deepEqual(host.log, [], "nothing is followed while the session is not ready");
    else {
      assert.equal(host.log.filter((entry) => entry === "tuner.openTuner").length, 1, "a structural change inside the same screen kind does not reopen it");
      assert.ok(host.log.lastIndexOf("grid.close") > host.log.indexOf("tuner.openTuner"), "returning to the Grid closes the local screens");
    }
    host.renderer.unmount();
  }
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

test("every native state batch feeds the CPU readings, the undo availability and the Gig screen from one place", () => {
  const seen: string[] = [];
  const views: unknown[] = [];
  const states = [{ kind: "gigView", gigViewShown: true }, { kind: "cpuLoad", cpuTotalLoad: 20 }, { kind: "gigView" }, { kind: "history" }] as QcStateUpdate[];
  consumeQcDisplayStates(states, {
    setView: ((next: unknown) => { views.push(next); }) as never,
    cpu: { consume: (batch) => { seen.push(`cpu:${batch.length}`); } },
    undo: { consume: (batch) => { seen.push(`undo:${batch.length}`); } }
  });
  assert.deepEqual(seen, ["cpu:4", "undo:4"]);
  assert.equal(views.length, 1, "a gigView update without a shown flag is ignored");
  assert.equal((views[0] as (current: string | null) => string | null)(null), "gig");
});
