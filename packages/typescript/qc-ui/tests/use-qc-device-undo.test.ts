import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import type { QcStateUpdate } from "@qc-remote/core";
import type { QcDeviceUndo, QcDeviceUndoOptions } from "../src/use-qc-device-undo.ts";

// Load the hook only after the test loader shares React with react-test-renderer.
register("./support/react-singleton-loader.mjs", import.meta.url);
const { createElement } = (await import("react")).default;
const { useQcDeviceUndo } = await import("../src/use-qc-device-undo.ts");

function mount(overrides: { undoEntry?: { label: string }; redoEntry?: { label: string }; identity?: string | undefined; undoThrows?: boolean } = {}) {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const log: string[] = [];
  const identity = "identity" in overrides ? overrides.identity : "preset-1";
  const options: QcDeviceUndoOptions = {
    history: {
      undoEntry: overrides.undoEntry,
      redoEntry: overrides.redoEntry,
      undo: async () => { log.push("history.undo"); },
      redo: async () => { log.push("history.redo"); }
    },
    snapshot: { presetIdentity: identity } as QcDeviceUndoOptions["snapshot"],
    gateway: {
      undo: async (id: string) => { log.push(`gateway.undo:${id}`); if (overrides.undoThrows) throw new Error("undo refused"); },
      redo: async (id: string) => { log.push(`gateway.redo:${id}`); }
    } as unknown as QcDeviceUndoOptions["gateway"],
    fail: (error) => { log.push(`fail:${(error as Error).message}`); }
  };
  const seen: { current?: QcDeviceUndo } = {};
  function Probe() { seen.current = useQcDeviceUndo(options); return null; }
  let renderer!: ReactTestRenderer;
  act(() => { renderer = create(createElement(Probe)); });
  return { log, get undo() { return seen.current!; }, renderer };
}
const history = (undoAvailable?: boolean, redoAvailable?: boolean) => ({ kind: "history", undoAvailable, redoAvailable }) as QcStateUpdate;

test("the unit's reported availability enables undo and redo, and a partial update keeps the other value", () => {
  const host = mount();
  assert.equal(host.undo.canUndo, false);
  act(() => host.undo.consume([history(true, true)]));
  assert.equal(host.undo.canUndo, true);
  assert.equal(host.undo.redoAvailable, true);
  act(() => host.undo.consume([history(undefined, false), { kind: "cpuLoad" } as QcStateUpdate]));
  assert.equal(host.undo.undoAvailable, true);
  assert.equal(host.undo.redoAvailable, false);
  host.renderer.unmount();
});

test("the app's own history step wins and supplies the label; otherwise the unit's history is used", async () => {
  const own = mount({ undoEntry: { label: "Bypass Amp" }, redoEntry: { label: "Bypass Amp" } });
  assert.equal(own.undo.canUndo, true);
  assert.equal(own.undo.undoLabel, "Bypass Amp");
  await act(async () => { await own.undo.undo(); await own.undo.redo(); });
  assert.deepEqual(own.log, ["history.undo", "history.redo"]);
  own.renderer.unmount();
  const unit = mount();
  act(() => unit.undo.consume([history(true, true)]));
  await act(async () => { await unit.undo.undo(); await unit.undo.redo(); });
  assert.deepEqual(unit.log, ["gateway.undo:preset-1", "gateway.redo:preset-1"]);
  unit.renderer.unmount();
});

test("nothing is sent without availability or without a preset identity", async () => {
  const host = mount();
  await act(async () => { await host.undo.undo(); });
  assert.deepEqual(host.log, []);
  host.renderer.unmount();
  const anonymous = mount({ identity: undefined });
  act(() => anonymous.undo.consume([history(true, true)]));
  await act(async () => { await anonymous.undo.undo(); });
  assert.deepEqual(anonymous.log, []);
  anonymous.renderer.unmount();
});

test("a refused unit undo is reported instead of becoming an unhandled rejection", async () => {
  const host = mount({ undoThrows: true });
  act(() => host.undo.consume([history(true)]));
  await act(async () => { await host.undo.undo(); });
  assert.deepEqual(host.log, ["gateway.undo:preset-1", "fail:undo refused"]);
  host.renderer.unmount();
});
