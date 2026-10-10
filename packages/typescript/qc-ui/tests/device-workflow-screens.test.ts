import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { LOOPER_X_ACTIONS, mergeModeSlots, QC_MODE_CYCLES, splitModeSlot } from "../src/device-workflow-model.ts";

test("Looper X screen exposes each semantic action using the gateway command spelling", () => {
  assert.deepEqual(LOOPER_X_ACTIONS.map(([command]) => command), ["duplicate", "oneShot", "halfSpeed", "punch", "record", "play", "reverse", "undoRedo"]);
});

test("production screens retain the device-owned CorOS layout and shared glyphs", () => {
  const looper = readFileSync(new URL("../src/looper-x-screen.tsx", import.meta.url), "utf8");
  const modes = readFileSync(new URL("../src/modes-configuration-screen.tsx", import.meta.url), "utf8");
  assert.match(looper, /className="qc-screen coros-looper"/);
  assert.match(looper, /className="looper-timeline"/);
  assert.match(looper, /className="looper-actions"/);
  assert.match(looper, /QcLooperActionGlyph/);
  assert.match(modes, /className="coros-modes-configuration"/);
  assert.match(modes, /className="modes-cycle"/);
  assert.match(modes, /QcModeGlyph/);
});

test("the live Grid keeps view state separate from its grouped action contract", () => {
  const surface = readFileSync(new URL("../src/quad-cortex-surface.tsx", import.meta.url), "utf8");
  assert.match(surface, /type GridActions = Pick<QuadCortexSurfaceProps,[\s\S]*?"onAction"[\s\S]*?"onContextAction"/);
  assert.match(surface, /function CorOsGrid\(\{ snapshot, presetSlotAccent, selectedBlockId, gridActions \}/);
  assert.match(surface, /gridActions=\{\{ onAction, onOpenPreset, onUndo, canUndo, undoLabel, onSave, onOpenRouting, onRefresh, presetDirectory, routingPicker, addDevicePicker, savePreset, onContextAction, disabledContextActions \}\}/);
  assert.doesNotMatch(surface, /<CorOsGrid[^>]+\bonAction=\{onAction\}/);
});

test("the Global EQ picker reads the host's session fold and distinguishes unavailable from empty", () => {
  const screen = readFileSync(new URL("../src/io-global-eq-screens.tsx", import.meta.url), "utf8");
  const hook = readFileSync(new URL("../src/use-io-eq-workflows.ts", import.meta.url), "utf8");
  assert.ok(screen.includes('workflow.globalEq.status === "observed" ? workflow.globalEq.presets : []'));
  assert.ok(screen.includes("globalEqUnavailableNotice(workflow.globalEq)"), "unavailable has its own wording");
  assert.ok(screen.includes("No Global EQ presets observed in this session."));
  assert.ok(!screen.includes("No Global EQ presets were returned"));
  assert.ok(!screen.includes("modelPresets"), "the picker has no list of its own to fall back on");
  assert.doesNotMatch(hook, /acceptModelPresets|setModelPresets/);
  assert.ok(hook.includes("globalEqPresetsFor(catalog, catalogSession)"));
  assert.ok(hook.includes("[connected, catalog, catalogSession]"), "memoised on the two values the decision depends on");
  assert.ok(hook.includes("connected ? globalEqPresetsFor(catalog, catalogSession)"), "a disconnected workflow offers nothing");
});

test("loading a Global EQ preset is refused before the gateway unless the host's fold lists the tuple", () => {
  const hook = readFileSync(new URL("../src/use-io-eq-workflows.ts", import.meta.url), "utf8");
  const load = hook.slice(hook.indexOf("loadEqPreset:"));
  const admission = load.indexOf("admitGlobalEqPreset(snapshot, preset)");
  assert.ok(admission > 0, "loadEqPreset must admit the tuple");
  assert.ok(admission < load.indexOf("gateway.loadGlobalEqModelPreset"), "admission precedes the gateway call");
  assert.ok(load.slice(admission, admission + 160).includes("if (refusal)"));
});

test("secondary tripwire: the Windows app saves a workspace only through saveWorkspaceDocument", () => {
  // The behaviour test in qc-core runs the real save function with a recording writer; this only watches
  // the call site. It is a tripwire for the current shape, not proof that no other path exists: the Rust
  // command that writes the file accepts any object with version 1 and an object snapshot, so nothing
  // behind this checks the catalog is absent. Line endings are normalised so a CRLF checkout cannot
  // change the result.
  const read = (url: URL) => readFileSync(url, "utf8").replaceAll("\r\n", "\n");
  const app = read(new URL("../../../../apps/windows/src/App.tsx", import.meta.url));
  const saver = read(new URL("../../qc-core/src/workspace-save.ts", import.meta.url));
  assert.ok(saver.includes("withoutModelPresetCatalog(snapshot)"), "the builder strips the catalog");
  assert.ok(app.includes("await saveWorkspaceDocument(workspaceFiles, {"), "the app saves through the function");
  assert.equal((app.match(/workspaceFiles\.(?:save|saveAs)\(/g) ?? []).length, 0, "no direct writer call in App.tsx");
  assert.equal((app.match(/\bsaveWorkspaceDocument\(/g) ?? []).length, 1, "one save call site");
  assert.ok(!/\bcreateWorkspaceDocument\b|\bbuildWorkspaceDocument\b/.test(app), "the component builds no document of its own");
  assert.ok(!/\.\.\.\s*document\b/.test(app) && !/\bdocument\s*\.\s*snapshot\b/.test(app), "no spread or replacement of the saved document");
  assert.ok(!/\bversion:\s*1\b/.test(app), "no hand-built version-1 document in App.tsx");
  assert.ok(!app.includes("JSON.stringify(snapshot"), "no other path serialises the live snapshot");
});

test("no fake workflow or exported document can hand a screen a missing or foreign preset list", () => {
  const audit = readFileSync(new URL("../src/production-screen-audit.tsx", import.meta.url), "utf8");
  const transfer = readFileSync(new URL("../src/preset-transfer.ts", import.meta.url), "utf8");
  assert.ok(audit.includes('const globalEq: IoEqWorkflow["globalEq"] ='), "the audit workflow has a typed Global EQ list");
  assert.ok(!audit.includes("as unknown as IoEqWorkflow"), "the cast no longer hides a missing member");
  assert.ok(transfer.includes("delete published.modelPresetCatalog"), "an exported preset carries no device preset list");
});

test("mode cycle screen reorder is ordered, bounded, and preserves a copy", () => {
  assert.equal(QC_MODE_CYCLES.length, 9);
  assert.deepEqual(mergeModeSlots([0, 1, 2], 0, 1), [3, 2]);
  assert.deepEqual(mergeModeSlots([0, 1, 2], 2, 0), [7, 1]);
  assert.deepEqual(splitModeSlot([3, 2], 0), [0, 1, 2]);
  assert.equal(splitModeSlot([0, 3, 2], 1), undefined, "splitting beyond three modes is refused");
});
