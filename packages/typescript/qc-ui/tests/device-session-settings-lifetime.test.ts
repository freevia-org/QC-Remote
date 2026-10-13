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
