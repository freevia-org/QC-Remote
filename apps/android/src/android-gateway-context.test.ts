import assert from "node:assert/strict";
import test from "node:test";
import { createAndroidGatewayContext, type AndroidGatewayEnvelope } from "./android-gateway-context.ts";
import type { AndroidBrokerTarget } from "./android-gateway-target.ts";

const first: AndroidBrokerTarget = { source: "physical", sessionId: "accepted", generation: 1 };

test("managed calls always require native accepted-target admission, including after retirement", async () => {
  const calls: AndroidGatewayEnvelope[] = [];
  const context = createAndroidGatewayContext(async <T,>(request: AndroidGatewayEnvelope) => { calls.push(request); return {} as T; });
  await context.invoke("device.setMasterVolume", { value: 10 });
  context.accept(first);
  await context.invoke("device.setMasterVolume", { value: 20 });
  context.accept(undefined);
  await context.invoke("device.setMasterVolume", { value: 30 });
  assert.deepEqual(calls.map((call) => call.requireAcceptedBrokerTarget), [true, true, true]);
  assert.equal(calls[0].expectedBrokerTarget, undefined);
  assert.deepEqual(calls[1].expectedBrokerTarget, first);
  assert.equal(calls[2].expectedBrokerTarget, undefined);
});

test("a latest status reply cannot silently replace the UI-accepted context", async () => {
  const calls: AndroidGatewayEnvelope[] = [];
  const latest = { ...first, generation: 2 };
  const context = createAndroidGatewayContext(async <T,>(request: AndroidGatewayEnvelope) => {
    calls.push(request); return { brokerTarget: latest } as T;
  });
  const offered = { ...first };
  context.accept(offered);
  offered.generation = 99;
  await context.invoke("system.status");
  await context.invoke("device.setTempo", { bpm: 120 });
  assert.deepEqual(calls[1].expectedBrokerTarget, first);
  assert.ok(Object.isFrozen(calls[1].expectedBrokerTarget));
});

test("native refusal is propagated once without discovery, retry or target adoption", async () => {
  const failure = Object.assign(new Error("The accepted target was retired"), { code: "TARGET_SUPERSEDED" });
  let calls = 0;
  const context = createAndroidGatewayContext(async () => { calls++; throw failure; });
  context.accept(first);
  await assert.rejects(context.invoke("device.setMasterVolume", { value: 40 }), (error) => error === failure);
  assert.equal(calls, 1);
  assert.throws(() => context.accept({ ...first, source: "demo" }), TypeError);
});
