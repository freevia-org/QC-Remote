import assert from "node:assert/strict";
import test from "node:test";
import { demoSnapshot } from "../../qc-core/src/device-reference/demo-snapshot.ts";
import { capturePresetText, captureSetlistForDemo } from "../src/preset-transfer.ts";

const block = { id: "amp", modelId: 42, name: "Amp", kind: "amp" as const, row: 0, column: 0 };
const snapshot = { ...demoSnapshot, dirty: false, presetIdentity: "id", activeScene: 2, blocks: [block] };
const details = { ...block, category: "Amp", scene: 2, parameters: [{ index: 0, writable: true, normalizedValue: .3 }] };

test("preset text rejects values read from a different block address, model or scene", async () => {
  for (const mismatch of [{ row: 1 }, { column: 1 }, { modelId: 43 }, { scene: 3 }]) {
    const result = await capturePresetText({ snapshot, source: "device", gateway: { blockDetails: async () => ({ ...details, ...mismatch }) } as never });
    assert.equal(result.document.parameterValues, "none");
    assert.equal((result.document.snapshot.blocks[0] as { parameterValues?: unknown }).parameterValues, undefined);
    assert.match(result.summary, /could not be read/);
    assert.match(result.summary, /scene changed/);
  }
  const healthy = await capturePresetText({ snapshot, source: "device", gateway: { blockDetails: async () => details } as never });
  assert.equal(healthy.document.parameterValues, "complete");
  assert.deepEqual((healthy.document.snapshot.blocks[0] as { parameterValues?: unknown }).parameterValues, { 0: .3 });
});

test("whole-setlist capture refuses mixed block or lane observations and restores the original scene", async () => {
  for (const mismatch of ["block-row", "block-column", "block-model", "block-scene", "lane-row", "lane-scene", "none"]) {
    let current = snapshot;
    const scenesSelected: number[] = [];
    const gateway = {
      identity: async () => ({ serial: "test-unit" }),
      currentSnapshot: async () => current,
      listPresets: async () => ({ setlistName: snapshot.setlistName, presets: [{ position: snapshot.presetPosition, location: snapshot.presetLocation, name: snapshot.presetName, occupied: true }] }),
      selectScene: async (scene: number) => { scenesSelected.push(scene); current = { ...current, activeScene: scene }; return { snapshot: current }; },
      blockDetails: async () => ({ ...details, scene: current.activeScene,
        ...(mismatch === "block-row" ? { row: 1 } : mismatch === "block-column" ? { column: 1 } : mismatch === "block-model" ? { modelId: 43 } : mismatch === "block-scene" ? { scene: (current.activeScene + 1) % 8 } : {}) }),
      laneControlDetails: async (row: number) => ({ ...details, row: mismatch === "lane-row" ? row + 1 : row,
        scene: mismatch === "lane-scene" ? (current.activeScene + 1) % 8 : current.activeScene })
    };
    const run = captureSetlistForDemo({ gateway: gateway as never, setlistKey: snapshot.setlistKey, setlistName: snapshot.setlistName });
    if (mismatch === "none") {
      const result = await run;
      assert.equal(result.presets[0].scenes.length, 8);
      assert.equal(result.presets[0].scenes[7].blocks["0:0"].scene, 7);
    } else await assert.rejects(run, /scene changed/, mismatch);
    assert.equal(current.activeScene, snapshot.activeScene, `${mismatch}: original scene restored`);
    assert.equal(scenesSelected.at(-1), snapshot.activeScene);
  }
});
