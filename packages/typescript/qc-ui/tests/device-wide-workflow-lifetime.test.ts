import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create } from "react-test-renderer";

register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const { useTempoWorkflow } = await import("../src/use-tempo-workflow.ts");
const { useLooperXWorkflow } = await import("../src/use-looper-x-workflow.ts");
const { useModesConfigurationWorkflow } = await import("../src/use-modes-configuration-workflow.ts");

function deferred() {
  let resolve!: (value: unknown) => void;
  let reject!: (value: Error) => void;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const cases = [
  { name: "tempo", hook: useTempoWorkflow, write: "setTempoMode", reads: ["globalTempoSettings", "currentTempoClock"],
    invoke: (workflow) => workflow.setMode("GLOBAL"), reading: { mode: "GLOBAL", globalBpm: 123 }, cache: "settings" },
  { name: "looper", hook: useLooperXWorkflow, write: "controlLooper", reads: ["looperStatus"],
    invoke: (workflow) => workflow.run("play"), reading: { playing: true }, cache: "status" },
  { name: "modes", hook: useModesConfigurationWorkflow, write: "setModeCycle", reads: ["modeCycle"],
    invoke: (workflow) => workflow.commit([0, 1]), reading: { slots: [0, 1] }, cache: "slots" }
];

test("Device-wide writes stop their follow-up reads at session boundaries and leave a successor usable", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const specification of cases) await t.test(specification.name, async () => {
    const old = deferred(), next = deferred();
    const calls: string[] = [], notices: string[] = [], failures: unknown[] = [], pending: boolean[] = [];
    let writes = 0, value, renderer;
    const gateway = {
      [specification.write]: () => { calls.push("write"); return ++writes === 1 ? old.promise : next.promise; },
      ...Object.fromEntries(specification.reads.map((method) => [method, async () => { calls.push(method); return specification.reading; }]))
    };
    let options = { gateway, connected: true, sessionKey: "usb-A", pending: false,
      setPending: (busy: boolean) => pending.push(busy), notice: (message: string) => notices.push(message), fail: (error) => failures.push(error) };
    function Harness() { value = specification.hook(options as never); return null; }
    await act(async () => { renderer = create(React.createElement(Harness)); });
    t.after(async () => { await act(async () => renderer.unmount()); });
    let first, second;
    await act(async () => { first = specification.invoke(value); void specification.invoke(value); });
    assert.equal(writes, 1, "Two calls before React rerenders must admit only one write");
    await act(async () => { options = { ...options, sessionKey: "usb-B" }; renderer.update(React.createElement(Harness)); });
    assert.equal(value[specification.cache], undefined);
    notices.length = 0; pending.length = 0;
    await act(async () => { second = specification.invoke(value); });
    assert.equal(writes, 2);
    const pendingBeforeOldReply = [...pending];
    await act(async () => { old.resolve({ detail: "old completion" }); await first; });
    assert.deepEqual(calls, ["write", "write"], "A stale completion must not initiate a read on the replacement gateway");
    assert.deepEqual(notices, []);
    assert.deepEqual(failures, []);
    assert.deepEqual(pending, pendingBeforeOldReply, "The old operation must not release its successor's pending state");
    await act(async () => { next.resolve({ detail: "new completion" }); await second; });
    assert.deepEqual(calls, ["write", "write", ...specification.reads]);
    assert.equal(notices.at(-1), "new completion");
    assert.equal(pending.at(-1), false);
    assert.deepEqual(value[specification.cache], specification.name === "modes" ? [0, 1] : specification.reading);
  });
});

test("Device-wide reads discard stale values and failures before dispatching another source's read", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const specification of cases) await t.test(specification.name, async () => {
    for (const rejected of [false, true]) {
      const old = deferred();
      let read = () => old.promise;
      const calls: string[] = [], failures: unknown[] = [], notices: string[] = [];
      let value, renderer;
      const gateway = Object.fromEntries(specification.reads.map((method, index) => [method, () => {
        calls.push(method); return index === 0 ? read() : Promise.resolve(specification.reading);
      }]));
      let options = { gateway, connected: true, sessionKey: "usb-A", pending: false,
        setPending: () => undefined, notice: (message: string) => notices.push(message), fail: (error) => failures.push(error) };
      function Harness() { value = specification.hook(options as never); return null; }
      await act(async () => { renderer = create(React.createElement(Harness)); });
      let work;
      await act(async () => { work = value.refresh(); });
      await act(async () => { options = { ...options, sessionKey: "usb-B" }; renderer.update(React.createElement(Harness)); });
      await act(async () => { if (rejected) old.reject(new Error("old read failure")); else old.resolve(specification.reading); await work; });
      assert.deepEqual(calls, [specification.reads[0]]);
      assert.equal(value[specification.cache], undefined);
      assert.deepEqual(failures, []); assert.deepEqual(notices, []);
      read = () => Promise.resolve(specification.reading);
      await act(async () => { await value.refresh(); });
      assert.deepEqual(value[specification.cache], specification.name === "modes" ? [0, 1] : specification.reading);
      await act(async () => renderer.unmount());
    }
  });
});

test("Device-wide writes stop on host invalidation before React observes a transport change", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const specification of cases) await t.test(specification.name, async () => {
    const old = deferred();
    const calls: string[] = [], notices: string[] = [], failures: unknown[] = [];
    let generation = 1, value, renderer;
    const gateway = {
      [specification.write]: () => { calls.push("write"); return old.promise; },
      ...Object.fromEntries(specification.reads.map((method) => [method, async () => { calls.push(method); return specification.reading; }]))
    };
    const options = { gateway, connected: true, pending: false,
      captureSessionCurrent: () => { const captured = generation; return () => captured === generation; },
      setPending: () => undefined, notice: (message: string) => notices.push(message), fail: (error) => failures.push(error) };
    function Harness() { value = specification.hook(options as never); return null; }
    await act(async () => { renderer = create(React.createElement(Harness)); });
    let work;
    await act(async () => { work = specification.invoke(value); });
    generation++;
    await act(async () => { old.resolve({ detail: "retired USB completion" }); await work; });
    assert.deepEqual(calls, ["write"], "Host invalidation must stop the chained read even before a rerender");
    assert.deepEqual(notices, []); assert.deepEqual(failures, []);
    assert.equal(value[specification.cache], undefined);
    await act(async () => renderer.unmount());
  });
});

test("Retained device-wide APIs cannot reopen, load or mutate a successor session", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const specification of cases) await t.test(specification.name, async () => {
    const calls: string[] = [], notices: string[] = [], pending: boolean[] = [];
    let value, renderer;
    const gateway = {
      [specification.write]: async () => { calls.push("write"); return { detail: "sent" }; },
      ...Object.fromEntries(specification.reads.map((method) => [method, async () => { calls.push(method); return specification.reading; }]))
    };
    let options = { gateway, connected: true, sessionKey: "usb-A", pending: false,
      setPending: (busy: boolean) => pending.push(busy), notice: (message: string) => notices.push(message), fail: () => undefined };
    function Harness() { value = specification.hook(options as never); return null; }
    await act(async () => { renderer = create(React.createElement(Harness)); });
    const retired = value;
    await act(async () => { options = { ...options, sessionKey: "usb-B" }; renderer.update(React.createElement(Harness)); });
    await act(async () => { await value.refresh(); });
    const currentCache = value[specification.cache];
    calls.length = 0;
    await act(async () => {
      await retired.refresh(); await specification.invoke(retired);
      if (specification.name === "tempo") retired.openTempo();
    });
    assert.deepEqual(calls, []); assert.deepEqual(pending, []); assert.deepEqual(notices, []);
    assert.equal(value.loading, false);
    assert.deepEqual(value[specification.cache], currentCache);
    if (specification.name === "tempo") {
      assert.equal(value.open, false);
      await act(async () => { value.openTempo(); });
      assert.equal(value.open, true);
      await act(async () => { retired.close(); });
      assert.equal(value.open, true, "A retired close callback must not close the successor's screen");
    }
    await act(async () => renderer.unmount());
  });
});

test("The first replacement render hides the preceding device's cached observations", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const specification of cases) await t.test(specification.name, async () => {
    let value, renderer;
    const renders: Array<{ key: string; cache: unknown; loading: boolean; open: unknown }> = [];
    const gateway = Object.fromEntries(specification.reads.map((method) => [method, async () => specification.reading]));
    let options = { gateway, connected: true, sessionKey: "usb-A", pending: false,
      setPending: () => undefined, notice: () => undefined, fail: () => undefined };
    function Harness() {
      value = specification.hook(options as never);
      renders.push({ key: options.sessionKey, cache: value[specification.cache], loading: value.loading, open: value.open });
      return null;
    }
    await act(async () => { renderer = create(React.createElement(Harness)); });
    await act(async () => { await value.refresh(); if (specification.name === "tempo") value.openTempo(); });
    assert.notEqual(value[specification.cache], undefined);
    if (specification.name === "tempo") assert.equal(value.open, true);
    await act(async () => { options = { ...options, sessionKey: "usb-B" }; renderer.update(React.createElement(Harness)); });
    const first = renders.find((render) => render.key === "usb-B");
    assert.equal(first?.cache, undefined, "Old values must be hidden before the reset effect can run");
    assert.equal(first?.loading, false);
    if (specification.name === "tempo") assert.equal(first?.open, false);
    await act(async () => renderer.unmount());
  });
});
