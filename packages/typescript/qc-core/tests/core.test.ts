import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import test from "node:test";
import type { BlockDetails } from "../../qc-client/src/index.ts";
import { demoSnapshot } from "../src/device-reference/demo-snapshot.ts";
import { blockSelectionIntent, emptyBlockEditorSession, parameterDrafts, reduceBlockEditorSession, updateBlockParameter } from "../src/editor.ts";
import { applyFootswitchPreview, footswitchIntent, footswitchLeds, optimisticallyPressFootswitch } from "../src/footswitch.ts";
import { applyQcStateUpdate, applyQcStateUpdates, canAdoptNativeState, clearPendingBypassChanges, markPendingBypass, movePresetInSnapshot, reconcilePendingBypass, reconcilePresetSnapshot, reconcileQcStateUpdates, recordPendingBypassChanges, reduceQcStateFrame, selectModeSlotInSnapshot, setBlockBypassInSnapshot, setTempoInSnapshot, type PendingBypassIntents } from "../src/state.ts";
import { dispatchSurfaceCommand, surfaceCommand } from "../src/surface-actions.ts";
import type { ModelPresetCatalogState, ModelPresetSummary } from "../../qc-client/src/index.ts";
import { buildWorkspaceDocument, saveWorkspaceDocument, type WorkspaceWriter } from "../src/workspace-save.ts";
import { admitGlobalEqPreset, adoptModelPresetCatalog, globalEqPresetsFor, globalEqPresetsFrom, globalEqUnavailableNotice, withoutModelPresetCatalog, type GlobalEqPresets } from "../src/model-preset-catalog.ts";
import { recordTempoTap, synchronizeTempoPulseEpoch } from "../src/tempo.ts";

import { createQcGatewayTransport } from "../src/gateway-transport.ts";
import type { GatewayTransport } from "../../qc-client/src/index.ts";

test("UP and DOWN recall adjacent slots with the loaded preset guards", async () => {
  const loaded = { ...demoSnapshot, presetName: "Loaded preset", presetPosition: 53, setlistKey: "/user/Live", presetIdentity: "loaded-generation" };
  const calls: unknown[][] = [];
  const gateway = { recallPreset: async (...args: unknown[]) => { calls.push(args); return { detail: "accepted" }; } } as unknown as GatewayTransport;
  const transport = createQcGatewayTransport(gateway, () => ({ ...loaded, presetName: "Newer preset", presetIdentity: "newer-generation" }));
  for (const delta of [-1, 1] as const) {
    await transport.movePreset(delta, loaded);
    assert.deepEqual(calls.at(-1), [loaded.setlistKey, loaded.presetPosition + delta, loaded.presetName, loaded.presetPosition, loaded.setlistKey, loaded.presetIdentity]);
  }
});

test("tempo pulse phase follows the beat message, and tick 1 is only a pulse flag", () => {
  // Observed on a CorOS 4.1.0 unit (2026-10-07): one message at each beat start
  // (tick absent or 0) and one with tick 1 about 23 ms later at 120 BPM.
  const epoch = synchronizeTempoPulseEpoch(undefined, 10_000, 0, 120);
  assert.equal(epoch, 10_000, "the first beat message is the beat origin");
  assert.equal(synchronizeTempoPulseEpoch(epoch, 10_023, 1, 120), epoch, "the tick-1 flag 23ms after the beat must not move the lamp");
  assert.equal(synchronizeTempoPulseEpoch(epoch, 10_500, 0, 120), epoch);
  assert.equal(synchronizeTempoPulseEpoch(epoch, 10_523, 1, 120), epoch);
  assert.equal(synchronizeTempoPulseEpoch(epoch, 10_570, 0, 120), 10_070);
  // A received beat message is authoritative. Keeping even a small phase delta
  // makes the WebView visibly lag the physical lamp over repeated beats.
  assert.equal(synchronizeTempoPulseEpoch(epoch, 10_504, 0, 120), 10_004, "a 4ms USB pulse shift must move the lamp");
  assert.equal(synchronizeTempoPulseEpoch(epoch, 10_537, 0, 120), 10_037, "37ms of drift must be corrected");
});
test("tempo pulse epoch is never derived from a tick count", () => {
  // The old model divided the tick by 24, so tick 1 put the origin period/24
  // before the message and tick 12 half a beat before it.
  // No ticks-per-beat figure exists on the unit: any non-zero tick is a flag.
  for (const tick of [1, 2, 12, 21, 23, 24, 25, 99]) {
    assert.equal(synchronizeTempoPulseEpoch(10_000, 10_100 + tick, tick, 120), 10_000, `tick ${tick} must leave the epoch alone`);
    assert.equal(synchronizeTempoPulseEpoch(undefined, 10_100 + tick, tick, 120), undefined, `tick ${tick} cannot start an epoch`);
  }
  // Without an epoch a flag message waits for the next beat message.
  assert.equal(synchronizeTempoPulseEpoch(undefined, 10_023, 1, 120), undefined);
  assert.equal(synchronizeTempoPulseEpoch(Number.NaN, 10_023, 1, 120), undefined);
  assert.equal(synchronizeTempoPulseEpoch(Number.NaN, 10_500, 0, 120), 10_500);
  // The beat period still follows the tempo when the phase is wrapped.
  assert.ok(Math.abs((synchronizeTempoPulseEpoch(10_000, 10_000 + 60_000 / 212 + 5, 0, 212) ?? Number.NaN) - 10_005) < 1e-6);
  // A beat message on the other side of a boundary maps to the nearest beat.
  assert.equal(synchronizeTempoPulseEpoch(10_000, 10_000 + 500 - 6, 0, 120), 10_000 - 6);
});
import { QcCommandCoordinator } from "../src/command-coordinator.ts";
import { inputRouteOptions, routeOptionValue, routeOptionsForRow, routePickerGroup, routePickerLabel } from "../src/routing.ts";
import { controlAccessPermitsAction, parseControlAccessMode } from "../src/control-access.ts";

function collectTypeScriptSources(directory: URL, prefix = ""): string[] {
  const sources: string[] = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) sources.push(...collectTypeScriptSources(new URL(`${entry.name}/`, directory), relative));
    else if (entry.isFile() && entry.name.endsWith(".ts")) sources.push(relative);
  }
  return sources;
}

test("core stays independent of UI and native runtimes", () => {
  const core = new URL("../src/", import.meta.url);
  const coreSources = collectTypeScriptSources(core);
  // An empty listing would assert nothing and still report success.
  assert.ok(coreSources.length > 3, `expected the core sources, got ${coreSources.length}`);
  for (const name of coreSources) {
    const source = readFileSync(new URL(name, core), "utf8");
    assert.doesNotMatch(source, /(?:@tauri-apps|@capacitor|from ["']react["']|window\.|document\.|navigator\.)/, name);
  }
});
test("MCP control access uses the generated four-tier action policy", () => {
  assert.equal(parseControlAccessMode("modify"), "modify");
  assert.equal(parseControlAccessMode("administrator"), "full");
  assert.equal(controlAccessPermitsAction("read-only", "get_current_preset"), true);
  assert.equal(controlAccessPermitsAction("read-only", "set_tempo"), false);
  assert.equal(controlAccessPermitsAction("performance", "set_tempo"), true);
  assert.equal(controlAccessPermitsAction("modify", "set_parameter"), true);
});

test("surface command dispatch is shared by platform composition roots", () => {
  const calls: string[] = [];
  assert.equal(dispatchSurfaceCommand(surfaceCommand({ kind: "select-scene", scene: 3 }), {
    selectScene: (scene) => calls.push(`scene:${scene}`)
  }), true);
  assert.equal(dispatchSurfaceCommand(surfaceCommand({ kind: "switch", role: "bank:down", phase: "release" }), {
    movePreset: (delta) => calls.push(`preset:${delta}`)
  }), true);
  assert.equal(dispatchSurfaceCommand({ kind: "none" }, {}), false);
  assert.deepEqual(calls, ["scene:3", "preset:1"]);
});

test("live QC updates have one canonical snapshot reducer", () => {
  const dirty = { ...demoSnapshot, dirty: true };
  const preset = applyQcStateUpdate(dirty, { kind: "preset", presetName: "Clean", tempo: 96 });
  assert.equal(preset.presetName, "Clean");
  assert.equal(preset.tempo, 96);
  assert.equal(preset.dirty, true, "a full preset read does not observe whether edits are saved");
  const clean = applyQcStateUpdate(preset, {kind: "dirty", dirty: false});
  assert.equal(clean.dirty, false);
  const editedAgain = applyQcStateUpdates(clean, [
    {kind: "dirty", dirty: true},
    {kind: "preset", catalogRefresh: true, presetName: "Catalog name"},
    {kind: "preset", tempo: 143}
  ]);
  assert.equal(editedAgain.dirty, true);
  assert.equal(editedAgain.tempo, 143);

  const bypassed = applyQcStateUpdate(preset, {
    kind: "bypassBatch",
    bypassUpdates: [{ row: 0, column: 1, bypassed: true }, { row: 0, column: 2, bypassed: true }]
  });
  assert.equal(bypassed.blocks.find((block) => block.row === 0 && block.column === 1)?.bypassed, true);
  assert.equal(bypassed.blocks.find((block) => block.row === 0 && block.column === 2)?.bypassed, true);
});

test("sparse Grid topology stays incomplete through projections until a full preset arrives", () => {
  const complete = { ...demoSnapshot, gridLayoutComplete: true, stateVersion: { session: "grid-session", revision: 10 } };
  const incomplete = applyQcStateUpdate(complete, {
    kind: "gridTopology", gridLayoutComplete: false,
    stateVersion: { session: "grid-session", revision: 12 }
  });
  assert.equal(incomplete.gridLayoutComplete, false);
  const delayedComplete = { ...complete, stateVersion: { session: "grid-session", revision: 11 } };
  assert.equal(canAdoptNativeState(incomplete.stateVersion, delayedComplete.stateVersion), false,
    "a complete snapshot captured before a newer sparse invalidation must not replace it");
  assert.equal(canAdoptNativeState(incomplete.stateVersion,
    { session: "grid-session", revision: 13 }), true,
  "a newer full publication from the same session remains adoptable");
  assert.equal(canAdoptNativeState(incomplete.stateVersion,
    { session: "replacement-session", revision: 13 }), false,
  "a delayed snapshot from another session must not replace the current one");
  const projected = applyQcStateUpdates(incomplete, [
    { kind: "bypassBatch", bypassUpdates: [{ row: 0, column: 1, bypassed: true }], stateVersion: { session: "grid-session", revision: 13 } },
    { kind: "preset", presetName: "Internal sparse projection", gridLayoutComplete: false, stateVersion: { session: "grid-session", revision: 14 } }
  ]);
  assert.equal(projected.gridLayoutComplete, false);
  const refreshed = applyQcStateUpdate(projected, {
    kind: "preset", presetName: "Authoritative", blocks: demoSnapshot.blocks, gridLayoutComplete: true,
    stateVersion: { session: "grid-session", revision: 15 }
  });
  assert.equal(refreshed.gridLayoutComplete, true);
  assert.deepEqual(refreshed.blocks, demoSnapshot.blocks);
});

test("native block removal preserves dirty state, routing and surviving block controls", () => {
  const target = demoSnapshot.blocks[0];
  assert.ok(target);
  const before = { ...demoSnapshot, dirty: true, tempo: 147 };
  const after = applyQcStateUpdate(before, { kind: "blockRemoved", row: target.row, column: target.column });
  assert.equal(after.blocks.length, before.blocks.length - 1);
  assert.deepEqual(after.blocks, before.blocks.filter((block) => block !== target));
  assert.equal(after.dirty, true);
  assert.equal(after.tempo, 147);
  assert.deepEqual(after.routes, before.routes);
  assert.deepEqual(applyQcStateUpdate(after, { kind: "blockRemoved", row: target.row, column: target.column }), after);
});

test("partial native I/O updates merge into the shared snapshot", () => {
  const withInput = applyQcStateUpdate(demoSnapshot, {
    kind: "ioPorts",
    ioPorts: [{ kind: "input", id: 1, label: "In 1", plugged: true }]
  });
  const withOutput = applyQcStateUpdate(withInput, {
    kind: "ioPorts",
    ioPorts: [{ kind: "output", id: 4, label: "Out 1", plugged: false }]
  });
  assert.deepEqual(withOutput.ioPorts, [
    { kind: "input", id: 1, label: "In 1", plugged: true },
    { kind: "output", id: 4, label: "Out 1", plugged: false }
  ]);
});

test("native MIDI settings replace the matching preset projections", () => {
  const before = {
    ...demoSnapshot,
    midiOut: [
      { source: 1, messages: [{ type: 1, channel: 1, param1: 2, param2: 3, param3: 4 }] },
      { source: 4, messages: [{ type: 2, channel: 4, param1: 11, param2: 12, param3: 13 }] }
    ],
    presetLoadMidiOut: [{ type: 2, channel: 2, param1: 5, param2: 6, param3: 7 }]
  };
  const unchanged = structuredClone(before);
  const added = { source: 7, messages: [{ type: 1, channel: 5, param1: 14, param2: 15, param3: 16 }] };
  const after = applyQcStateUpdate(before, {
    kind: "midiOut",
    midiOut: [{ source: 1, messages: [] }, added],
    presetLoadMidiOut: [{ type: 3, channel: 3, param1: 8, param2: 9, param3: 10 }]
  });
  assert.deepEqual(after.midiOut, [before.midiOut[1], added]);
  assert.equal(after.midiOut?.some((row) => row.source === 1), false,
    "an explicitly empty source withdraws only its own messages");
  assert.deepEqual(after.presetLoadMidiOut, [{ type: 3, channel: 3, param1: 8, param2: 9, param3: 10 }]);
  assert.deepEqual(before, unchanged, "a sparse publication never mutates the prior snapshot");
  const empty = applyQcStateUpdate(after, { kind: "midiOut", midiOut: [] });
  assert.deepEqual(empty.midiOut, after.midiOut,
    "an empty collection contains no evidence of an addressed source change");
  assert.deepEqual(empty.presetLoadMidiOut, after.presetLoadMidiOut);
  const clearedLoad = applyQcStateUpdate(empty, { kind: "midiOut", presetLoadMidiOut: [] });
  assert.deepEqual(clearedLoad.midiOut, after.midiOut);
  assert.deepEqual(clearedLoad.presetLoadMidiOut, []);
});

test("new optimistic bypass intent cannot be rolled back by an older device echo", () => {
  const pending = new Map();
  markPendingBypass(pending, 0, 1, true, 1_000);
  markPendingBypass(pending, 0, 1, false, 1_050);

  const stale = reconcilePendingBypass({ kind: "bypassBatch", bypassUpdates: [{ row: 0, column: 1, bypassed: true }] }, pending, 1_100);
  assert.equal(stale, null);
  assert.equal(pending.get("0:1")?.bypassed, false);

  const acknowledgement = reconcilePendingBypass({ kind: "bypassBatch", bypassUpdates: [{ row: 0, column: 1, bypassed: false }] }, pending, 1_150);
  assert.deepEqual(acknowledgement, { kind: "bypassBatch", bypassUpdates: [{ row: 0, column: 1, bypassed: false }] });
  assert.equal(pending.size, 0);

  markPendingBypass(pending, 0, 1, true, 2_000);
  const enriched = reconcilePendingBypass({
    kind: "preset",
    catalogRefresh: true,
    blocks: demoSnapshot.blocks.map((block) => block.row === 0 && block.column === 1 ? { ...block, bypassed: false } : block)
  }, pending, 2_100);
  assert.equal(enriched?.blocks?.find((block) => block.row === 0 && block.column === 1)?.bypassed, true);
  assert.equal(pending.get("0:1")?.bypassed, true);
});

test("a full snapshot that started before a rapid second tap cannot acknowledge its ABA state", () => {
  const pending: PendingBypassIntents = new Map();
  markPendingBypass(pending, 0, 0, false, 200);
  const stale = reconcilePendingBypass(
    { kind: "preset", blocks: [{ id: "drive", name: "Drive", kind: "utility", row: 0, column: 0, bypassed: false }] },
    pending,
    500,
    3_000,
    100
  );
  assert.equal(stale?.blocks?.[0].bypassed, false);
  assert.equal(pending.get("0:0")?.bypassed, false, "the pre-tap read must not clear the final intent");

  const confirmed = reconcilePendingBypass(
    { kind: "preset", blocks: stale?.blocks },
    pending,
    700,
    3_000,
    600
  );
  assert.equal(confirmed?.blocks?.[0].bypassed, false);
  assert.equal(pending.size, 0);
});

test("native frame batching and optimistic bypass bookkeeping are shared", () => {
  const pending: PendingBypassIntents = new Map();
  const before = { ...demoSnapshot, blocks: [{ ...demoSnapshot.blocks[0], row: 0, column: 0, bypassed: false }] };
  const after = { ...before, blocks: [{ ...before.blocks[0], bypassed: true }] };
  const changed = recordPendingBypassChanges(before, after, pending, 1_000);
  assert.deepEqual(changed, ["0:0"]);

  const states = reconcileQcStateUpdates([
    { kind: "bypass", row: 0, column: 0, bypassed: false, observedAt: 900 },
    { kind: "tempo", tempo: 132, observedAt: 1_010 }
  ], pending, 1_020);
  const applied = applyQcStateUpdates(after, states);
  assert.equal(applied.blocks[0].bypassed, true, "a pre-command device echo must be suppressed");
  assert.equal(applied.tempo, 132);
  assert.equal(pending.size, 1);

  clearPendingBypassChanges(pending, changed, 999);
  assert.equal(pending.size, 1, "an older failure must not clear a newer intent");
  clearPendingBypassChanges(pending, changed, 1_000);
  assert.equal(pending.size, 0);
});

test("native frames and slow complete reads use shared stale-echo reduction", () => {
  const pending: PendingBypassIntents = new Map();
  markPendingBypass(pending, 0, 0, true, 1_000);
  const before = { ...demoSnapshot, blocks: [{ ...demoSnapshot.blocks[0], row: 0, column: 0, bypassed: true }] };
  const reduced = reduceQcStateFrame(before, [
    { kind: "bypass", row: 0, column: 0, bypassed: false, observedAt: 900 },
    { kind: "tempo", tempo: 144, observedAt: 1_010 }
  ], pending, 1_020);
  assert.equal(reduced.snapshot.blocks[0].bypassed, true);
  assert.equal(reduced.snapshot.tempo, 144);

  const staleRead = { ...before, blocks: [{ ...before.blocks[0], bypassed: false }] };
  assert.equal(reconcilePresetSnapshot(staleRead, pending, 950, 1_030).blocks[0].bypassed, true);
});

test("command coordinator suppresses stale scalar echoes and accepts acknowledgements", () => {
  const coordinator = new QcCommandCoordinator();
  const command = coordinator.beginScene(demoSnapshot, 3, 1_000);
  assert.equal(command.snapshot.activeScene, 3);

  const stale = coordinator.reconcileFrame(command.snapshot, [
    { kind: "scene", activeScene: 0, observedAt: 900 }
  ], 1_100);
  assert.equal(stale.snapshot.activeScene, 3);
  assert.equal(stale.states.length, 0);

  const acknowledged = coordinator.reconcileFrame(stale.snapshot, [
    { kind: "scene", activeScene: 3, observedAt: 1_050 }
  ], 1_150);
  assert.equal(acknowledged.snapshot.activeScene, 3);
  assert.equal(acknowledged.states.length, 1);
  assert.equal(coordinator.hasPendingCommands, false);
});

test("an older command failure cannot roll back a newer optimistic value", () => {
  const coordinator = new QcCommandCoordinator();
  const first = coordinator.beginTempo(demoSnapshot, 130, 1_000);
  const second = coordinator.beginTempo(first.snapshot, 140, 1_010);
  assert.equal(coordinator.fail(second.snapshot, first.token).tempo, 140);
  assert.equal(coordinator.hasPendingCommands, true);
  assert.equal(coordinator.fail(second.snapshot, second.token).tempo, demoSnapshot.tempo,
    "the latest failure returns to the authoritative base when the predecessor is still unconfirmed");
});

test("scene chains roll back to the authoritative predecessor across overlapping outcomes", () => {
  const base = { ...demoSnapshot, activeScene: 0 };

  const bothFail = new QcCommandCoordinator();
  const firstRejected = bothFail.beginScene(base, 1, 1_000);
  const secondRejected = bothFail.beginScene(firstRejected.snapshot, 2, 1_010);
  assert.equal(bothFail.fail(secondRejected.snapshot, firstRejected.token).activeScene, 2, "the older failure leaves the newest preview intact");
  const reverted = bothFail.fail(secondRejected.snapshot, secondRejected.token);
  assert.equal(reverted.activeScene, 0, "two rejected intents restore the original authoritative scene");
  assert.equal(bothFail.hasPendingCommands, false);

  const firstSucceeds = new QcCommandCoordinator();
  const firstAccepted = firstSucceeds.beginScene(base, 1, 2_000);
  const secondRejectedAfterSuccess = firstSucceeds.beginScene(firstAccepted.snapshot, 2, 2_010);
  firstSucceeds.acknowledge(firstAccepted.token);
  assert.equal(firstSucceeds.fail(secondRejectedAfterSuccess.snapshot, secondRejectedAfterSuccess.token).activeScene, 1,
    "the newer failure restores the predecessor the earlier command successfully selected");

  const newestSucceeds = new QcCommandCoordinator();
  const firstLateFailure = newestSucceeds.beginScene(base, 1, 3_000);
  const secondAccepted = newestSucceeds.beginScene(firstLateFailure.snapshot, 2, 3_010);
  assert.equal(newestSucceeds.fail(secondAccepted.snapshot, firstLateFailure.token).activeScene, 2,
    "a late older failure cannot roll back the successful newest intent");
  newestSucceeds.acknowledge(secondAccepted.token);
  assert.equal(secondAccepted.snapshot.activeScene, 2);
});

test("preset observations discard optimistic values belonging to a different setlist or slot", () => {
  for (const target of [
    {setlistKey: "/user/B", presetPosition: 0},
    {setlistKey: "/user/A", presetPosition: 1}
  ]) {
    for (const fullSnapshot of [false, true]) {
      const coordinator = new QcCommandCoordinator();
      const before = {...demoSnapshot, setlistKey: "/user/A", presetPosition: 0,
        blocks: [{...demoSnapshot.blocks.find((block) => block.id === "amp")!, bypassed: false}]};
      const bypass = coordinator.beginBypass(before, "amp", true, 1_000);
      const optimistic = coordinator.beginTempo(bypass.snapshot, 150, 1_010);
      const incoming = {...before, ...target, presetName: "Different", tempo: 110};
      const next = fullSnapshot
        ? coordinator.reconcileSnapshot(incoming, 1_100, 1_100)
        : coordinator.reconcileFrame(optimistic.snapshot, [
          {kind: "position", position: target.presetPosition, setlistKey: target.setlistKey},
          {kind: "preset", presetName: incoming.presetName, blocks: incoming.blocks, tempo: incoming.tempo}
        ], 1_100).snapshot;
      assert.equal(next.blocks[0].bypassed, false);
      assert.equal(next.tempo, 110);
      assert.equal(next.setlistKey, target.setlistKey);
      assert.equal(next.presetPosition, target.presetPosition);
      assert.equal(coordinator.hasPendingCommands, false);
      assert.deepEqual(coordinator.fail(next, bypass.token), next, "an old failure cannot modify the new preset");
    }
  }
});

test("command coordinator handles rapid bypass ABA and safe rollback", () => {
  const coordinator = new QcCommandCoordinator();
  const base = {
    ...demoSnapshot,
    blocks: demoSnapshot.blocks.map((block) => block.id === "amp" ? { ...block, bypassed: false } : block)
  };
  const first = coordinator.beginBypass(base, "amp", true, 1_000);
  const second = coordinator.beginBypass(first.snapshot, "amp", false, 1_010);
  const stale = coordinator.reconcileFrame(second.snapshot, [
    { kind: "bypass", row: 0, column: 1, bypassed: true, observedAt: 1_005 }
  ], 1_020);
  assert.equal(stale.snapshot.blocks.find((block) => block.id === "amp")?.bypassed, false);
  assert.equal(coordinator.fail(stale.snapshot, first.token).blocks.find((block) => block.id === "amp")?.bypassed, false);
  assert.equal(coordinator.fail(stale.snapshot, second.token).blocks.find((block) => block.id === "amp")?.bypassed, true);
});

test("optimistic commands expose a bounded authoritative reconciliation deadline", () => {
  const coordinator = new QcCommandCoordinator();
  assert.equal(coordinator.pendingReconciliationDelay(1_000), undefined);

  coordinator.beginBypass(demoSnapshot, "amp", true, 1_000);
  assert.equal(coordinator.pendingReconciliationDelay(1_000), 3_000);
  assert.equal(coordinator.pendingReconciliationDelay(3_999), 1);
  assert.equal(coordinator.pendingReconciliationDelay(4_000), 0);
});

test("authoritative refresh adopts a physical override after the stale-echo window", () => {
  const coordinator = new QcCommandCoordinator();
  const base = {
    ...demoSnapshot,
    blocks: demoSnapshot.blocks.map((block) => block.id === "amp" ? { ...block, bypassed: false } : block)
  };
  const command = coordinator.beginBypass(base, "amp", true, 1_000);
  const suppressed = coordinator.reconcileFrame(command.snapshot, [
    { kind: "bypass", row: 0, column: 1, bypassed: false, observedAt: 1_100 }
  ], 1_100);
  assert.equal(suppressed.snapshot.blocks.find((block) => block.id === "amp")?.bypassed, true);
  assert.equal(coordinator.hasPendingCommands, true);

  const refreshed = coordinator.reconcileSnapshot(base, 4_001, 4_001);
  assert.equal(refreshed.blocks.find((block) => block.id === "amp")?.bypassed, false);
  assert.equal(coordinator.hasPendingCommands, false);
});

test("slow complete reads preserve all newer scalar and bypass commands", () => {
  const coordinator = new QcCommandCoordinator();
  const scene = coordinator.beginScene(demoSnapshot, 2, 1_000);
  const bypass = coordinator.beginBypass(scene.snapshot, "amp", true, 1_010);
  const stale = coordinator.reconcileSnapshot(demoSnapshot, 900, 1_020);
  assert.equal(stale.activeScene, 2);
  assert.equal(stale.blocks.find((block) => block.id === "amp")?.bypassed, true);
});

test("mode and tempo rollback restore their complete UI state", () => {
  const coordinator = new QcCommandCoordinator();
  const base = {
    ...demoSnapshot,
    mode: "HYBRID" as const,
    footswitchModes: ["STOMP", "SCENE"] as ["STOMP", "SCENE"],
    tempoLedEnabled: false
  };
  const mode = coordinator.beginModeSlot(base, 2, 1_000);
  assert.deepEqual(mode.snapshot.footswitchModes, ["STOMP", "STOMP"]);
  const modeFailed = coordinator.fail(mode.snapshot, mode.token);
  assert.equal(modeFailed.mode, "HYBRID");
  assert.deepEqual(modeFailed.footswitchModes, ["STOMP", "SCENE"]);

  const tempo = coordinator.beginTempo(modeFailed, 150, 1_100);
  const tempoFailed = coordinator.fail(tempo.snapshot, tempo.token);
  assert.equal(tempoFailed.tempo, base.tempo);
  assert.equal(tempoFailed.tempoLedEnabled, false);
});

test("local preview mutations share the same bounded snapshot rules", () => {
  assert.equal(movePresetInSnapshot(demoSnapshot, -1).presetPosition, 0);
  assert.equal(movePresetInSnapshot({ ...demoSnapshot, presetPosition: 7 }, 1).presetLocation, "2A");
  assert.equal(setTempoInSnapshot(demoSnapshot, 999).tempo, 240);
  assert.equal(setBlockBypassInSnapshot(demoSnapshot, "amp", true).blocks.find((block) => block.id === "amp")?.bypassed, true);
  assert.equal(selectModeSlotInSnapshot(demoSnapshot, 2).mode, "STOMP");
  const hybrid = selectModeSlotInSnapshot({
    ...demoSnapshot,
    modeSlots: [{ slot: 0, label: "STOMP / SCENE", mode: "HYBRID" }]
  }, 0);
  assert.equal(hybrid.mode, "HYBRID");
  assert.deepEqual(hybrid.footswitchModes, ["STOMP", "SCENE"]);
  const missing = { ...demoSnapshot, modeSlots: undefined };
  assert.equal(selectModeSlotInSnapshot(missing, 2), missing);
});

test("tap tempo calculation is shared and bounded", () => {
  assert.deepEqual(recordTempoTap([], 1000), { taps: [1000], status: "need-more" });
  assert.equal(recordTempoTap([1000], 1500).bpm, 120);
  assert.deepEqual(recordTempoTap([1000], 1100), { taps: [1100], status: "invalid" });
  assert.deepEqual(recordTempoTap([1000], 4000), { taps: [4000], status: "need-more" });
});

test("surface actions resolve to platform-neutral commands", () => {
  assert.deepEqual(surfaceCommand({ kind: "switch", role: "footswitch:C", phase: "release" }), { kind: "press-footswitch", index: 2 });
  assert.deepEqual(surfaceCommand({ kind: "switch", role: "tempo", phase: "release" }), { kind: "tap-tempo" });
  assert.deepEqual(surfaceCommand({ kind: "switch", role: "tempo", phase: "press" }), { kind: "none" });
  assert.deepEqual(surfaceCommand({ kind: "select-block", blockId: "amp" }), { kind: "toggle-block-editor", blockId: "amp" });
});

test("routing taxonomy and row constraints are shared", () => {
  assert.equal(routePickerLabel("output", "Multi Out"), "Multiple Outputs");
  assert.equal(routePickerGroup("input", 3), "STEREO");
  assert.equal(routeOptionValue("input", undefined, "In1"), 1);
  assert.equal(inputRouteOptions.some(([, label]) => label === "Sidechain"), false);
  const routes = demoSnapshot.routes.map((route) => route.row === 0 ? { ...route, outputId: 16 } : route);
  assert.equal(routeOptionsForRow("input", 2, 0, routes).some(([value]) => value === 7), true);
  assert.equal(routeOptionsForRow("input", 1, 0, routes).some(([value]) => value === 7), false);
});

test("parameter editor helpers share open-close and draft rules", () => {
  const details: BlockDetails = {
    row: 0,
    column: 1,
    modelId: 1,
    name: "Amp",
    category: "Amp",
    scene: 0,
    parameters: [
      { index: 0, name: "Gain", normalizedValue: 0.5, displayValue: "5.0", units: "", type: "float", minimum: 0, maximum: 10, steps: null, sceneMode: false, options: [], writable: true },
      { index: 1, name: "Meter", normalizedValue: null, displayValue: "", units: "", type: "meter", minimum: 0, maximum: 1, steps: null, sceneMode: false, options: [], writable: false }
    ]
  };
  assert.deepEqual(parameterDrafts(details), { 0: 0.5 });
  assert.equal(updateBlockParameter(details, 0, 0.75).parameters[0].normalizedValue, 0.75);
  assert.equal(blockSelectionIntent("amp", "amp"), "close");
  assert.equal(blockSelectionIntent("amp", "cab"), "open");

  const opened = reduceBlockEditorSession(emptyBlockEditorSession, { type: "details", details, resetPage: true });
  assert.deepEqual(opened.drafts, { 0: 0.5 });
  const edited = reduceBlockEditorSession(opened, { type: "parameters", values: { 0: 0.8 } });
  assert.equal(edited.details?.parameters[0].normalizedValue, 0.8);
  assert.equal(edited.drafts[0], 0.8);
  assert.equal(reduceBlockEditorSession({ ...edited, page: 3 }, { type: "scene", scene: 4 }).details?.scene, 4);
  assert.deepEqual(reduceBlockEditorSession(edited, { type: "close" }), { details: undefined, drafts: {}, page: 0 });
});

test("footswitch LEDs and optimistic STOMP behavior belong to the core", () => {
  const stomp = {
    ...demoSnapshot,
    mode: "STOMP" as const,
    footswitchModes: ["STOMP", "STOMP"] as ["STOMP", "STOMP"],
    blocks: demoSnapshot.blocks.map((block) => block.id === "amp" ? { ...block, footswitch: 0, bypassed: false } : block)
  };
  assert.deepEqual(footswitchLeds(stomp)[0], { active: true, assigned: true, color: "#ff2727" });
  assert.deepEqual(footswitchIntent(stomp, 0), { kind: "toggle-stomp", index: 0 });
  assert.equal(optimisticallyPressFootswitch(stomp, 0).blocks.find((block) => block.id === "amp")?.bypassed, true);
  assert.equal(applyFootswitchPreview(stomp, 0).blocks.find((block) => block.id === "amp")?.bypassed, true);

  const scene = { ...demoSnapshot, mode: "SCENE" as const, footswitchModes: ["SCENE", "SCENE"] as ["SCENE", "SCENE"] };
  assert.equal(applyFootswitchPreview(scene, 3).activeScene, 3);
  const preset = { ...demoSnapshot, mode: "PRESET" as const, presetPosition: 10, footswitchModes: ["PRESET", "PRESET"] as ["PRESET", "PRESET"] };
  assert.deepEqual(footswitchIntent(preset, 5), { kind: "select-preset", position: 13 });
  assert.equal(applyFootswitchPreview(preset, 5).presetLocation, "2F");
});

test("qc-client ships no device-derived names or colours, so it stays plain MIT OR Apache-2.0", () => {
  const clientRoot = new URL("../../qc-client/", import.meta.url);
  assert.equal(JSON.parse(readFileSync(new URL("package.json", clientRoot), "utf8")).license, "MIT OR Apache-2.0");
  const sources = readdirSync(new URL("src/", clientRoot)).filter((name) => name.endsWith(".ts"))
    .map((name) => [name, readFileSync(new URL(`src/${name}`, clientRoot), "utf8")] as const);
  assert.ok(sources.length > 0);
  // The demo preset's model and plugin names and the device colour tables are
  // device-derived; they live in qc-core's carved-out device-reference/.
  const deviceDerived = [/\bdemoSnapshot\b/, /\bQC_(?:SCENE|PRESET_SLOT|CATEGORY|HARDWARE)_COLOR/, /Plini|Gojira|Nolly|Cory Wong|Rabea|Brit(?:ish)? 2203/, /#[0-9a-f]{6}\b/i];
  for (const [name, source] of sources) {
    for (const pattern of deviceDerived) assert.doesNotMatch(source, pattern, `qc-client/src/${name} carries device-derived material (${pattern})`);
  }
  assert.equal(demoSnapshot.sceneColors?.length, 8);
});

test("an unknown footswitch mode stays unknown: it keeps its number, names no row modes and presses nothing", () => {
  const stomp = {
    ...demoSnapshot,
    mode: "STOMP" as const,
    footswitchModes: ["STOMP", "STOMP"] as ["STOMP", "STOMP"],
    blocks: demoSnapshot.blocks.map((block) => block.id === "amp" ? { ...block, footswitch: 0, bypassed: false } : block)
  };
  const unknown = applyQcStateUpdate(stomp, { kind: "mode", mode: "UNKNOWN", rawMode: 11 });
  assert.equal(unknown.mode, "UNKNOWN");
  assert.equal(unknown.rawMode, 11);
  // The previous mode's per-row pair does not outlive the mode it was reported with.
  assert.equal(unknown.footswitchModes, undefined);
  assert.deepEqual(footswitchIntent(unknown, 0), { kind: "none" }, "an unknown mode is not STOMP");
  assert.deepEqual(footswitchLeds(unknown)[0], { active: false, assigned: false, color: footswitchLeds(unknown)[7].color });
  assert.equal(optimisticallyPressFootswitch(unknown, 0), unknown);
  // A reported base mode still carries its pair.
  const back = applyQcStateUpdate(unknown, { kind: "mode", mode: "STOMP", rawMode: 2, footswitchModes: ["STOMP", "STOMP"] });
  assert.equal(back.rawMode, 2);
  assert.deepEqual(footswitchIntent(back, 0), { kind: "toggle-stomp", index: 0 });
});

test("an optimistic tempo write changes only the tempo, and an optimistic hybrid mode names no pair", () => {
  const off = { ...demoSnapshot, tempoLedEnabled: false };
  assert.equal(setTempoInSnapshot(off, 150).tempoLedEnabled, false);
  const coordinator = new QcCommandCoordinator();
  const hybrid = coordinator.beginModeSlot({ ...demoSnapshot, modeSlots: [{ slot: 0, label: "STOMP / SCENE", mode: "HYBRID" }] }, 0, 1_000);
  // The pair comes from the slot's own label, not from a guess at what the kind implies.
  assert.deepEqual(hybrid.snapshot.footswitchModes, ["STOMP", "SCENE"]);
});

test("placeholders leave the unobserved list when a device message sets them, and a scene label push shows the default text for a blank one", () => {
  const fresh = {
    ...demoSnapshot,
    unobserved: ["mode", "tempo", "tempoLedEnabled", "masterVolume", "dirty", "presetLocation"] as NonNullable<typeof demoSnapshot.unobserved>,
    sceneLabelsRaw: ["", "", "", "", "", "", "", ""]
  };
  const afterTempo = applyQcStateUpdate(fresh, { kind: "tempo", tempo: 98 });
  assert.deepEqual(afterTempo.unobserved, ["mode", "tempoLedEnabled", "masterVolume", "dirty", "presetLocation"]);
  const afterAll = applyQcStateUpdates(afterTempo, [
    { kind: "mode", mode: "SCENE", rawMode: 1, footswitchModes: ["SCENE", "SCENE"] },
    { kind: "tempo", tempoLedEnabled: true },
    { kind: "master", masterVolume: 0.5 },
    { kind: "dirty", dirty: false },
    { kind: "position", position: 3 }
  ]);
  assert.equal(afterAll.unobserved, undefined, "the list is dropped once every placeholder has been observed");
  // An update that carries no value observes nothing.
  assert.deepEqual(applyQcStateUpdate(fresh, { kind: "tempo" }).unobserved, fresh.unobserved);

  const labelled = applyQcStateUpdate(fresh, { kind: "sceneLabel", index: 2, label: "Lead" });
  assert.equal(labelled.scenes[2], "Lead");
  assert.equal(labelled.sceneLabelsRaw?.[2], "Lead");
  const cleared = applyQcStateUpdate(labelled, { kind: "sceneLabel", index: 2, label: "  " });
  assert.equal(cleared.scenes[2], "Scene C");
  assert.equal(cleared.sceneLabelsRaw?.[2], "  ");
});

test("a model preset catalog update is kept beside the preset and fenced by session and generation", () => {
  const counters = {
    messages: 1, creates: 1, updates: 0, renames: 0, renameMisses: 0, deletes: 0, deleteMisses: 0, wipes: 0, readsIgnored: 0,
    otherActionsIgnored: 0, markerUpdates: 0, ambiguousMarkers: 0, loadedIgnored: 0, malformedIdentities: 0, decodeFailures: 0, inboundLoss: 0,
    duplicateTuples: 0, ambiguousTuples: 0, retainedEntries: 1, retainedOtherHashEntries: 0
  };
  const projection = (session: string, generation: number) => ({
    state: "folding" as const, session, generation, scopeHash: 4004,
    entries: [{ id: "a", name: "A", isFactory: false, isDefault: false, modelHash: 4004 }], counters
  });
  const base = { ...demoSnapshot, gridLayoutComplete: true, stateVersion: { session: "s1", revision: 1 } };
  const first = applyQcStateUpdate(base, {
    kind: "modelPresetCatalog", modelPresetCatalog: projection("s1", 2), stateVersion: { session: "s1", revision: 2 }
  });
  assert.equal(first.modelPresetCatalog?.generation, 2);
  assert.equal(first.blocks, base.blocks, "a list says nothing about the preset");
  assert.equal(first.gridLayoutComplete, true);
  const lower = applyQcStateUpdate(first, {
    kind: "modelPresetCatalog", modelPresetCatalog: projection("s1", 1), stateVersion: { session: "s1", revision: 3 }
  });
  assert.equal(lower.modelPresetCatalog?.generation, 2, "an older projection never replaces a newer one");
  const foreign = applyQcStateUpdate(first, {
    kind: "modelPresetCatalog", modelPresetCatalog: projection("s2", 9), stateVersion: { session: "s2", revision: 1 }
  });
  assert.equal(foreign, first, "an update from another session is dropped before it reaches the catalog");
  const withoutBody = applyQcStateUpdate(first, { kind: "modelPresetCatalog", stateVersion: { session: "s1", revision: 4 } });
  assert.equal(withoutBody.modelPresetCatalog, first.modelPresetCatalog);
});

test("a model preset catalog event survives the controller's frame reconciliation and stays session-fenced", () => {
  const counters = {
    messages: 1, creates: 1, updates: 0, renames: 0, renameMisses: 0, deletes: 0, deleteMisses: 0, wipes: 0, readsIgnored: 0,
    otherActionsIgnored: 0, markerUpdates: 0, ambiguousMarkers: 0, loadedIgnored: 0, malformedIdentities: 0, decodeFailures: 0,
    inboundLoss: 0, duplicateTuples: 0, ambiguousTuples: 0, retainedEntries: 0, retainedOtherHashEntries: 0
  };
  const coordinator = new QcCommandCoordinator();
  const base = { ...demoSnapshot, stateVersion: { session: "s1", revision: 1 } };
  const event = (session: string, generation: number) => ({
    kind: "modelPresetCatalog" as const, stateVersion: { session, revision: generation + 1 },
    modelPresetCatalog: { state: "folding" as const, session, generation, scopeHash: 4004, entries: [], counters }
  });
  const reduced = coordinator.reconcileFrame(base, [event("s1", 1)]);
  assert.equal(reduced.snapshot.modelPresetCatalog?.generation, 1, "the event reaches the snapshot through the coordinator");
  const foreign = coordinator.reconcileFrame(reduced.snapshot, [event("s2", 7)]);
  assert.equal(foreign.snapshot.modelPresetCatalog?.session, "s1", "another session's projection never replaces it");
});

// ---- Global EQ preset catalog (the host's session fold as a client sees it) ----
// Wrapped in a block so these helpers cannot collide with names elsewhere in this file.
{
  const SESSION = "qc-state-v1.session-a";
  const SCOPE = 4004;
  const counters: ModelPresetCatalogState["counters"] = {
    messages: 1, creates: 0, updates: 1, renames: 0, renameMisses: 0, deletes: 0, deleteMisses: 0, wipes: 0, readsIgnored: 0,
    otherActionsIgnored: 0, markerUpdates: 0, ambiguousMarkers: 0, loadedIgnored: 0, malformedIdentities: 0, decodeFailures: 0, inboundLoss: 0,
    duplicateTuples: 0, ambiguousTuples: 0, retainedEntries: 0, retainedOtherHashEntries: 0
  };
  const preset = (id: string, isFactory = false, modelHash: number | null = SCOPE): ModelPresetSummary =>
    ({ id, name: id.toUpperCase(), isFactory, isDefault: false, ...(modelHash === null ? {} : { modelHash }) });
  const catalog = (patch: Partial<ModelPresetCatalogState> = {}): ModelPresetCatalogState =>
    ({ state: "folding", session: SESSION, generation: 1, scopeHash: SCOPE, entries: [preset("a", true), preset("b")], counters, ...patch });
  const carrier = (modelPresetCatalog: ModelPresetCatalogState | undefined, session: string | null = SESSION) =>
    ({ modelPresetCatalog, stateVersion: session === null ? undefined : { session, revision: 5 } });

  test("the picker reads unavailable, not none, until the host has projected a list", () => {
    assert.deepEqual(globalEqPresetsFrom(undefined), { status: "unavailable", reason: "not-observed" });
    assert.deepEqual(globalEqPresetsFrom(carrier(undefined)), { status: "unavailable", reason: "not-observed" });
    assert.deepEqual(globalEqPresetsFrom(carrier(catalog({ state: "unavailable", entries: undefined }))), { status: "unavailable", reason: "not-observed" });
    assert.deepEqual(globalEqPresetsFrom(carrier(catalog({ state: "invalidated", entries: undefined, invalidReason: "inbound-loss" }))), { status: "unavailable", reason: "invalidated", invalidReason: "inbound-loss" });
  });

  test("an observed empty list is observed, and says nothing about absence", () => {
    assert.deepEqual(globalEqPresetsFrom(carrier(catalog({ entries: [] }))), { status: "observed", presets: [] });
  });

  test("a projection from another session than the snapshot is unavailable", () => {
    assert.deepEqual(globalEqPresetsFrom(carrier(catalog(), "qc-state-v1.session-b")), { status: "unavailable", reason: "session-mismatch" });
    assert.deepEqual(globalEqPresetsFrom(carrier(catalog(), null)), { status: "unavailable", reason: "session-mismatch" });
    assert.deepEqual(globalEqPresetsFrom(carrier(catalog({ session: "" }))), { status: "unavailable", reason: "session-mismatch" });
  });

  test("only scope-hash entries with a complete, unique identity are offered", () => {
    const entries = [
      preset("a", true), preset("other-family", false, 77), preset("no-hash", false, null), preset("  ", false),
      preset("twin"), preset("twin"), preset("twin", true), preset("b")
    ];
    const result = globalEqPresetsFrom(carrier(catalog({ entries })));
    assert.equal(result.status, "observed");
    assert.deepEqual((result as Extract<GlobalEqPresets, { status: "observed" }>).presets.map((entry) => `${entry.id}:${entry.isFactory}`), ["a:true", "twin:true", "b:false"]);
  });

  test("admission accepts exactly an offered tuple and refuses a stale, foreign or unobserved one", () => {
    const snapshot = carrier(catalog());
    assert.equal(admitGlobalEqPreset(snapshot, preset("a", true)), undefined);
    assert.equal(admitGlobalEqPreset(snapshot, preset("b")), undefined);
    // The same value with the other factory flag, another family, no hash: all different identities.
    assert.match(admitGlobalEqPreset(snapshot, preset("a", false)) ?? "", /not in the list/);
    assert.match(admitGlobalEqPreset(snapshot, preset("b", false, 77)) ?? "", /not in the list/);
    assert.match(admitGlobalEqPreset(snapshot, preset("b", false, null)) ?? "", /not in the list/);
    // After the host deleted it, the next projection no longer lists it.
    const afterDelete = carrier(catalog({ generation: 2, entries: [preset("b")] }));
    assert.match(admitGlobalEqPreset(afterDelete, preset("a", true)) ?? "", /not in the list/);
    // After a session change nothing carries over.
    assert.match(admitGlobalEqPreset(carrier(catalog(), "qc-state-v1.session-b"), preset("a", true)) ?? "", /has not been observed/);
    assert.match(admitGlobalEqPreset(carrier(undefined), preset("a", true)) ?? "", /has not been observed/);
    assert.match(admitGlobalEqPreset(carrier(catalog({ state: "invalidated", entries: undefined })), preset("a", true)) ?? "", /may be incomplete/);
  });

  test("the active marker is never authority to load anything", () => {
    const snapshot = carrier(catalog({ entries: [], activeMarker: { id: "z", isFactory: false, modelHash: SCOPE } }));
    assert.notEqual(admitGlobalEqPreset(snapshot, preset("z")), undefined);
  });

  test("the reducer lets the latest generation win and never lowers one", () => {
    const first = catalog({ generation: 3 });
    assert.equal(adoptModelPresetCatalog(undefined, first), first);
    assert.equal(adoptModelPresetCatalog(first, undefined), first);
    const newer = catalog({ generation: 4, entries: [] });
    assert.equal(adoptModelPresetCatalog(first, newer), newer);
    assert.equal(adoptModelPresetCatalog(first, catalog({ generation: 4 }))?.generation, 4);
    const older = catalog({ generation: 2, entries: [] });
    assert.equal(adoptModelPresetCatalog(first, older), first);
    assert.equal(adoptModelPresetCatalog(newer, first), newer);
  });

  test("an invalidated fold is sticky within its session and a foreign projection is ignored", () => {
    const dead = catalog({ state: "invalidated", generation: 9, entries: undefined, invalidReason: "decode-failure" });
    assert.equal(adoptModelPresetCatalog(dead, catalog({ generation: 10 })), dead);
    const foreign = catalog({ session: "qc-state-v1.session-b", generation: 99 });
    assert.equal(adoptModelPresetCatalog(catalog(), foreign)?.session, SESSION);
    // An invalidation arriving after a good fold does replace it.
    const invalidated = catalog({ state: "invalidated", generation: 2, entries: undefined });
    assert.equal(adoptModelPresetCatalog(catalog({ generation: 1 }), invalidated), invalidated);
  });

  test("a limit overflow is described as a size limit, never as something a reconnect would fix", () => {
    for (const invalidReason of ["retention-limit", "publication-limit"] as const) {
      const unavailable = globalEqPresetsFrom(carrier(catalog({ state: "invalidated", invalidReason, entries: undefined })));
      assert.deepEqual(unavailable, { status: "unavailable", reason: "invalidated", invalidReason });
      const notice = globalEqUnavailableNotice(unavailableOf(unavailable));
      assert.match(notice, /more presets than the supported maximum/);
      assert.match(notice, /exactly the maximum is accepted/, "the limits are inclusive maxima");
      assert.doesNotMatch(notice, /reconnect/i);
      assert.equal(admitGlobalEqPreset(carrier(catalog({ state: "invalidated", invalidReason, entries: undefined })), preset("a", true)), notice);
    }
    const lost = globalEqPresetsFrom(carrier(catalog({ state: "invalidated", invalidReason: "inbound-loss", entries: undefined })));
    assert.match(globalEqUnavailableNotice(unavailableOf(lost)), /may be incomplete/);
  });

  test("an observed list is never presented as empty because the host dropped entries", () => {
    // The host invalidates instead of truncating, so an invalidated projection carries no entries
    // and reads as unavailable, not as an observed empty list.
    const overflow = catalog({ state: "invalidated", invalidReason: "publication-limit", entries: undefined });
    assert.equal(globalEqPresetsFrom(carrier(overflow)).status, "unavailable");
    assert.equal(globalEqPresetsFrom(carrier(catalog({ state: "unavailable", entries: [] }))).status, "unavailable");
  });

  test("a stale catalog from an earlier session is never offered or admitted", () => {
    const old = carrier(catalog({ session: "qc-state-v1.session-a" }), "qc-state-v1.session-a");
    assert.equal(globalEqPresetsFrom(old).status, "observed");
    // A snapshot object whose version moved on while the old projection stayed attached.
    const moved = { ...old, stateVersion: { session: "qc-state-v1.session-b", revision: 1 } };
    assert.deepEqual(globalEqPresetsFrom(moved), { status: "unavailable", reason: "session-mismatch" });
    assert.match(admitGlobalEqPreset(moved, preset("a", true)) ?? "", /has not been observed/);
    // An old-session event cannot reach a new-session snapshot through the reducer either.
    const base = { ...(old as object), stateVersion: { session: "qc-state-v1.session-b", revision: 2 } } as never;
    const afterEvent = applyQcStateUpdate(base, {
      kind: "modelPresetCatalog", modelPresetCatalog: catalog({ session: "qc-state-v1.session-a", generation: 50 }),
      stateVersion: { session: "qc-state-v1.session-a", revision: 9 }
    });
    assert.equal(afterEvent, base);
    // A new-session projection is adopted, and only for the session it names.
    const adopted = applyQcStateUpdate({ ...(base as object), modelPresetCatalog: undefined } as never, {
      kind: "modelPresetCatalog", modelPresetCatalog: catalog({ session: "qc-state-v1.session-b", generation: 1 }),
      stateVersion: { session: "qc-state-v1.session-b", revision: 3 }
    });
    assert.equal(globalEqPresetsFrom(adopted).status, "observed");
  });

  test("a missing, empty or foreign session is a mismatch on either side, and an unfenced projection is never adopted", () => {
    assert.deepEqual(globalEqPresetsFor(catalog(), undefined), { status: "unavailable", reason: "session-mismatch" });
    assert.deepEqual(globalEqPresetsFor(catalog(), ""), { status: "unavailable", reason: "session-mismatch" });
    assert.deepEqual(globalEqPresetsFor(catalog({ session: "" }), ""), { status: "unavailable", reason: "session-mismatch" });
    assert.deepEqual(globalEqPresetsFor(catalog({ session: "other" }), SESSION), { status: "unavailable", reason: "session-mismatch" });
    assert.equal(globalEqPresetsFor(catalog(), SESSION).status, "observed");
    const unfenced = catalog({ session: "", generation: 50 });
    assert.equal(adoptModelPresetCatalog(undefined, unfenced), undefined);
    const held = catalog({ generation: 2 });
    assert.equal(adoptModelPresetCatalog(held, unfenced), held);
  });

  test("a workspace document never persists the device preset list, whatever else the snapshot holds", () => {
    const withCatalog = { ...demoSnapshot, modelPresetCatalog: catalog({ entries: [preset("My Secret Global EQ", false)] }) };
    assert.ok(JSON.stringify(withCatalog).includes("My Secret Global EQ"), "the fixture does carry a preset name");
    const document = buildWorkspaceDocument({ snapshot: withCatalog, selectedBlockId: "b1", formFactorId: "f1", savedAt: "2026-10-08T00:00:00.000Z" });
    const text = JSON.stringify(document);
    assert.ok(!text.includes("modelPresetCatalog"), "no catalog key in the saved document");
    assert.ok(!text.includes("My Secret Global EQ"), "no preset name in the saved document");
    assert.equal(document.snapshot.presetName, demoSnapshot.presetName, "the rest of the snapshot is kept");
    assert.equal(document.source.presetName, demoSnapshot.presetName);
    assert.deepEqual(document.ui, { selectedBlockId: "b1", formFactorId: "f1" });
    assert.ok("modelPresetCatalog" in withCatalog, "the live snapshot is not modified");
  });

  test("the real save path never writes the device preset list, through the real reducer and either writer", async () => {
    // A host-shaped projection is folded into a snapshot by the real reducer, then saved through the real save function.
    const secret = "Global EQ Preset Nobody Else Has";
    const event = {
      kind: "modelPresetCatalog" as const,
      stateVersion: { session: SESSION, revision: 6 },
      modelPresetCatalog: catalog({ entries: [preset(secret, false)] })
    };
    const live = applyQcStateUpdate({ ...demoSnapshot, stateVersion: { session: SESSION, revision: 5 } }, event);
    assert.ok(JSON.stringify(live).includes(secret), "the reducer put the list on the live snapshot");

    const written: Array<{ via: string; text: string }> = [];
    const writer: WorkspaceWriter = {
      save: async (path, document) => { written.push({ via: `save:${path}`, text: JSON.stringify(document) }); return { cancelled: false, path, name: "w.qcw" }; },
      saveAs: async (document, name) => { written.push({ via: `saveAs:${name}`, text: JSON.stringify(document) }); return { cancelled: false, path: "C:/w.qcw", name: "w.qcw" }; }
    };
    const base = { snapshot: live, selectedBlockId: "b1", formFactorId: "f1" };
    const saved = await saveWorkspaceDocument(writer, { ...base, path: "C:/existing.qcw", saveAs: false, suggestedName: "1A Name" });
    const copied = await saveWorkspaceDocument(writer, { ...base, path: "C:/existing.qcw", saveAs: true, suggestedName: "1A Name" });
    const fresh = await saveWorkspaceDocument(writer, { ...base, saveAs: false, suggestedName: "1A Name" });

    assert.deepEqual(written.map((entry) => entry.via), ["save:C:/existing.qcw", "saveAs:1A Name", "saveAs:1A Name"], "the writer each case reaches");
    for (const entry of written) {
      assert.ok(!entry.text.includes(secret), `${entry.via}: no preset name reached the writer`);
      assert.ok(!entry.text.includes("modelPresetCatalog"), `${entry.via}: no catalog key reached the writer`);
      assert.ok(entry.text.includes(demoSnapshot.presetName), `${entry.via}: the rest of the snapshot did`);
    }
    for (const kept of [saved.document, copied.document, fresh.document]) {
      assert.ok(!JSON.stringify(kept).includes(secret), "the copy the caller keeps is stripped too");
    }
    assert.ok(JSON.stringify(live).includes(secret), "the live snapshot is not modified by saving");
  });

  test("a saved or exported snapshot carries no device preset list", () => {
    const snapshot = { presetName: "x", modelPresetCatalog: catalog(), stateVersion: { session: SESSION, revision: 1 } };
    const stripped = withoutModelPresetCatalog(snapshot) as Record<string, unknown>;
    assert.equal("modelPresetCatalog" in stripped, false);
    assert.equal(stripped.presetName, "x");
    assert.equal("modelPresetCatalog" in snapshot, true, "the live snapshot is not modified");
  });


  function unavailableOf(value: GlobalEqPresets): Extract<GlobalEqPresets, { status: "unavailable" }> {
    assert.equal(value.status, "unavailable");
    return value as Extract<GlobalEqPresets, { status: "unavailable" }>;
  }

}
