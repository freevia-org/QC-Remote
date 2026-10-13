import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create } from "react-test-renderer";
register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const { useIoEqWorkflows } = await import("../src/use-io-eq-workflows.ts");
const { useTunerWorkflow } = await import("../src/use-tuner-workflow.ts");
const { useGuardedMutation } = await import("../src/use-guarded-mutation.ts");
const { useDeviceWorkflowSession } = await import("../src/use-device-workflow-session.ts");
const { useExpressionAssignment } = await import("../src/use-expression-assignment.ts");
const { demoSnapshot } = await import("../../qc-core/src/device-reference/demo-snapshot.ts");
function deferred() {
  let resolve!: (value: unknown) => void, reject!: (error: Error) => void;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const specifications = [
  { name: "io-eq", hook: useIoEqWorkflows, write: "setOutputPort", read: "ioSettings", reading: { outputs: [] }, cache: "io",
    invoke: (value, amount) => value.setOutputPort(1, amount, null, null) },
  { name: "tuner", hook: useTunerWorkflow, write: "setTunerReference", read: "tunerSettings", reading: { referenceHz: 440, referenceOffsetHz: 0 }, cache: "settings",
    invoke: (value, amount) => value.setReferenceOffset(amount) }
];
test("device-wide dial queues discard old writes and readbacks while a successor owns pending", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const spec of specifications) await t.test(spec.name, async () => {
    const old = deferred(), next = deferred();
    const calls: string[] = [], notices: string[] = [], failures: unknown[] = [];
    let writes = 0, pending = false, value, renderer;
    const gateway = { [spec.write]: () => { calls.push("write"); return ++writes === 1 ? old.promise : next.promise; },
      [spec.read]: async () => { calls.push("read"); return spec.reading; } };
    let options = { gateway, connected: true, sessionKey: "usb-A", pending: false, setPending: (busy) => { pending = busy; }, notice: (message) => notices.push(message), fail: (error) => failures.push(error) };
    function Harness() { value = spec.hook(options as never); return null; }
    await act(async () => { renderer = create(React.createElement(Harness)); });
    t.after(async () => { await act(async () => renderer.unmount()); });
    await act(async () => { spec.invoke(value, 1); spec.invoke(value, 2); });
    assert.equal(writes, 1);
    const retired = value;
    await act(async () => { options = { ...options, sessionKey: "usb-B" }; renderer.update(React.createElement(Harness)); });
    assert.equal(pending, false);
    assert.equal(value[spec.cache], undefined);
    await act(async () => { await retired.refresh(); spec.invoke(retired, 99); });
    assert.equal(value.loading, false);
    assert.equal(writes, 1);
    await act(async () => { spec.invoke(value, 3); });
    await act(async () => { old.resolve({ detail: "old write" }); });
    assert.equal(pending, true);
    assert.deepEqual(calls, ["write", "write"]);
    assert.deepEqual(notices, []);
    assert.deepEqual(failures, []);
    await act(async () => { next.resolve({ detail: "new write" }); });
    assert.equal(pending, false);
    assert.deepEqual(calls, ["write", "write", "read"]);
    assert.deepEqual(value[spec.cache], spec.reading);
    assert.equal(notices.at(-1), "new write");
  });
});

test("late IO-EQ and tuner reads/failures cannot populate replacement caches or availability", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const spec of specifications) for (const rejects of [false, true]) await t.test(`${spec.name}-${rejects}`, async () => {
    const old = deferred();
    const failures: unknown[] = [];
    let options = { gateway: { [spec.read]: () => old.promise, globalEq: () => old.promise }, connected: true, sessionKey: "A", pending: false, setPending: () => {}, notice: () => {}, fail: (error) => failures.push(error) };
    let value, renderer;
    function Harness() { value = spec.hook(options as never); return null; }
    await act(async () => { renderer = create(React.createElement(Harness)); });
    t.after(async () => { await act(async () => renderer.unmount()); });
    let reading;
    await act(async () => { reading = value.refresh(); });
    const retired = value;
    await act(async () => { options = { ...options, sessionKey: "B" }; renderer.update(React.createElement(Harness)); });
    await act(async () => { if (rejects) old.reject(new Error("old read")); else old.resolve(spec.reading); await reading; });
    assert.equal(value[spec.cache], undefined);
    assert.equal(value.loading, false);
    if (spec.name === "tuner") {
      assert.equal(value.available, true);
      await act(async () => retired.openTuner());
      assert.equal(value.open, false);
    }
    assert.deepEqual(failures, []);
  });
});

test("shared command ownership prevents same-tick writes across two different device workflows", async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const waiting = deferred();
  const calls: string[] = [];
  let io, tuner, renderer;
  const options = { gateway: { setOutputPort: () => { calls.push("io"); return waiting.promise; }, ioSettings: async () => ({ outputs: [] }),
    setTunerMute: async () => { calls.push("tuner"); return { detail: "muted" }; }, tunerSettings: async () => ({ referenceHz: 440, referenceOffsetHz: 0 }) },
    connected: true, pending: false, setPending: () => {}, notice: () => {}, fail: (error) => { throw error; } };
  function Harness() { io = useIoEqWorkflows(options); tuner = useTunerWorkflow(options); return null; }
  await act(async () => { renderer = create(React.createElement(Harness)); });
  await act(async () => { io.setOutputPort(1, 1, null, null); tuner.setMuted(true); });
  assert.deepEqual(calls, ["io"]);
  await act(async () => { waiting.resolve({ detail: "updated" }); });
  await act(async () => { tuner.setMuted(true); });
  assert.deepEqual(calls, ["io", "tuner"]);
  await act(async () => renderer.unmount());
});

test("guarded mutation refuses an invalid captured session before dispatch or pending", async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  let current = true, value, renderer;
  const calls: string[] = [];
  const options = { connected: true, pending: false, captureSessionCurrent: () => () => current,
    setPending: () => calls.push("pending"), notice: () => calls.push("notice"), fail: () => calls.push("fail") };
  function Harness() { value = useGuardedMutation(options, "writing"); return null; }
  await act(async () => { renderer = create(React.createElement(Harness)); });
  current = false;
  let result;
  await act(async () => { result = await value.run({ write: async () => { calls.push("write"); } }); });
  assert.equal(result, false);
  assert.deepEqual(calls, []);
  await act(async () => renderer.unmount());
});

test("StrictMode replay replaces revoked device owners and keeps fresh hook APIs usable", async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const initial = { ...demoSnapshot, presetIdentity: "preset-a", blocks: [{ id: "block", row: 0, column: 1, modelId: 42, name: "Delay" }] };
  const old = deferred();
  let reads = 0, session, expression, renderer;
  const observed: unknown[] = [];
  const options = { gateway: { identity: () => ++reads === 1 ? old.promise : Promise.resolve({ name: "new" }) }, snapshot: initial,
    connected: true, pending: false, setPending: () => {}, notice: () => {}, fail: () => {}, reconcile: () => {}, recordHistory: () => {} };
  function Harness() {
    session = useDeviceWorkflowSession(options);
    expression = useExpressionAssignment(options as never);
    React.useEffect(() => { void session.options.gateway.identity().then((value) => session.apply(() => observed.push(value))).catch(() => {}); }, [session.key]);
    return null;
  }
  await act(async () => { renderer = create(React.createElement(React.StrictMode, null, React.createElement(Harness))); });
  assert.equal(session.current(), true);
  assert.ok(observed.length > 0);
  const before = observed.length;
  await act(async () => { old.resolve({ name: "retired" }); });
  assert.equal(observed.length, before);
  await act(async () => expression.openAssignment({ row: 0, column: 1, parameters: [{ index: 0, expressionAssignable: true, writable: true }] }));
  assert.equal(expression.isOpen, true);
  await act(async () => renderer.unmount());
  assert.equal(session.current(), false);
});


test("replacement renders never expose previous device I/O or tuner observations before effects reset", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const spec of specifications) await t.test(spec.name, async () => {
    const rendered: { key: string; value: unknown }[] = [];
    let options = { gateway: { [spec.read]: async () => spec.reading, globalEq: async () => ({ bands: [] }) }, connected: true, sessionKey: "A", pending: false, setPending: () => {}, notice: () => {}, fail: () => {} };
    let value, renderer;
    function Harness() { value = spec.hook(options as never); rendered.push({ key: options.sessionKey, value: value[spec.cache] }); return null; }
    await act(async () => { renderer = create(React.createElement(Harness)); });
    await act(async () => { await value.refresh(); });
    assert.deepEqual(value[spec.cache], spec.reading);
    await act(async () => { options = { ...options, sessionKey: "B" }; renderer.update(React.createElement(Harness)); });
    assert.equal(rendered.find((entry) => entry.key === "B")?.value, undefined);
    await act(async () => renderer.unmount());
  });
});


test("unmounted range writer retains its physical command lease until dispatch settles", async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const wire = deferred();
  const pendingChanges: boolean[] = [], calls: string[] = [];
  let pending = false, io, peer, first, second;
  const setPending = (value: boolean) => { pending = value; pendingChanges.push(value); };
  const gateway = { setOutputPort: () => { calls.push("io"); return wire.promise; }, ioSettings: async () => ({ outputs: [] }) };
  const options = () => ({ gateway, connected: true, sessionKey: "physical-A", commandSessionKey: "physical-A", pending, setPending, notice: () => {}, fail: () => {} });
  function First() { io = useIoEqWorkflows(options()); return null; }
  function Second() { peer = useGuardedMutation(options(), "writing"); return null; }
  await act(async () => { first = create(React.createElement(First)); });
  await act(async () => { io.setOutputPort(1, 1, null, null); io.setOutputPort(1, 2, null, null); });
  await act(async () => first.unmount());
  assert.equal(pendingChanges.at(-1), true);
  await act(async () => { second = create(React.createElement(Second)); });
  let accepted;
  await act(async () => { accepted = await peer.run({ write: async () => { calls.push("peer"); } }); });
  assert.equal(accepted, false);
  assert.deepEqual(calls, ["io"]);
  await act(async () => { wire.resolve({ detail: "retired" }); });
  assert.equal(pendingChanges.at(-1), false);
  await act(async () => second.update(React.createElement(Second)));
  await act(async () => { accepted = await peer.run({ write: async () => { calls.push("peer"); } }); });
  assert.equal(accepted, true);
  assert.deepEqual(calls, ["io", "peer"]);
  await act(async () => second.unmount());
});

test("guarded dispatched leases survive unmount and retired APIs cannot write again", async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const wire = deferred();
  const calls: string[] = [];
  let pending = false, firstApi, peer, first, second;
  const setPending = (value: boolean) => { pending = value; };
  const options = () => ({ connected: true, sessionKey: "same", pending, setPending, notice: () => {}, fail: () => {} });
  function First() { firstApi = useGuardedMutation(options(), "writing"); return null; }
  function Second() { peer = useGuardedMutation(options(), "writing"); return null; }
  await act(async () => { first = create(React.createElement(First)); });
  const retired = firstApi;
  let running;
  await act(async () => { running = retired.run({ write: () => { calls.push("old"); return wire.promise; } }); });
  await act(async () => first.unmount());
  assert.equal(pending, true);
  await act(async () => { second = create(React.createElement(Second)); });
  await act(async () => { assert.equal(await peer.run({ write: async () => { calls.push("peer"); } }), false); });
  await act(async () => { wire.resolve(undefined); assert.equal(await running, false); });
  assert.equal(pending, false);
  await act(async () => { assert.equal(await retired.run({ write: async () => { calls.push("retired"); } }), false); });
  await act(async () => second.update(React.createElement(Second)));
  await act(async () => { assert.equal(await peer.run({ write: async () => { calls.push("peer"); } }), true); });
  assert.deepEqual(calls, ["old", "peer"]);
  await act(async () => second.unmount());
});

test("physical session reset clears retired pending but old finalizers cannot clear successor", async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const old = deferred(), next = deferred();
  let pending = false, key = "A", value, renderer;
  const changes: boolean[] = [];
  const setPending = (busy: boolean) => { pending = busy; changes.push(busy); };
  function Harness() { value = useGuardedMutation({ connected: true, sessionKey: key, pending, setPending, notice: () => {}, fail: () => {} }, "writing"); return null; }
  await act(async () => { renderer = create(React.createElement(Harness)); });
  let first, successor;
  await act(async () => { first = value.run({ write: () => old.promise }); });
  await act(async () => { key = "B"; renderer.update(React.createElement(Harness)); });
  assert.equal(pending, false);
  await act(async () => renderer.update(React.createElement(Harness)));
  await act(async () => { successor = value.run({ write: () => next.promise }); });
  assert.equal(pending, true);
  await act(async () => { old.resolve(undefined); await first; });
  assert.equal(pending, true);
  await act(async () => { next.resolve(undefined); assert.equal(await successor, true); });
  assert.equal(pending, false);
  assert.deepEqual(changes, [true, false, true, false]);
  await act(async () => renderer.unmount());
});

test("different workflow availability keys share one explicit physical command session", async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const old = deferred();
  let pending = false, a, b, renderer;
  const calls: string[] = [];
  const setPending = (busy: boolean) => { pending = busy; };
  const gateway = {};
  function Harness() {
    const common = { gateway, connected: true, commandSessionKey: "demo-session", pending, setPending, notice: () => {}, fail: () => {} };
    const first = useDeviceWorkflowSession({ ...common, sessionKey: "demo:hardware-false" });
    const second = useDeviceWorkflowSession({ ...common, sessionKey: "demo:ready-true" });
    a = useGuardedMutation(first.options, "writing");
    b = useGuardedMutation(second.options, "writing");
    return null;
  }
  await act(async () => { renderer = create(React.createElement(Harness)); });
  let running;
  await act(async () => { running = a.run({ write: () => { calls.push("a"); return old.promise; } }); });
  await act(async () => renderer.update(React.createElement(Harness)));
  assert.equal(pending, true);
  await act(async () => { assert.equal(await b.run({ write: async () => { calls.push("b"); } }), false); });
  await act(async () => { old.resolve(undefined); assert.equal(await running, true); });
  assert.equal(pending, false);
  await act(async () => renderer.update(React.createElement(Harness)));
  await act(async () => { assert.equal(await b.run({ write: async () => { calls.push("b"); } }), true); });
  assert.deepEqual(calls, ["a", "b"]);
  await act(async () => renderer.unmount());
});


test("new physical context clears pending from an already unmounted owner", async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const old = deferred(), next = deferred();
  let pending = false, key = "A", value, first, second;
  const setPending = (busy: boolean) => { pending = busy; };
  const gateway = {};
  function Harness() {
    const session = useDeviceWorkflowSession({ gateway, connected: true, sessionKey: key, commandSessionKey: key, pending, setPending, notice: () => {}, fail: () => {} });
    value = useGuardedMutation(session.options, "writing");
    return null;
  }
  await act(async () => { first = create(React.createElement(Harness)); });
  let firstRun, nextRun;
  await act(async () => { firstRun = value.run({ write: () => old.promise }); });
  await act(async () => first.unmount());
  assert.equal(pending, true);
  await act(async () => { key = "B"; second = create(React.createElement(Harness)); });
  assert.equal(pending, false);
  await act(async () => second.update(React.createElement(Harness)));
  await act(async () => { nextRun = value.run({ write: () => next.promise }); });
  await act(async () => { old.resolve(undefined); await firstRun; });
  assert.equal(pending, true);
  await act(async () => { next.resolve(undefined); assert.equal(await nextRun, true); });
  assert.equal(pending, false);
  await act(async () => second.unmount());
});

test("StrictMode replay keeps fresh guarded API usable without reviving earlier API", async () => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const calls: string[] = [];
  let firstApi, value, renderer;
  const options = { connected: true, sessionKey: "physical", pending: false, setPending: () => {}, notice: () => {}, fail: () => {} };
  function Harness() { value = useGuardedMutation(options, "writing"); firstApi ??= value; return null; }
  await act(async () => { renderer = create(React.createElement(React.StrictMode, null, React.createElement(Harness))); });
  await act(async () => { assert.equal(await firstApi.run({ write: async () => { calls.push("retired"); } }), false); });
  await act(async () => { assert.equal(await value.run({ write: async () => { calls.push("current"); } }), true); });
  assert.deepEqual(calls, ["current"]);
  await act(async () => renderer.unmount());
  await act(async () => { assert.equal(await value.run({ write: async () => { calls.push("unmounted"); } }), false); });
  assert.deepEqual(calls, ["current"]);
});
