import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create } from "react-test-renderer";
import { demoSnapshot } from "../../qc-core/src/device-reference/demo-snapshot.ts";

register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const { usePresetMidiOutWorkflow } = await import("../src/use-preset-midi-out-workflow.ts");
const { captureGatewaySession, registerGatewayRoute } = await import("../src/gateway-session.ts");
const { PresetLifetime } = await import("../../qc-core/src/preset-lifetime.ts");

function deferred() {
  let resolve!: (value: unknown) => void, reject!: (error: Error) => void;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test("MIDI Out save retires at source, preset and session boundaries without stale snapshots or failures", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const boundary of ["source", "preset", "session", "source-ABA"] as const) {
    await t.test(boundary, async () => {
      const old = deferred(), next = deferred();
      let writes = 0, value;
      const saved: unknown[] = [];
      const gateway = { setMidiOut: () => (++writes === 1 ? old.promise : next.promise) };
      const initial = { ...demoSnapshot, presetIdentity: "preset-a", stateVersion: { session: "usb-a", revision: 1 } };
      let options = { gateway, snapshot: initial, demo: false, onSaved: (snapshot) => saved.push(snapshot) };
      function Harness() { value = usePresetMidiOutWorkflow(options); return null; }
      let renderer;
      await act(async () => { renderer = create(React.createElement(Harness)); });
      t.after(async () => { await act(async () => renderer.unmount()); });
      let first;
      await act(async () => { first = value.save(0, []); void value.save(1, []); });
      assert.equal(writes, 1, "A same-tick duplicate must not enter the native gateway");
      await act(async () => {
        options = boundary.startsWith("source") ? { ...options, demo: true }
          : boundary === "preset" ? { ...options, snapshot: { ...initial, presetIdentity: "preset-b" } }
            : { ...options, snapshot: { ...initial, stateVersion: { session: "usb-b", revision: 1 } } };
        renderer.update(React.createElement(Harness));
      });
      assert.equal(value.pending, false);
      assert.equal(value.error, "");
      assert.equal(value.status, null);
      if (boundary === "source-ABA") {
        await act(async () => { options = { ...options, demo: false }; renderer.update(React.createElement(Harness)); });
      }
      if (boundary.startsWith("source")) {
        await act(async () => { old.resolve({ detail: "old saved", snapshot: initial }); await first; });
      } else {
        let second;
        await act(async () => { second = value.save(0, []); });
        assert.equal(value.pending, true);
        await act(async () => { old.reject(new Error("old failure")); await first; });
        assert.equal(value.pending, true, "A retired operation cannot release the new operation");
        assert.equal(value.error, "");
        await act(async () => { next.resolve({ detail: "new saved", snapshot: options.snapshot }); await second; });
        assert.equal(value.status, "saved");
      }
      assert.equal(saved.includes(initial), false);
    });
  }
});

test("MIDI Out Clear all stops before the next source after its session changes", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const old = deferred();
  const message = { type: 1, channel: 1, param1: 5, param2: 0, param3: 0 };
  const initial = { ...demoSnapshot, presetIdentity: "preset-a", midiOut: [{ source: 0, messages: [message] }, { source: 1, messages: [message] }] };
  let writes = 0, value, renderer;
  const saved: unknown[] = [];
  let options = { gateway: { setMidiOut: () => { writes++; return old.promise; } }, snapshot: initial, onSaved: (snapshot) => saved.push(snapshot) };
  function Harness() { value = usePresetMidiOutWorkflow(options); return null; }
  await act(async () => { renderer = create(React.createElement(Harness)); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  let work;
  await act(async () => { work = value.clearAll(); });
  await act(async () => { options = { ...options, snapshot: { ...initial, presetIdentity: "preset-b" } }; renderer.update(React.createElement(Harness)); });
  await act(async () => { old.resolve({ detail: "old clear", snapshot: initial }); await work; });
  assert.equal(writes, 1);
  assert.deepEqual(saved, []);
  assert.equal(value.status, null);
});

test("retained MIDI actions cannot acquire a replacement owner while fresh actions remain usable", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  for (const boundary of ["route", "route-ABA", "session", "preset"] as const) {
    await t.test(boundary, async () => {
      const old = deferred();
      const initial = { ...demoSnapshot, presetIdentity: "action-a",
        midiOut: [{source: 0, messages: [{type: 1, channel: 1, param1: 7, param2: 90, param3: 0}]}] };
      const lifetime = new PresetLifetime(initial);
      const saved: unknown[] = [];
      let writes = 0, value, renderer;
      const firstTarget = { setMidiOut: () => dispatch() };
      const nextTarget = { setMidiOut: () => dispatch() };
      let active = firstTarget, revision = 0;
      const gateway = { setMidiOut: () => active.setMidiOut() };
      registerGatewayRoute(gateway, { active: () => active, revision: () => revision });
      let options = { gateway, lifetime, snapshot: initial,
        captureSessionCurrent: () => captureGatewaySession(gateway).current,
        onSaved: (snapshot) => saved.push(snapshot) };
      const dispatch = () => ++writes === 1 ? old.promise : Promise.resolve({snapshot: options.snapshot, detail: "current saved"});
      function Harness() { value = usePresetMidiOutWorkflow(options); return null; }
      await act(async () => { renderer = create(React.createElement(Harness)); });
      let first;
      try {
        const retiredSave = value.save, retiredClear = value.clearAll;
        await act(async () => { first = retiredSave(0, []); });
        assert.equal(writes, 1);
        await act(async () => {
          if (boundary.startsWith("route")) { active = nextTarget; revision++; }
          else if (boundary === "session") lifetime.reset();
          else options = {...options, snapshot: {...initial, presetIdentity: "action-b"}};
          renderer.update(React.createElement(Harness));
        });
        if (boundary === "route-ABA") await act(async () => {
          active = firstTarget; revision++;
          renderer.update(React.createElement(Harness));
        });
        assert.notEqual(value.save, retiredSave, "the replacement receives its own action callbacks");
        assert.notEqual(value.clearAll, retiredClear);
        await act(async () => { old.resolve({snapshot: initial, detail: "obsolete saved"}); await first; });
        assert.deepEqual(saved, [], "old completion cannot reconcile the replacement preset");
        assert.equal(value.status, null);
        assert.equal(value.error, "");
        await act(async () => { await retiredSave(1, []); await retiredClear(); });
        assert.equal(writes, 1, "retained actions are refused before native dispatch");
        await act(async () => { await value.save(1, []); });
        assert.equal(writes, 2, "a current action still acquires and completes normally");
        assert.deepEqual(saved, [options.snapshot]);
        assert.equal(value.status, "saved");
        assert.equal(value.pending, false);
      } finally {
        await act(async () => { old.resolve({}); if (first) await first; renderer.unmount(); });
      }
    });
  }
});
