import assert from "node:assert/strict";
import test from "node:test";
import { createDemoBrokerInstanceObservation, createDemoBrokerRecovery, createDemoBrokerLifecycle, rebindDemoBrokerFromObservation, waitForDemoBrokerRegistration } from "../src/demo-broker-lifecycle.ts";

for (const admission of ["inventory", "instance-change"] as const) test(`failed automatic ${admission} registration is terminal across inventory ticks until explicit reset`, async () => {
  let serial = 0;
  let starts = 0;
  let failed!: () => void;
  const failure = new Promise<void>(resolve => { failed = resolve; });
  const recovery = createDemoBrokerRecovery(() => lifecycle.rebind(true));
  const lifecycle = createDemoBrokerLifecycle({
    newId: () => `worker-${++serial}`,
    invalidate() {},
    stop: async () => undefined,
    start: async () => { starts += 1; throw new Error("registration refused"); },
    failed() { recovery.failed(); failed(); }
  });
  if (admission === "inventory") {
    recovery.failed(); // Unexpected loss of the previously acknowledged provider.
    recovery.observed("same-broker");
  } else {
    rebindDemoBrokerFromObservation(recovery, "same-broker", false, available => lifecycle.rebind(available));
  }
  await failure;
  const observe = createDemoBrokerInstanceObservation(instance => {
    rebindDemoBrokerFromObservation(recovery, instance, false, available => lifecycle.rebind(available));
  });
  for (let tick = 0; tick < 100; tick += 1) {
    recovery.failed(); // Duplicate native status must not re-arm a terminal failure.
    recovery.observed(undefined);
    recovery.observed("same-broker");
    assert.equal(recovery.changed(undefined), false);
    assert.equal(recovery.changed("same-broker"), false);
    observe(null);
    observe(undefined);
    observe("same-broker");
    await Promise.resolve();
  }
  assert.equal(starts, 1);
  assert.ok(serial >= 2); // Absence may invalidate ids/retire, but cannot spawn.
  assert.equal(recovery.changed("new-broker"), true);
  // A new explicit selection constructs a fresh recovery owner.
  let explicitRetries = 0;
  const explicit = createDemoBrokerRecovery(() => { explicitRetries += 1; });
  explicit.failed();
  explicit.observed("same-broker");
  assert.equal(explicitRetries, 1);
  await lifecycle.dispose();
});

test("failed Demo recovery waits for existing inventory and never retries while registration is pending", () => {
  let retries = 0;
  const recovery = createDemoBrokerRecovery(() => { retries += 1; });
  recovery.failed();
  recovery.observed(undefined);
  recovery.observed(null);
  recovery.observed("broker", true);
  assert.equal(retries, 0);
  recovery.observed("broker");
  recovery.observed("broker");
  assert.equal(retries, 1);
  recovery.failed();
  recovery.ready();
  recovery.observed("broker");
  assert.equal(retries, 1);
});

test("a failed inventory read keeps the explicit Demo provider; confirmed replacement or absence retires it", async () => {
  const calls: string[] = [];
  let serial = 0;
  let selected = "Demo";
  const lifecycle = createDemoBrokerLifecycle({
    newId: () => `provider-${++serial}`,
    start: async id => { calls.push(`start:${id}`); },
    stop: async id => { calls.push(`stop:${id}`); },
    invalidate: () => { calls.push("invalidate"); },
    failed: error => assert.fail(String(error))
  });
  const observe = createDemoBrokerInstanceObservation(instance => {
    if (!instance) selected = "Disconnected";
    lifecycle.rebind(Boolean(instance));
  });
  observe("broker-A");
  await lifecycle.start();
  const original = lifecycle.id;
  const before = [...calls];
  observe(undefined); // App's inventory catch: Unknown, not broker loss.
  observe(undefined);
  observe("broker-A");
  await Promise.resolve();
  assert.equal(selected, "Demo");
  assert.equal(lifecycle.id, original);
  assert.deepEqual(calls, before);
  observe("broker-B");
  await lifecycle.start();
  assert.notEqual(lifecycle.id, original);
  assert.ok(calls.includes(`start:${lifecycle.id}`)); // Native replace retires the old worker.
  observe(null); // Successful inventory explicitly reports no broker.
  await lifecycle.dispose();
  assert.equal(selected, "Disconnected");
  assert.ok(calls.includes(`stop:provider-${serial - 1}`));
});

test("native startup alone does not publish Demo before the broker acknowledges it", async () => {
  let observe!: (ready: boolean, message?: string) => void;
  let cleaned = false;
  let complete = false;
  const pending = waitForDemoBrokerRegistration({
    start: async () => undefined,
    subscribe: listener => { observe = listener; return () => { cleaned = true; }; }
  }).then(() => { complete = true; });
  await Promise.resolve();
  assert.equal(complete, false);
  observe(true);
  await pending;
  assert.equal(complete, true);
  assert.equal(cleaned, true);
});

test("provider loss or cancellation after ready remains terminal until native startup completes", async () => {
  for (const reason of ["provider lost", "selection ended"]) {
    let observe!: (ready: boolean, message?: string) => void;
    let release!: () => void;
    let cleaned = false;
    const start = new Promise<void>(resolve => { release = resolve; });
    const pending = waitForDemoBrokerRegistration({
      start: () => start,
      subscribe: listener => { observe = listener; return () => { cleaned = true; }; }
    });
    observe(true);
    observe(false, reason);
    observe(true); // A later stale positive cannot undo terminal loss.
    release();
    await assert.rejects(pending, error => error instanceof Error && error.message === reason);
    assert.equal(cleaned, true);
  }
});

test("failed or timed-out registration cleans its observation and never reports Demo ready", async () => {
  for (const mode of ["native-error", "broker-error", "timeout"]) {
    let observe!: (ready: boolean, message?: string) => void;
    let cleaned = false;
    const pending = waitForDemoBrokerRegistration({
      timeoutMs: 10,
      start: async () => { if (mode === "native-error") throw new Error("native startup failed"); },
      subscribe: listener => { observe = listener; return () => { cleaned = true; }; }
    });
    if (mode === "broker-error") observe(false, "registration refused");
    await assert.rejects(pending, mode === "native-error" ? /native startup failed/ : mode === "broker-error" ? /registration refused/ : /did not acknowledge/);
    assert.equal(cleaned, true, mode);
  }
});

test("leaving Demo while its registration is pending retires it before replacement work", async () => {
  let observe!: (ready: boolean, message?: string) => void;
  let entered!: () => void;
  const entering = new Promise<void>(resolve => { entered = resolve; });
  const stopped: string[] = [];
  const lifecycle = createDemoBrokerLifecycle({
    newId: () => "demo-opening", invalidate() {}, failed() {},
    start: () => waitForDemoBrokerRegistration({
      start: async () => { entered(); },
      subscribe: listener => { observe = listener; return () => undefined; }
    }),
    stop: async id => { stopped.push(id); }
  });
  const opening = lifecycle.start();
  await entering;
  const rejected = assert.rejects(opening, /selection ended/);
  observe(false, "selection ended");
  await lifecycle.dispose();
  await rejected;
  assert.equal(lifecycle.isCurrent("demo-opening"), false);
  assert.deepEqual(stopped, ["demo-opening"]);
});

test("broker restart uses a fresh worker identity so late old status/replies are refused", async () => {
  let serial = 0;
  const started: string[] = [];
  const stopped: string[] = [];
  let replacement!: () => void;
  const replaced = new Promise<void>(resolve => { replacement = resolve; });
  const lifecycle = createDemoBrokerLifecycle({
    newId: () => `worker-${++serial}`, invalidate() {}, failed(error) { throw error; },
    start: async id => { started.push(id); if (id === "worker-2") replacement(); },
    stop: async id => { stopped.push(id); }
  });
  const old = lifecycle.id;
  await lifecycle.start();
  lifecycle.rebind(true);
  assert.equal(lifecycle.isCurrent(old), false);
  await replaced;
  assert.equal(lifecycle.isCurrent("worker-2"), true);
  await lifecycle.dispose();
  assert.deepEqual(started, ["worker-1", "worker-2"]);
  assert.deepEqual(stopped, ["worker-2"]); // Native replace retires worker-1.
});

test("closing during pending startup retires its exact worker after acknowledgement", async () => {
  let entered!: () => void;
  let release!: () => void;
  const started = new Promise<void>(resolve => { entered = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  const stopped: string[] = [];
  const lifecycle = createDemoBrokerLifecycle({
    newId: () => "worker-1", invalidate() {}, failed(error) { throw error; },
    start: async () => { entered(); await gate; }, stop: async id => { stopped.push(id); }
  });
  const opening = lifecycle.start();
  await started;
  const closed = lifecycle.dispose();
  release();
  await Promise.all([opening, closed]);
  assert.equal(lifecycle.isCurrent("worker-1"), false);
  assert.deepEqual(stopped, ["worker-1"]);
});

test("failed IPC startup is cleaned up without replaying registration or device work", async () => {
  const stopped: string[] = [];
  const failures: unknown[] = [];
  let starts = 0;
  const lifecycle = createDemoBrokerLifecycle({
    newId: () => "worker-1", invalidate() {}, failed: error => { failures.push(error); },
    start: async () => { starts += 1; throw new Error("reply lost after native startup"); },
    stop: async id => { stopped.push(id); }
  });
  await assert.rejects(lifecycle.start(), /reply lost/);
  await lifecycle.dispose();
  assert.equal(starts, 1);
  assert.deepEqual(stopped, ["worker-1"]);
  assert.equal(failures.length, 1);
});
