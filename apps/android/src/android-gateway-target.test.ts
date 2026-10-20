import assert from "node:assert/strict";
import test from "node:test";
import type { PresetSnapshot } from "@qc-remote/client";
import { readAndroidPhysicalTarget, type AndroidBrokerTarget, type AndroidGatewayRuntimeStatus } from "./android-gateway-target.ts";

const target: AndroidBrokerTarget = { source: "physical", sessionId: "native-session", generation: 1 };
const status = (brokerTarget = target): AndroidGatewayRuntimeStatus => ({
  platform: "Android", gatewayAvailable: true, message: "Ready", connected: true, synchronized: true, brokerTarget
});
const snapshot = { presetName: "Physical preset" } as PresetSnapshot;

test("physical discovery brackets a pinned snapshot without adopting the target", async () => {
  let reads = 0;
  const accepted: AndroidBrokerTarget[] = [];
  const observation = await readAndroidPhysicalTarget({
    async status() { reads++; return status(); },
    async snapshot(captured) { assert.deepEqual(captured, target); return snapshot; },
    accept: (captured) => accepted.push(captured)
  });
  assert.equal(reads, 2);
  assert.equal(observation.connection.phase, "ready");
  assert.equal(observation.snapshot, snapshot);
  assert.deepEqual(accepted, []);
  observation.accept?.();
  assert.deepEqual(accepted, [target]);
  assert.ok(Object.isFrozen(accepted[0]));
});

test("discovery refuses an intervening physical target replacement", async () => {
  let reads = 0;
  let adopted = false;
  await assert.rejects(readAndroidPhysicalTarget({
    async status() { return status(++reads === 1 ? target : { ...target, generation: 2 }); },
    async snapshot() { return snapshot; },
    accept() { adopted = true; }
  }), { code: "TARGET_SUPERSEDED" });
  assert.equal(adopted, false);
});

test("native demo or incomplete physical state cannot become a physical UI observation", async () => {
  for (const result of [status({ ...target, source: "demo" }), { ...status(), synchronized: false }, { ...status(), connected: false }]) {
    const observation = await readAndroidPhysicalTarget({
      async status() { return result; },
      async snapshot() { throw new Error("No physical read should be admitted"); },
      accept() { throw new Error("No physical target should be adopted"); }
    });
    assert.equal(observation.connection.phase, "disconnected");
    assert.equal(observation.accept, undefined);
  }
});

test("failed physical snapshot reads never adopt or retry another target", async () => {
  let reads = 0;
  await assert.rejects(readAndroidPhysicalTarget({
    async status() { reads++; return status(); },
    async snapshot() { throw new Error("target was removed"); },
    accept() { throw new Error("Must not adopt on failure"); }
  }), /target was removed/);
  assert.equal(reads, 1);
});
