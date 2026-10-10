import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { COROS_CONTEXT_ACTION_LABELS, corOsUnavailableContextActionMessage, type CorOsContextAction } from "../src/coros-ui.ts";
import type { QcScreenDestination } from "../src/use-qc-screen-destination.ts";

// Load the hook only after the test loader shares React with react-test-renderer.
register("./support/react-singleton-loader.mjs", import.meta.url);
const { createElement } = (await import("react")).default;
const { QC_DISABLED_CONTEXT_ACTIONS, qcContextActionHandler, useQcScreenDestination } = await import("../src/use-qc-screen-destination.ts");

function mount() {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const seen: { current?: QcScreenDestination } = {};
  function Probe() { seen.current = useQcScreenDestination(); return null; }
  let renderer!: ReactTestRenderer;
  act(() => { renderer = create(createElement(Probe)); });
  return { get value() { return seen.current!; }, renderer };
}

test("opening a screen from the Grid menu remembers where focus returns, and closing forgets it", () => {
  const host = mount();
  assert.equal(host.value.view, null);
  act(() => host.value.open("cpu-monitor", "grid-menu"));
  assert.equal(host.value.view, "cpu-monitor");
  assert.equal(host.value.returnFocusTarget, "grid-menu");
  act(() => host.value.setView(null));
  assert.equal(host.value.view, null);
  assert.equal(host.value.returnFocusTarget, undefined);
  host.renderer.unmount();
});

test("a functional update that changes nothing keeps the return-focus target; an explicit set drops it", () => {
  const host = mount();
  act(() => host.value.open("modes", "grid-menu"));
  // An unrelated gig-state publication: "keep the current view".
  act(() => host.value.setView((current) => current));
  assert.equal(host.value.view, "modes");
  assert.equal(host.value.returnFocusTarget, "grid-menu", "a no-op must not lose the focus target");
  act(() => host.value.setView((current) => (current?.startsWith("gig") ? null : current)));
  assert.equal(host.value.returnFocusTarget, "grid-menu");
  // A real navigation (including to the same view) is an explicit choice and forgets it.
  act(() => host.value.setView("modes"));
  assert.equal(host.value.returnFocusTarget, undefined);
  act(() => host.value.setView("gig"));
  act(() => host.value.setView((current) => (current?.startsWith("gig") ? null : current)));
  assert.equal(host.value.view, null);
  host.renderer.unmount();
});

function router() {
  const calls: string[] = [];
  const destination = { open: (view: string, focus?: string) => calls.push(`open:${view}:${focus ?? "-"}`) };
  const handler = qcContextActionHandler({
    destination: destination as never,
    openSave: () => calls.push("save"),
    deviceSettings: { openSettings: (focus) => calls.push(`settings:${focus ?? "-"}`) },
    scene: { openCommand: (swap) => calls.push(`scene:${swap ? "swap" : "copy"}`) },
    modes: { refresh: () => calls.push("modes-refresh") },
    tempo: { openTempo: (focus) => calls.push(`tempo:${focus ?? "-"}`) },
    notice: (message) => calls.push(`notice:${message}`)
  });
  return { calls, handler };
}

test("each Grid context action reaches the one workflow or screen that owns it", () => {
  const expected: [CorOsContextAction, string[]][] = [
    ["edit-details", ["save"]],
    ["settings", ["settings:grid-menu"]],
    ["copy-scene", ["scene:copy"]],
    ["swap-scene", ["scene:swap"]],
    ["preset-midi-out", ["open:midi-out:grid-menu"]],
    ["modes-configuration", ["open:modes:grid-menu", "modes-refresh"]],
    ["tempo", ["tempo:grid-menu"]],
    ["cpu-monitor", ["open:cpu-monitor:grid-menu"]]
  ];
  for (const [action, calls] of expected) {
    const { handler, calls: seen } = router();
    handler(action, "grid-menu");
    assert.deepEqual(seen, calls, action);
  }
  const { handler, calls: seen } = router();
  handler("tempo");
  assert.deepEqual(seen, ["tempo:-"], "no return focus when not opened from the Grid menu");
});

test("exactly the actions the router refuses are the ones listed as disabled, with the shared wording", () => {
  const refused = (Object.keys(COROS_CONTEXT_ACTION_LABELS) as CorOsContextAction[]).filter((action) => {
    const { handler, calls } = router();
    handler(action);
    if (calls.length === 1 && calls[0] === `notice:${corOsUnavailableContextActionMessage(action)}`) return true;
    assert.ok(!calls.some((call) => call.startsWith("notice:")), `${action} must not both route and notify`);
    return false;
  });
  assert.deepEqual(refused.sort(), Object.keys(QC_DISABLED_CONTEXT_ACTIONS).sort());
});
