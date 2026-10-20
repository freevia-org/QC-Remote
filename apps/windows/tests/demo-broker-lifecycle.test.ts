import assert from "node:assert/strict";
import test from "node:test";
import { createDemoBrokerLifecycle } from "../src/demo-broker-lifecycle.ts";

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
