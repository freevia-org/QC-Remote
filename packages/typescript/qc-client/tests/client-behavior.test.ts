import assert from "node:assert/strict";
import test from "node:test";
import { brokerTargetKey, selectedBrokerTarget, normalizeBrokerConnection, requireModelId, requirePresetIdentity, withDeviceConnectionSource, type GatewayTransport } from "../src/index.ts";

const state = (phase: "ready" | "disconnected") => ({ phase, detail: phase, demo: false, source: "virtual" as const });

test("connection source wrapper preserves explicit virtual lifecycle results", async () => {
  const transport = {
    reconnect: async () => state("ready"),
    resetSession: async () => state("disconnected"),
    disconnect: async () => state("disconnected"),
    currentSnapshot: async () => ({ presetName: "unchanged" })
  } as unknown as GatewayTransport;
  const wrapped = withDeviceConnectionSource(transport);
  assert.equal((await wrapped.reconnect()).source, "virtual");
  assert.equal((await wrapped.resetSession()).source, "virtual");
  assert.equal((await wrapped.disconnect()).source, "virtual");
  assert.deepEqual(await wrapped.currentSnapshot(), { presetName: "unchanged" });
});

test("broker phase aliases normalize and unknown phases stay attention-worthy", () => {
  const connection = (phase: string) => ({ phase, detail: phase, demo: false, source: "device" as const }) as Parameters<typeof normalizeBrokerConnection>[0];
  assert.equal(normalizeBrokerConnection(connection("ready")).phase, "ready");
  assert.equal(normalizeBrokerConnection(connection("error")).phase, "needs-attention");
  const connecting = normalizeBrokerConnection(connection("connecting"));
  assert.equal(connecting.phase, "opening");
  assert.equal(connecting.demo, true);
  assert.equal(normalizeBrokerConnection(connection("future-phase")).phase, "needs-attention");
});

test("model identity validation accepts positive signed 32-bit ids and rejects unsafe targets", () => {
  assert.equal(requireModelId({ modelId: 1 }), 1);
  assert.equal(requireModelId({ modelId: 0x7fffffff }), 0x7fffffff);
  for (const target of [undefined, {}, { modelId: 0 }, { modelId: -1 }, { modelId: 1.5 }, { modelId: 0x80000000 }, { modelId: null }]) {
    assert.throws(() => requireModelId(target), /model is unavailable/);
  }
});

test("preset identity must be captured before a mutation", () => {
  assert.equal(requirePresetIdentity({ presetIdentity: "setlist:3:preset:8" }), "setlist:3:preset:8");
  for (const snapshot of [{}, { presetIdentity: "" }]) {
    assert.throws(() => requirePresetIdentity(snapshot), /identity is unavailable/);
  }
});


test("legacy native lifecycle results default to device without deriving source from demo", async () => {
  for (const demo of [true, false]) {
    const legacy = async () => ({ phase: "ready", detail: "Legacy native connection", demo });
    const wrapped = withDeviceConnectionSource({ reconnect: legacy, resetSession: legacy, disconnect: legacy } as unknown as GatewayTransport);
    for (const method of ["reconnect", "resetSession", "disconnect"] as const) {
      assert.equal((await wrapped[method]()).source, "device");
    }
  }
});

test("unknown explicit sources fail closed instead of becoming physical", async () => {
  const invalid = async () => ({ ...state("ready"), source: "future" });
  const wrapped = withDeviceConnectionSource({ reconnect: invalid, resetSession: invalid, disconnect: invalid } as unknown as GatewayTransport);
  for (const method of ["reconnect", "resetSession", "disconnect"] as const) await assert.rejects(wrapped[method](), /source is unsupported/);
});


test("broker target keys validate kind and generation and distinguish a return to the same provider", () => {
  const demo = { source: "demo" as const, sessionId: "provider", generation: 1 };
  assert.equal(brokerTargetKey(undefined), undefined);
  assert.notEqual(brokerTargetKey(demo), brokerTargetKey({ ...demo, generation: 3 }));
  assert.notEqual(brokerTargetKey(demo), brokerTargetKey({ ...demo, source: "physical" }));
  for (const invalid of [null, {}, { ...demo, source: "virtual" }, { ...demo, sessionId: " " }, { ...demo, generation: 0 }, { ...demo, generation: 1.5 }, { ...demo, generation: Number.MAX_SAFE_INTEGER + 1 }]) {
    assert.throws(() => brokerTargetKey(invalid as any), /identity is unsupported/);
  }
});


test("effective readiness cannot revive absent or withdrawn target metadata", () => {
  const target = { source: "demo" as const, sessionId: "provider", generation: 1 };
  assert.equal(selectedBrokerTarget({}), undefined);
  assert.equal(selectedBrokerTarget({ targetReady: false, brokerTarget: target }), undefined);
  assert.equal(selectedBrokerTarget({ targetReady: true, brokerTarget: target }), target);
  assert.throws(() => selectedBrokerTarget({ targetReady: true }), /has no identity/);
  assert.throws(() => selectedBrokerTarget({ targetReady: "yes" } as any), /readiness is unsupported/);
});
