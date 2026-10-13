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
const { useGuardedMutation } = await import("../src/use-guarded-mutation.ts");
const { PresetLifetime } = await import("@qc-remote/core");

function mount(overrides: { undoEntry?: { label: string }; redoEntry?: { label: string }; identity?: string | undefined; undoThrows?: boolean } = {}) {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const log: string[] = [];
  const identity = "identity" in overrides ? overrides.identity : "preset-1";
  const options: QcDeviceUndoOptions = {
    connected: true, pending: false, setPending: () => undefined, notice: () => undefined,
    sessionKey: "device-A", commandSessionKey: "device-A", reconcile: () => undefined,
    history: {
      undoEntry: overrides.undoEntry,
      redoEntry: overrides.redoEntry,
      undo: async () => { log.push("history.undo"); },
      redo: async () => { log.push("history.redo"); }
    },
    snapshot: { presetIdentity: identity, setlistKey: "A", presetPosition: 0 },
    gateway: {
      undo: async (id: string) => { log.push(`gateway.undo:${id}`); if (overrides.undoThrows) throw new Error("undo refused"); return { detail: "undone" }; },
      redo: async (id: string) => { log.push(`gateway.redo:${id}`); return { detail: "redone" }; },
      currentSnapshot: async () => ({ presetIdentity: identity, setlistKey: "A", presetPosition: 0 })
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

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function ownedHost() {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const old = deferred<Awaited<ReturnType<QcDeviceUndoOptions["gateway"]["undo"]>>>();
  const next = deferred<Awaited<ReturnType<QcDeviceUndoOptions["gateway"]["undo"]>>>();
  const calls: string[] = [], failures: unknown[] = [], notices: string[] = [], pending: boolean[] = [], snapshots: unknown[] = [];
  let writes = 0, value!: QcDeviceUndo, peer!: ReturnType<typeof useGuardedMutation>, renderer!: ReactTestRenderer;
  let options: QcDeviceUndoOptions = {
    sessionKey: "device-A", commandSessionKey: "device-A", connected: true, pending: false,
    snapshot: { presetIdentity: "id-A", setlistKey: "setlist", presetPosition: 0, stateVersion: { session: "A", revision: 1 } },
    history: { undo: async () => { calls.push("history.undo"); }, redo: async () => { calls.push("history.redo"); } },
    setPending: (busy) => pending.push(busy), notice: (message) => notices.push(message), fail: (error) => failures.push(error),
    reconcile: (snapshot) => snapshots.push(snapshot),
    gateway: {
      undo: (identity) => { calls.push(`undo:${identity}`); return ++writes === 1 ? old.promise : next.promise; },
      redo: (identity) => { calls.push(`redo:${identity}`); return ++writes === 1 ? old.promise : next.promise; },
      currentSnapshot: async () => { calls.push("snapshot"); return { ...options.snapshot } as never; }
    }
  };
  function Probe() { value = useQcDeviceUndo(options); peer = useGuardedMutation(options, "editing"); return null; }
  act(() => { renderer = create(createElement(Probe)); });
  return {
    old, next, calls, failures, notices, pending, snapshots,
    get undo() { return value; }, get peer() { return peer; },
    update(patch: Partial<QcDeviceUndoOptions>) { options = { ...options, ...patch }; renderer.update(createElement(Probe)); },
    snapshot(patch: Partial<QcDeviceUndoOptions["snapshot"]>) { return { ...options.snapshot, ...patch }; },
    close() { renderer.unmount(); }
  };
}

test("native undo claims the common wire slot before rerender and through authoritative readback", async (t) => {
  const host = ownedHost();
  t.after(() => act(() => host.close()));
  act(() => host.undo.consume([history(true, true)]));
  let first!: Promise<void>;
  await act(async () => {
    first = host.undo.undo();
    await host.undo.redo();
    await host.peer.run({ write: async () => { host.calls.push("peer.write"); } });
  });
  assert.deepEqual(host.calls, ["undo:id-A"]);
  assert.deepEqual(host.pending, [true]);
  assert.equal(host.notices.length, 2, "both repeated native command and peer explain their refusal");
  await act(async () => { host.old.resolve({ detail: "undone" }); await first; });
  assert.deepEqual(host.calls, ["undo:id-A", "snapshot"]);
  assert.equal(host.snapshots.length, 1);
  assert.deepEqual(host.pending, [true, false]);
  await act(async () => { await host.peer.run({ write: async () => { host.calls.push("peer.write"); } }); });
  assert.equal(host.calls.at(-1), "peer.write");
});

test("native undo refuses a busy or disconnected host and accepts a current embedded snapshot", async (t) => {
  const host = ownedHost();
  t.after(() => act(() => host.close()));
  act(() => host.undo.consume([history(true)]));
  act(() => host.update({ pending: true }));
  await act(async () => { await host.undo.undo(); });
  assert.deepEqual(host.calls, []);
  act(() => host.update({ pending: false, connected: false }));
  assert.equal(host.undo.canUndo, false);
  await act(async () => { await host.undo.undo(); });
  assert.deepEqual(host.calls, []);
  act(() => host.update({ connected: true }));
  act(() => host.undo.consume([history(true)]));
  let work!: Promise<void>;
  await act(async () => { work = host.undo.undo(); });
  const snapshot = host.snapshot({ presetIdentity: "accepted" });
  await act(async () => { host.old.resolve({ detail: "undone", snapshot: snapshot as never }); await work; });
  assert.deepEqual(host.calls, ["undo:id-A"]);
  assert.deepEqual(host.snapshots, [snapshot]);
});

test("native history availability and retained callbacks cannot cross preset or physical-session ABA", async (t) => {
  for (const boundary of ["preset", "session", "source"] as const) await t.test(boundary, async () => {
    const host = ownedHost();
    t.after(() => act(() => host.close()));
    act(() => host.undo.consume([history(true, true)]));
    const retired = host.undo;
    let first!: Promise<void>;
    await act(async () => { first = retired.undo(); });
    act(() => host.update(boundary === "preset"
      ? { snapshot: host.snapshot({ presetPosition: 1, presetIdentity: "id-B" }) }
      : { sessionKey: `${boundary}-B`, commandSessionKey: `${boundary}-B`, snapshot: host.snapshot({ stateVersion: { session: "B", revision: 1 }, presetIdentity: "id-B" }) }));
    assert.equal(host.undo.canUndo, false);
    act(() => host.update(boundary === "preset"
      ? { snapshot: host.snapshot({ presetPosition: 0, presetIdentity: "id-A" }) }
      : { sessionKey: "device-A", commandSessionKey: "device-A", snapshot: host.snapshot({ stateVersion: { session: "A", revision: 1 }, presetIdentity: "id-A" }) }));
    act(() => retired.consume([history(true, true)]));
    assert.equal(host.undo.canUndo, false, "the pre-ABA consumer cannot revive observations");
    await act(async () => { await retired.undo(); await retired.redo(); });
    assert.deepEqual(host.calls, ["undo:id-A"]);
    act(() => host.undo.consume([history(true)]));
    let second!: Promise<void>;
    if (boundary === "preset") {
      await act(async () => { await host.undo.undo(); });
      assert.deepEqual(host.calls, ["undo:id-A"], "preset navigation retains the already-dispatched wire lease");
      await act(async () => { host.old.resolve({ detail: "old result" }); await first; });
      assert.deepEqual(host.snapshots, []);
      await act(async () => { second = host.undo.undo(); });
    } else {
      await act(async () => { second = host.undo.undo(); });
      const held = [...host.pending];
      await act(async () => { host.old.reject(new Error("old session failed")); await first; });
      assert.deepEqual(host.pending, held, "the old finalizer cannot release its successor");
    }
    assert.deepEqual(host.calls, ["undo:id-A", "undo:id-A"]);
    assert.deepEqual(host.failures, []);
    await act(async () => { host.next.resolve({ detail: "current result" }); await second; });
    assert.deepEqual(host.calls, ["undo:id-A", "undo:id-A", "snapshot"]);
    assert.equal(host.snapshots.length, 1);
    assert.equal(host.pending.at(-1), false);
  });
});

test("foreign native history observations are ignored and current partial history is retained", (t) => {
  const host = ownedHost();
  t.after(() => act(() => host.close()));
  act(() => host.undo.consume([{ ...history(true, true), stateVersion: { session: "foreign", revision: 2 } }]));
  assert.equal(host.undo.canUndo, false);
  act(() => host.undo.consume([{ ...history(true, true), stateVersion: { session: "A", revision: 2 } }]));
  act(() => host.undo.consume([{ ...history(undefined, false), stateVersion: { session: "A", revision: 3 } }]));
  assert.equal(host.undo.undoAvailable, true);
  assert.equal(host.undo.redoAvailable, false);
});

test("same-address reload retires native history before rerender without treating every edit as navigation", async (t) => {
  const host = ownedHost();
  t.after(() => act(() => host.close()));
  const presetLifetime = new PresetLifetime({ setlistKey: "setlist", presetPosition: 0 });
  act(() => host.update({ presetLifetime }));
  act(() => host.undo.consume([history(true, true)]));
  const retired = host.undo;
  // Ordinary edits/readbacks revise identity but keep the current preset's history.
  act(() => host.update({ snapshot: host.snapshot({ presetIdentity: "edited-A" }) }));
  assert.equal(host.undo.canUndo, true);
  presetLifetime.invalidate();
  await act(async () => { await retired.undo(); });
  assert.deepEqual(host.calls, [], "explicit replacement blocks stale command before React renders");
  act(() => host.update({}));
  assert.equal(host.undo.canUndo, false);
  act(() => retired.consume([history(true, true)]));
  assert.equal(host.undo.canUndo, false);
  act(() => host.undo.consume([history(true)]));
  let work!: Promise<void>;
  await act(async () => { work = host.undo.undo(); });
  await act(async () => { host.old.resolve({ detail: "current reload history" }); await work; });
  assert.deepEqual(host.calls, ["undo:edited-A", "snapshot"]);
  assert.equal(host.snapshots.length, 1);
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
