import assert from "node:assert/strict";
import test from "node:test";
import { classifyQcGraphicsTree, type QcObservedScreenKind } from "../src/use-adaptive-screen-polling.ts";
import { appScreenKind, closeOnUnit, createQcScreenLatch, openOnUnit, QC_UNIT_MIRROR_KINDS, QC_UNIT_SCREENS, QC_UPDATE_SCREEN } from "../src/qc-unit-screens.ts";

const tree = (...classes: string[]) => ["zenUI::RootGraphicsItem", ...classes.map((name) => ` zenUI::${name}`)].join("\n");

test("every screen class the unit is known to show is recognised, overlays before the screen under them", () => {
  const cases: [string[], QcObservedScreenKind][] = [
    [["GigView"], "gig"], [["TunerDialog"], "tuner"], [["Tuner"], "tuner"], [["MetronomeEditor"], "tempo"], [["RotatingBusyIndicator"], "busy"],
    [["MessageDialog"], "message-dialog"], [["Keyboard"], "keyboard"], [["SearchResultsDialog"], "search"], [["SearchDialog"], "search"],
    [["HybridModeConfigDialog"], "modes"], [["MidiMatrixDialog"], "midi-out"], [["CheatSheetDialog"], "io"],
    [["NeuralCaptureDialog"], "neural-capture"], [["NCMetadataDialog"], "neural-capture"], [["NCModelEditor"], "neural-capture"],
    [["SettingsDialog"], "settings"], [["DSPDiagnosticsDialog"], "settings"], [["ZencoderDiagnosticsDialog"], "settings"], [["SOC2ARMDiagnosticsDialog"], "settings"],
    [["DirectoryDialog"], "directory"], [["CopySceneDialog"], "copy-scene"], [["SceneSelectorPopup"], "scene-selector"], [["PortSelector"], "route-selector"],
    [["MainMenuPopup"], "preset-menu"], [["_OptionsPopup"], "block-menu"], [["ModelMenu"], "device-browser"], [["AssignExpressionPedal"], "expression-assign"],
    [["CpuLoadIndicator"], "cpu-monitor"], [["Looper1TrackModelEditor"], "looper"],
    [["ParameterEditor"], "parameter-editor"], [["SMParamsEditor"], "parameter-editor"], [["LaneInputControlEditor"], "parameter-editor"], [["LaneOutputControlEditor"], "parameter-editor"],
    [["Grid"], "grid"], [["Nothing"], "unknown"]
  ];
  for (const [classes, expected] of cases) assert.equal(classifyQcGraphicsTree(tree(...classes)), expected, classes.join("+"));
  // The tree keeps the screen underneath an overlay: the overlay wins.
  assert.equal(classifyQcGraphicsTree(tree("Grid", "MainMenuPopup")), "preset-menu");
  assert.equal(classifyQcGraphicsTree(tree("Grid", "ParameterEditor", "CheatSheetDialog")), "io");
  assert.equal(classifyQcGraphicsTree(tree("Grid", "MessageDialog", "SettingsDialog")), "message-dialog", "a confirmation over Settings is the confirmation");
  assert.equal(classifyQcGraphicsTree(tree("Grid", "ParameterEditor")), "parameter-editor");
});

test("class names match whole: a name that merely contains another is not that screen", () => {
  assert.equal(classifyQcGraphicsTree(tree("GigViewStompAccessPane")), "unknown", "the Settings > Gig View pane is not Gig View");
  assert.equal(classifyQcGraphicsTree(tree("GridMeter")), "unknown");
  assert.equal(classifyQcGraphicsTree(tree("SettingsDialogHeader")), "unknown");
});

test("the table describes every screen the classifier can return, and the mirrored ones are exactly the ones with no app screen", () => {
  const kinds = new Set<string>();
  for (const name of ["GigView", "TunerDialog", "MetronomeEditor", "RotatingBusyIndicator", "MessageDialog", "Keyboard", "SearchDialog", "HybridModeConfigDialog", "MidiMatrixDialog", "CheatSheetDialog", "NeuralCaptureDialog", "SettingsDialog", "DirectoryDialog", "CopySceneDialog", "SceneSelectorPopup", "PortSelector", "MainMenuPopup", "_OptionsPopup", "ModelMenu", "AssignExpressionPedal", "CpuLoadIndicator", "Looper1TrackModelEditor", "ParameterEditor", "Grid"]) kinds.add(classifyQcGraphicsTree(tree(name)));
  for (const kind of kinds) assert.ok(kind in QC_UNIT_SCREENS, `${kind} has a table entry`);
  for (const kind of QC_UNIT_MIRROR_KINDS) assert.ok(QC_UNIT_SCREENS[kind as keyof typeof QC_UNIT_SCREENS].app.startsWith("unit mirror"), `${kind} is mirrored as a picture`);
  // Delete, Overwrite, Reset and Restart confirmations are never opened or closed by the app.
  assert.equal(QC_UNIT_SCREENS["message-dialog"].open, "never");
  assert.equal(QC_UNIT_SCREENS["message-dialog"].close, "never");
  // Gestures are only claimed for screens with an observed route; the rest wait for a probe on the unit.
  const probes = Object.entries(QC_UNIT_SCREENS).filter(([, entry]) => entry.open === "probe-needed").map(([kind]) => kind).sort();
  assert.ok(["midi-out"].every((kind) => probes.includes(kind)));
});

test("the app's own screen is described in the unit's terms", () => {
  const app = { view: null, deviceSettingsOpen: false, tempoOpen: false, popup: null };
  assert.equal(appScreenKind(app), "grid");
  assert.equal(appScreenKind({ ...app, popup: "grid-menu" }), "preset-menu");
  assert.equal(appScreenKind({ ...app, popup: "scene-menu" }), "scene-selector");
  assert.equal(appScreenKind({ ...app, popup: "mode-menu" }), "grid");
  assert.equal(appScreenKind({ ...app, deviceSettingsOpen: true, popup: "grid-menu" }), "settings");
  assert.equal(appScreenKind({ ...app, tempoOpen: true, deviceSettingsOpen: true }), "tempo");
  for (const [view, kind] of [["tuner", "tuner"], ["gig", "gig"], ["gig-live-tuner", "gig"], ["io-overview", "io"], ["io-usb", "io"], ["global-eq", "io"], ["modes", "modes"], ["midi-out", "midi-out"], ["cpu-monitor", "cpu-monitor"], ["looper-editor", "looper"], ["unit-mirror", "message-dialog"], ["settings-info", "grid"]] as const) {
    assert.equal(appScreenKind({ ...app, view }), kind, view);
  }
});

test("the loop guard: an expectation applies once, per side, and lapses", () => {
  let now = 0;
  const latch = createQcScreenLatch(() => now, 1_000);
  latch.expectUnit("modes");
  assert.equal(latch.takeUnit("settings"), false);
  assert.equal(latch.takeUnit("modes"), true);
  assert.equal(latch.takeUnit("modes"), false, "once");
  latch.expectApp("modes");
  assert.equal(latch.takeUnit("modes"), false, "the two sides keep separate books");
  assert.equal(latch.takeApp("modes"), true);
  latch.expectApp("settings");
  now = 1_001;
  assert.equal(latch.takeApp("settings"), false, "a lost frame cannot silence a later, genuine change");
});

test("updates: the app refuses any tap while the unit shows an update screen", () => {
  for (const text of ["DOWNLOAD UPDATE", "Install update", "Software Update", "Update available"]) assert.match(text, QC_UPDATE_SCREEN);
  for (const text of ["Device Updates", "CHECK FOR UPDATES", "Settings"]) assert.doesNotMatch(text, QC_UPDATE_SCREEN, text);
});

// A scripted unit.
function unit(start: string, responses: Partial<Record<string, string>> = {}) {
  const state = { screen: start };
  const log: string[] = [];
  const gateway = {
    graphicsTree: async () => ({ tree: tree(state.screen) }),
    tapScreen: async (x: number, y: number) => { log.push(`tap:${x},${y}`); const next = responses[`tap:${x},${y}:${state.screen}`]; if (next) state.screen = next; },
    longTapScreen: async (x: number, y: number) => { log.push(`hold:${x},${y}`); const next = responses[`hold:${x},${y}:${state.screen}`]; if (next) state.screen = next; },
    swipeScreen: async (x: number, y: number, toX: number, toY: number) => { log.push(`swipe:${x},${y},${toX},${toY}`); const next = responses[`swipe:${state.screen}`]; if (next) state.screen = next; }
  } as never;
  return { state, log, gateway };
}
const poll = { intervalMs: 1, timeoutMs: 2 };

test("opening on the unit uses each proven gesture, only from the screen it was proven on", async () => {
  const menu = unit("Grid", { "tap:764,22:Grid": "MainMenuPopup" });
  assert.equal(await openOnUnit(menu.gateway, poll, "preset-menu"), "shown");
  assert.deepEqual(menu.log, ["tap:764,22"]);
  const modes = unit("Grid", { "hold:700,70:Grid": "HybridModeConfigDialog" });
  assert.equal(await openOnUnit(modes.gateway, poll, "modes"), "shown");
  assert.deepEqual(modes.log, ["hold:700,70"]);
  const io = unit("Grid", { "swipe:Grid": "CheatSheetDialog" });
  assert.equal(await openOnUnit(io.gateway, poll, "io"), "shown");
  assert.deepEqual(io.log, ["swipe:400,100,400,440"]);
  const settings = unit("Grid", { "tap:764,22:Grid": "MainMenuPopup", "tap:600,406:MainMenuPopup": "SettingsDialog" });
  assert.equal(await openOnUnit(settings.gateway, poll, "settings"), "shown");
  assert.deepEqual(settings.log, ["tap:764,22", "tap:600,406"], "Settings goes through the preset menu");
  const fromMenu = unit("MainMenuPopup", { "tap:600,406:MainMenuPopup": "SettingsDialog" });
  assert.equal(await openOnUnit(fromMenu.gateway, poll, "settings"), "shown");
  assert.deepEqual(fromMenu.log, ["tap:600,406"], "already in the menu: only the row");
});

test("opening on the unit sends nothing over another screen and nothing when it is already there", async () => {
  for (const target of ["preset-menu", "settings", "modes", "io"] as const) {
    for (const over of ["MessageDialog", "TunerDialog", "GigView", "ParameterEditor", "DirectoryDialog"]) {
      const busy = unit(over);
      assert.equal(await openOnUnit(busy.gateway, poll, target), "busy", `${target} over ${over}`);
      assert.deepEqual(busy.log, []);
    }
  }
  const already = unit("HybridModeConfigDialog");
  assert.equal(await openOnUnit(already.gateway, poll, "modes"), "shown");
  assert.deepEqual(already.log, []);
});

test("opening Settings leaves the unit's menu as found when the row was not where expected", async () => {
  const moved = unit("Grid", { "tap:764,22:Grid": "MainMenuPopup", "tap:764,22:MainMenuPopup": "Grid" });
  assert.equal(await openOnUnit(moved.gateway, poll, "settings"), "not-shown");
  assert.deepEqual(moved.log, ["tap:764,22", "tap:600,406", "tap:764,22"], "the menu is closed again, not left open");
});

test("closing on the unit: the kebab toggles the menu, the check button closes a dialog, a nested dialog gets one more tap", async () => {
  const menu = unit("MainMenuPopup", { "tap:764,22:MainMenuPopup": "Grid" });
  assert.equal(await closeOnUnit(menu.gateway, poll, "preset-menu"), true);
  assert.deepEqual(menu.log, ["tap:764,22"]);
  const settings = unit("SettingsDialog", { "tap:743,30:SettingsDialog": "Grid" });
  assert.equal(await closeOnUnit(settings.gateway, poll, "settings"), true);
  assert.deepEqual(settings.log, ["tap:743,30"]);
  const stuck = unit("CheatSheetDialog");
  assert.equal(await closeOnUnit(stuck.gateway, poll, "io"), false);
  assert.deepEqual(stuck.log, ["tap:743,30", "tap:743,30"]);
  const gone = unit("Grid");
  assert.equal(await closeOnUnit(gone.gateway, poll, "modes"), true, "the unit already left it");
  assert.deepEqual(gone.log, []);
});
