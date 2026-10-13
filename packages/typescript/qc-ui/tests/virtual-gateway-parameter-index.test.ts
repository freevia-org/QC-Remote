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
