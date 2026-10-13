import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { createMemoryVirtualDeviceStorage, createVirtualGatewayTransport, VirtualDeviceStore } from "../src/virtual-gateway.ts";
import type { VirtualModelCatalogue } from "../src/virtual-device-defaults.ts";

const catalogue = JSON.parse(
  readFileSync(new URL("../../../../docs/reference/qc-live-parameter-scales.json", import.meta.url), "utf8")
) as VirtualModelCatalogue;

async function openVirtual() {
  const store = await VirtualDeviceStore.open({
    storage: createMemoryVirtualDeviceStorage(), catalogue, now: () => 0, newSessionId: () => "session-1"
  });
  return createVirtualGatewayTransport(store);
}

test("a parameter index that is not a small integer is refused and Object.prototype stays untouched", async () => {
  const gateway = await openVirtual();
  const snapshot = await gateway.currentSnapshot();
  const block = snapshot.blocks.find((candidate) => candidate.parameterValues !== undefined && 0 in candidate.parameterValues);
  assert.ok(block, "the demo preset has a block with parameter 0");
  const { row, column, modelId } = block;
  const name = snapshot.presetName;
  const identity = snapshot.presetIdentity!;
  // Per-scene mode makes `sceneValues` an object that a "__proto__" key would resolve on.
  await gateway.setParameterSceneMode(row, column, 0, true, modelId, name, identity);

  const before = Object.getOwnPropertyNames(Object.prototype).sort();
  for (const bad of ["__proto__", "constructor", 0.5, -1, 100_001, Number.NaN, Number.POSITIVE_INFINITY]) {
    await assert.rejects(
      gateway.setParameter(row, column, bad as unknown as number, 0.25, 0.5, snapshot.activeScene, modelId, name, identity),
      undefined,
      `setParameter must refuse the index ${String(bad)}`
    );
    await assert.rejects(
      gateway.previewParameter(row, column, bad as unknown as number, 0.25, 0.5, snapshot.activeScene, modelId, name, identity),
      undefined,
      `previewParameter must refuse the index ${String(bad)}`
    );
  }
  assert.deepEqual(Object.getOwnPropertyNames(Object.prototype).sort(), before, "Object.prototype gained or lost no property");
  assert.equal(({} as Record<number, unknown>)[3], undefined, "an unrelated object inherits no scene value");
  assert.equal(({} as Record<string, unknown>)["0"], undefined);

  // Control: the same write with a real index still works.
  const written = await gateway.setParameter(row, column, 0, 0.25, 0.5, snapshot.activeScene, modelId, name, identity);
  assert.equal(written.accepted, true);
});

test("parameter writes reject prototype-shaped scene keys even if internal state is corrupted", async () => {
  const store = await VirtualDeviceStore.open({ storage: createMemoryVirtualDeviceStorage(), catalogue, now: () => 0, newSessionId: () => "scene-guard" });
  const gateway = createVirtualGatewayTransport(store);
  const snapshot = await gateway.currentSnapshot();
  const name = snapshot.presetName, identity = snapshot.presetIdentity!;
  const block = snapshot.blocks.find((candidate) => candidate.parameterValues !== undefined && 0 in candidate.parameterValues);
  assert.ok(block);
  const before = block.parameterValues![0];
  await store.update((state) => { state.current.preset.activeScene = "__proto__" as unknown as number; });
  await assert.rejects(gateway.setParameter(block.row, block.column, 0, 0.2, before, "__proto__" as unknown as number, block.modelId, name, identity), /scene must be a valid scene index/);
  assert.equal(({} as { polluted?: unknown }).polluted, undefined);
});

test("virtual block reset applies measured defaults to active and inactive scenes while retaining Scene mode", async () => {
  const measuredCatalogue = { models: [{ id: 1_001, name: "Amp", category: "Guitar Amplifier", hidden: false,
    parameters: [0, 1].map((index) => ({ index, name: `Parameter ${index}`, type: "float", minimum: 0, maximum: 1, valueScale: "linear", options: [] })) }] } as VirtualModelCatalogue;
  const storage = createMemoryVirtualDeviceStorage();
  const store = await VirtualDeviceStore.open({ storage, catalogue: measuredCatalogue,
    measuredDefaults: { schema: 1, source: "device-derived compatibility data", models: {
      "1001": { name: "Amp", category: "Guitar Amplifier", parameters: [{ index: 0, name: "Scene parameter", normalizedValue: .2 }, { index: 1, name: "Ordinary parameter", normalizedValue: .3 }] }
    } }, newSessionId: () => "reset-test" });
  const gateway = createVirtualGatewayTransport(store);
  const initial = await gateway.currentSnapshot();
  const block = initial.blocks.find((block) => block.modelId === 1_001)!;
  assert.ok(block);
  await store.update((state) => {
    const target = state.current.preset.blocks.find((target) => target.modelId === 1_001)!;
    target.parameterValues = { 0: .7, 1: .9 };
    target.sceneValues = { 0: [.8, .6, .4, .9, .5, .7, .1, 1] };
    target.expressions = { 0: { pedal: 2, minimum: .1, maximum: .9 } };
  });
  const result = await gateway.resetBlockParameters(block.row, block.column, block.modelId!, initial.presetName, initial.presetIdentity!);
  assert.equal(result.verified, true);
  for (const scene of [initial.activeScene, (initial.activeScene + 1) % 8]) {
    await gateway.selectScene(scene, initial.presetName, initial.presetIdentity!);
    const readback = await gateway.blockDetails(block.row, block.column, initial.presetName, initial.presetIdentity!);
    assert.equal(readback.parameters.find((parameter) => parameter.index === 0)?.normalizedValue, .2);
    assert.equal(readback.parameters.find((parameter) => parameter.index === 0)?.sceneMode, true);
    assert.equal(readback.parameters.find((parameter) => parameter.index === 1)?.normalizedValue, .3);
    assert.equal(readback.parameters.find((parameter) => parameter.index === 0)?.expression, 2);
  }
  const persisted = JSON.parse(storage.text!);
  const stored = persisted.current.preset.blocks.find((target) => target.modelId === 1_001);
  assert.deepEqual(stored.sceneValues[0], Array(8).fill(.2));
});
