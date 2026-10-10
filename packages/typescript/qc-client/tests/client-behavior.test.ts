import assert from "node:assert/strict";
import test from "node:test";
import { normalizeBrokerConnection, requireModelId, requirePresetIdentity, withDeviceConnectionSource, type GatewayTransport } from "../src/index.ts";

const state = (phase: "ready" | "disconnected") => ({ phase, detail: phase, demo: false, source: "virtual" as const });

test("connection source wrapper stamps only lifecycle results as device-owned", async () => {
  const transport = {
    reconnect: async () => state("ready"),
    resetSession: async () => state("disconnected"),
    disconnect: async () => state("disconnected"),
    currentSnapshot: async () => ({ presetName: "unchanged" })
  } as unknown as GatewayTransport;
  const wrapped = withDeviceConnectionSource(transport);
  assert.equal((await wrapped.reconnect()).source, "device");
  assert.equal((await wrapped.resetSession()).source, "device");
  assert.equal((await wrapped.disconnect()).source, "device");
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
