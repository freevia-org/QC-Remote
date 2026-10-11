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
  assert.match(surface, /function CorOsGrid\(\{ snapshot, presetSlotAccent, selectedBlockId, gridActions, cpuColumns \}/);
  assert.match(surface, /gridActions=\{\{ onAction, onOpenPreset, onUndo, canUndo, undoLabel, onSave, onOpenRouting, onRefresh, presetDirectory, routingPicker, addDevicePicker, savePreset, onContextAction, disabledContextActions \}\}/);
  assert.doesNotMatch(surface, /<CorOsGrid[^>]+\bonAction=\{onAction\}/);
});

test("the CPU Monitor is an overlay on the live Grid, never a second Grid", () => {
  const read = (url: URL) => readFileSync(url, "utf8").replaceAll("\r\n", "\n");
  const surface = read(new URL("../src/quad-cortex-surface.tsx", import.meta.url));
  const monitor = read(new URL("../src/coros-cpu-monitor.tsx", import.meta.url));
  const windows = read(new URL("../../../../apps/windows/src/App.tsx", import.meta.url));
  const android = read(new URL("../../../../apps/android/src/App.tsx", import.meta.url));
  // The surface draws the one live Grid, hands it the per-block load, and puts the panel after it.
  assert.match(surface, /<CorOsGrid[\s\S]*?cpuColumns=\{cpuMonitor\?\.cpuColumns\}[\s\S]*?\/>\{cpuMonitor && <CorOsCpuMonitorPanel /);
  assert.match(surface, /cpuLoad=\{cpuColumns\?\.\[block\.row \* QC_GRID_COLUMNS \+ block\.column\]\?\.load\}/);
  // The panel itself draws no Grid; only the fixture wrapper composes it with the simplified one.
  const panel = monitor.slice(monitor.indexOf("export function CorOsCpuMonitorPanel"), monitor.indexOf("export function CorOsCpuMonitor("));
  assert.ok(panel.includes('aria-label="CPU Monitor"') && panel.includes("Close CPU Monitor"));
  assert.ok(!panel.includes("CorOsOfficialGrid"), "the overlay must not bring its own Grid");
  // Hosts must not replace the Grid with a CPU screen: no screen-content branch, and the surface keeps the grid view.
  for (const [name, source, view] of [["windows", windows, "deviceScreenView"], ["android", android, "mobileScreenView"]] as const) {
    assert.ok(!source.includes("<CorOsCpuMonitor"), `${name} must not render the fixture Grid wrapper`);
    assert.ok(!source.includes(`${view} === "cpu-monitor" ? <`), `${name} must not swap the Grid for a CPU screen`);
    assert.ok(source.includes(`cpuMonitorOpen ? undefined : ${view}`), `${name} keeps the surface on the Grid while the monitor is open`);
    assert.ok(source.includes("cpuMonitor={cpuMonitor}"), `${name} passes the monitor to the surface`);
    // Neither host owns CPU readings: both take them from the one shared hook and hand its monitor to the surface.
    assert.ok(source.includes("const cpuTelemetry = useQcCpuTelemetry();"), `${name} uses the shared CPU telemetry hook`);
    assert.ok(source.includes("cpu: cpuTelemetry"), `${name} feeds every state batch to the hook`);
    assert.ok(source.includes("cpuTelemetry.monitor(cpuMonitorOpen, "), `${name} builds the monitor from the hook`);
    assert.ok(!/setCpu(?:Load|Columns)|useState<CpuColumnLoad/.test(source), `${name} keeps no CPU state of its own`);
  }
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

test("both hosts take the screen destination and the Grid context-action routing from the shared hooks", () => {
  const read = (url: URL) => readFileSync(url, "utf8").replaceAll("\r\n", "\n");
  for (const [name, source] of [["windows", read(new URL("../../../../apps/windows/src/App.tsx", import.meta.url))], ["android", read(new URL("../../../../apps/android/src/App.tsx", import.meta.url))]] as const) {
    assert.ok(source.includes("const screenDestination = useQcScreenDestination();"), `${name} uses the shared screen destination`);
    assert.ok(source.includes("qcContextActionHandler({ destination: screenDestination,"), `${name} routes context actions through the shared handler`);
    assert.ok(source.includes("disabledContextActions={QC_DISABLED_CONTEXT_ACTIONS}"), `${name} shares the disabled-action table`);
    assert.ok(!/setScreenDestination|useState<\{ view: CorOsScreenView/.test(source), `${name} keeps no screen-view state of its own`);
    assert.ok(!source.includes("corOsUnavailableContextActionMessage"), `${name} leaves the unavailable-action wording to the router`);
  }
});

test("both hosts take the device screen content from the shared mapping and draw no CorOS screen themselves", () => {
  const read = (url: URL) => readFileSync(url, "utf8").replaceAll("\r\n", "\n");
  for (const [name, source] of [["windows", read(new URL("../../../../apps/windows/src/App.tsx", import.meta.url))], ["android", read(new URL("../../../../apps/android/src/App.tsx", import.meta.url))]] as const) {
    assert.ok(/: qcDeviceScreenContent\(\{/.test(source), `${name} maps the screen through qcDeviceScreenContent`);
    assert.doesNotMatch(source, /<(?:LooperXScreen|ModesConfigurationScreen|PresetMidiOutEditor|CorOs(?:Tuner|Tempo|DeviceSettings|GlobalEq|IoSettings)\w*)\b/, `${name} must not assemble a device screen itself`);
    assert.ok(!source.includes('block.name === "Looper X"'), `${name} leaves the Looper X block lookup to the shared screen actions`);
    assert.ok(!source.includes('looperWorkflow.run("close")'), `${name} closes Looper X only through closeQcLooperScreen`);
  }
});

test("both hosts follow the unit's screen and consume display states through the shared hooks", () => {
  const read = (url: URL) => readFileSync(url, "utf8").replaceAll("\r\n", "\n");
  for (const [name, source] of [["windows", read(new URL("../../../../apps/windows/src/App.tsx", import.meta.url))], ["android", read(new URL("../../../../apps/android/src/App.tsx", import.meta.url))]] as const) {
    assert.ok(source.includes("useQcScreenFollow({"), `${name} follows the unit's screen through the shared hook`);
    assert.ok(source.includes("consumeQcDisplayStates(states, {"), `${name} consumes the display states through the shared function`);
    assert.ok(!/followedScreenKind|useAdaptiveQcScreenPolling|gigViewShown/.test(source), `${name} keeps no screen-follow or Gig-state logic of its own`);
  }
});

test("both hosts open, close and toggle device screens, forward gestures and undo through the shared hooks", () => {
  const read = (url: URL) => readFileSync(url, "utf8").replaceAll("\r\n", "\n");
  for (const [name, source] of [["windows", read(new URL("../../../../apps/windows/src/App.tsx", import.meta.url))], ["android", read(new URL("../../../../apps/android/src/App.tsx", import.meta.url))]] as const) {
    assert.ok(source.includes("useQcDeviceViews({"), `${name} uses the shared device views`);
    assert.ok(source.includes("useQcDeviceUndo({"), `${name} uses the shared undo path`);
    assert.ok(source.includes('onOpenGigView={() => void deviceViews.toggle("gig")}') && source.includes('onOpenIoView={() => void deviceViews.toggle("io")}'), `${name} toggles Gig View and I/O`);
    assert.ok(source.includes("deviceInformation={deviceViews.deviceInformation}"), `${name} shows the Device Information screen`);
    assert.doesNotMatch(source, /showDeviceView\(|\.showGigView\(|\.longTapScreen\(|\.tapScreen\(|\.swipeScreen\(|Gateway\.undo\(|setDeviceInformation/, `${name} sends no device-screen command itself`);
    assert.doesNotMatch(source, /Opening the Quad Cortex|Synchronizing the Quad Cortex/, `${name} takes the connecting wording from the shared placeholder`);
    assert.doesNotMatch(source, /parameterEditorControlSlots|parameterEditorPageSize/, `${name} derives no parameter-editor LEDs of its own`);
    assert.doesNotMatch(source, /rotate: \(role|editorUnhandled|unhandled: \(action/, `${name} leaves the unhandled-control wording to the shared router`);
  }
});
