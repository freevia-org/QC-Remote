import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create } from "react-test-renderer";
import { demoSnapshot } from "../../qc-core/src/device-reference/demo-snapshot.ts";
register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const { useExpressionAssignment } = await import("../src/use-expression-assignment.ts");
const { useLatestValueWriter } = await import("../src/latest-value-writer.ts");
const { useQcWorkflows } = await import("../src/use-qc-workflows.ts");
const { PresetLifetime } = await import("../../qc-core/src/preset-lifetime.ts");

function deferred() {
  let resolve!: (value: unknown) => void, reject!: (error: Error) => void;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const details = { row: 0, column: 1, name: "Delay", scene: 0, modelId: 42,
  bypassExpression: { pedal: 1, mode: 0, invert: false, delayMs: 0, latchEmulation: false },
  parameters: [{ index: 3, name: "Mix", expressionAssignable: true, writable: true, expression: 1, expressionMinimum: 0, expressionMaximum: 1 }] };
const initial = { ...demoSnapshot, presetIdentity: "preset-a", blocks: [{ id: "delay", row: 0, column: 1, modelId: 42, name: "Delay" }], stateVersion: { session: "usb-a", revision: 1 } };

test("expression replies and queued writes retire permanently on disconnect, preset or session replacement", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const boundary of ["disconnect", "preset", "session", "source-ABA"] as const) {
    await t.test(boundary, async () => {
      const old = deferred();
      const writes: unknown[] = [], saved: unknown[] = [], histories: unknown[] = [], failures: unknown[] = [], notices: string[] = [];
      const gateway = { setParameterExpression: (...args) => { writes.push(args); return old.promise; }, setExpressionBypass: (...args) => { writes.push(args); return old.promise; } };
      let pending = false, value, renderer;
      let options = { gateway, snapshot: initial, sessionKey: "usb", connected: true, pending: false,
        setPending: (next) => { pending = next; }, reconcile: (next) => saved.push(next), recordHistory: (entry) => histories.push(entry), notice: (message) => notices.push(message), fail: (error) => failures.push(error) };
      function Harness() { value = useExpressionAssignment(options); return null; }
      await act(async () => { renderer = create(React.createElement(Harness)); });
      t.after(async () => { await act(async () => renderer.unmount()); });
      await act(async () => { value.openAssignment(details); });
      await act(async () => {
        const panel = value.assignmentFor(details);
        panel.onSetRange(3, 0.1, 1);
        panel.onSetRange(3, 0.2, 1);
        panel.onAssign("bypass");
      });
      assert.equal(writes.length, 1);
      await act(async () => {
        options = boundary === "disconnect" ? { ...options, connected: false, snapshot: { ...initial, stateVersion: undefined } }
          : boundary === "preset" ? { ...options, snapshot: { ...initial, presetIdentity: "preset-b" } }
          : boundary === "session" ? { ...options, snapshot: { ...initial, stateVersion: { session: "usb-b", revision: 1 } } }
          : { ...options, sessionKey: "demo" };
        renderer.update(React.createElement(Harness));
      });
      assert.equal(pending, false);
      assert.equal(value.isOpen, false);
      if (boundary === "source-ABA") await act(async () => { options = { ...options, sessionKey: "usb" }; renderer.update(React.createElement(Harness)); });
      await act(async () => { old.resolve({ detail: "old reply", snapshot: initial }); });
      assert.equal(writes.length, 1, "Retired queued range and assign must never dispatch");
      assert.deepEqual(saved, []);
      assert.deepEqual(histories, []);
      assert.deepEqual(notices, []);
      assert.deepEqual(failures, []);
    });
  }
});

test("expression assignment suppresses stale command success/history and rejection before rerender", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const reject of [false, true]) {
    const old = deferred();
    let current = true, value, renderer;
    const saved: unknown[] = [], histories: unknown[] = [], failures: unknown[] = [], notices: string[] = [];
    const options = { gateway: { setExpressionBypass: () => old.promise }, snapshot: initial, connected: true, pending: false,
      captureSessionCurrent: () => () => current, setPending: () => {}, reconcile: (next) => saved.push(next), recordHistory: (entry) => histories.push(entry), notice: (message) => notices.push(message), fail: (error) => failures.push(error) };
    function Harness() { value = useExpressionAssignment(options); return null; }
    await act(async () => { renderer = create(React.createElement(Harness)); });
    await act(async () => { value.openAssignment(details); });
    await act(async () => { value.assignmentFor(details).onAssign("bypass"); });
    current = false;
    const count = notices.length;
    await act(async () => { if (reject) old.reject(new Error("old failure")); else old.resolve({ detail: "old reply", snapshot: initial }); });
    assert.deepEqual(saved, []);
    assert.deepEqual(histories, []);
    assert.deepEqual(failures, []);
    assert.equal(notices.length, count);
    await act(async () => renderer.unmount());
  }
});

test("latest-value reset drops queued commands and old completion cannot release successor pending", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const reject of [false, true]) {
    const old = deferred(), next = deferred();
    const accepted: string[] = [], errors: unknown[] = [];
    let pending = false, key = "a", value, renderer, queuedCommands = 0;
    function Harness() { value = useLatestValueWriter({ pending, setPending: (next) => { pending = next; }, resetKey: key }); return null; }
    await act(async () => { renderer = create(React.createElement(Harness)); });
    await act(async () => {
      value.submit("range", 1, { write: () => old.promise, accept: () => accepted.push("old"), error: (error) => errors.push(error) });
      value.submit("range", 2, { write: () => { throw new Error("queued old write"); }, error: (error) => errors.push(error) });
      value.runCommand(async () => { queuedCommands++; }, (error) => errors.push(error));
    });
    const retired = value;
    await act(async () => { key = "b"; renderer.update(React.createElement(Harness)); });
    assert.equal(retired.submit("range", 99, { write: async () => 99, error: () => {} }), false);
    assert.equal(retired.runCommand(async () => { queuedCommands++; }, () => {}), false);
    assert.equal(pending, false);
    assert.equal(value.drafts.size, 0);
    await act(async () => { value.submit("range", 3, { write: () => next.promise, accept: () => accepted.push("new"), error: (error) => errors.push(error) }); });
    assert.equal(pending, true);
    await act(async () => { if (reject) old.reject(new Error("retired")); else old.resolve(1); });
    assert.equal(pending, true);
    assert.equal(queuedCommands, 0);
    assert.deepEqual(accepted, []);
    assert.deepEqual(errors, []);
    await act(async () => { next.resolve(3); });
    assert.equal(pending, false);
    assert.deepEqual(accepted, ["new"]);
    await act(async () => renderer.unmount());
  }
});

test("shared reconciliation rejects a retired callback while current callbacks still adopt snapshots", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const lifetime = new PresetLifetime(initial);
  const accepted: unknown[] = [];
  const controller = { lifetime, snapshot: initial, snapshotRef: { current: initial }, acceptSnapshot: (next) => { accepted.push(next); return next; }, resetCommands: () => {}, setSnapshot: () => {} };
  const options = { controller, transport: {}, gateway: {}, editor: { close: () => {}, cancel: () => {} }, selectedBlockId: "", setSelectedBlockId: () => {}, connected: false, demo: false, pending: false, setPending: () => {}, prompts: {}, notice: () => {}, fail: () => {} };
  let value, renderer;
  function Harness() { value = useQcWorkflows(options); return null; }
  await act(async () => { renderer = create(React.createElement(Harness)); });
  const oldReconcile = value.reconcile;
  await act(async () => { lifetime.reset(); renderer.update(React.createElement(Harness)); });
  oldReconcile(initial);
  assert.deepEqual(accepted, []);
  value.reconcile(initial);
  assert.deepEqual(accepted, [initial]);
  await act(async () => renderer.unmount());
});


test("reset during a discrete command retires its error and finalizer without clearing the next command", async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const old = deferred(), next = deferred();
  const failures: unknown[] = [];
  let key = "a", pending = false, value, renderer;
  function Harness() { value = useLatestValueWriter({ pending, setPending: (next) => { pending = next; }, resetKey: key }); return null; }
  await act(async () => { renderer = create(React.createElement(Harness)); });
  await act(async () => { value.runCommand(async () => { await old.promise; }, (error) => failures.push(error)); });
  await act(async () => { key = "b"; renderer.update(React.createElement(Harness)); });
  await act(async () => { value.runCommand(async () => { await next.promise; }, (error) => failures.push(error)); });
  await act(async () => { old.reject(new Error("old command")); });
  assert.equal(pending, true);
  assert.deepEqual(failures, []);
  await act(async () => { next.resolve(undefined); });
  assert.equal(pending, false);
  await act(async () => renderer.unmount());
});

test("expression range queue follows its own accepted preset identity and keeps the final slider value", async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const old = deferred();
  const nextSnapshot = { ...initial, presetIdentity: "accepted-edit" };
  const writes: number[] = [], expected: string[] = [];
  let options, value, renderer;
  const gateway = { setParameterExpression: (_row, _column, _index, _pedal, minimum, _maximum, _modelId, _name, identity) => {
    writes.push(minimum);
    expected.push(identity);
    return writes.length === 1 ? old.promise : Promise.resolve({ detail: "final", snapshot: nextSnapshot });
  } };
  function Harness() { value = useExpressionAssignment(options); return null; }
  options = { gateway, snapshot: initial, connected: true, pending: false, setPending: () => {},
    reconcile: (snapshot) => { options = { ...options, snapshot }; renderer.update(React.createElement(Harness)); }, recordHistory: () => {}, notice: () => {}, fail: (error) => { throw error; } };
  await act(async () => { renderer = create(React.createElement(Harness)); });
  await act(async () => { value.openAssignment(details); });
  await act(async () => { const panel = value.assignmentFor(details); panel.onSetRange(3, 0.1, 1); panel.onSetRange(3, 0.2, 1); });
  await act(async () => { old.resolve({ detail: "accepted", snapshot: nextSnapshot }); });
  assert.deepEqual(writes, [0.1, 0.2]);
  assert.deepEqual(expected, ["preset-a", "accepted-edit"]);
  assert.equal(value.isOpen, true);
  await act(async () => renderer.unmount());
});


test("expression undo stays usable after same-session edits but never crosses a transport switch", async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const entries: unknown[] = [], writes: unknown[] = [];
  let value, renderer;
  let options = { gateway: { setExpressionBypass: (...args) => { writes.push(args); return Promise.resolve({ detail: "accepted", snapshot: initial }); } },
    snapshot: initial, sessionKey: "usb", connected: true, pending: false, setPending: () => {}, reconcile: () => {}, recordHistory: (entry) => entries.push(entry), notice: () => {}, fail: (error) => { throw error; } };
  function Harness() { value = useExpressionAssignment(options); return null; }
  await act(async () => { renderer = create(React.createElement(Harness)); });
  await act(async () => { value.openAssignment(details); });
  await act(async () => { value.assignmentFor(details).onAssign("bypass"); });
  assert.equal(entries.length, 1);
  const entry = entries[0] as { execute(snapshot: unknown): Promise<unknown> };
  const edited = { ...initial, presetIdentity: "later-edit" };
  await act(async () => { options = { ...options, snapshot: edited }; renderer.update(React.createElement(Harness)); });
  await entry.execute(edited);
  assert.equal(writes.length, 2);
  await act(async () => { options = { ...options, sessionKey: "demo" }; renderer.update(React.createElement(Harness)); });
  await assert.rejects(entry.execute(edited), /device session changed/);
  assert.equal(writes.length, 2);
  await act(async () => renderer.unmount());
});
