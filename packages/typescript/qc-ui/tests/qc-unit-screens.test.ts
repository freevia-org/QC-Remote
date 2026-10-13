import assert from "node:assert/strict";
import test from "node:test";
import { classifyQcGraphicsTree, type QcObservedScreenKind } from "../src/use-adaptive-screen-polling.ts";
import { appScreenKind, closeOnUnit, createQcScreenLatch, openOnUnit, QC_UNIT_MIRROR_KINDS, QC_UNIT_SCREENS, QC_UPDATE_SCREEN } from "../src/qc-unit-screens.ts";

const tree = (...classes: string[]) => ["zenUI::RootGraphicsItem", ...classes.map((name) => ` zenUI::${name}`)].join("\n");

test("unknown overlays refuse generic navigation touches without claiming the covered dialog closed", async () => {
  const poll = { intervalMs: 1, timeoutMs: 0 };
  for (const overlay of ["SomeNewPopup", "SomeNewDialog", "SomeNewOverlay", "SomeNewPanel"]) {
    const taps: number[][] = [];
    const gateway = {
      graphicsTree: async () => ({ tree: tree("SettingsDialog", overlay) }),
      tapScreen: async (x: number, y: number) => { taps.push([x, y]); },
      longTapScreen: async () => { throw new Error("Unexpected long tap"); },
      swipeScreen: async () => { throw new Error("Unexpected swipe"); }
    };
    assert.equal(await closeOnUnit(gateway, poll, "settings"), false, overlay);
    assert.equal(await openOnUnit(gateway, poll, "settings"), "busy", overlay);
    assert.deepEqual(taps, [], overlay);
  }
  let reads = 0;
  const taps: number[][] = [];
  const changed = {
    graphicsTree: async () => ({ tree: ++reads === 1 ? tree("SettingsDialog") : tree("SettingsDialog", "SomeNewPopup") }),
    tapScreen: async (x: number, y: number) => { taps.push([x, y]); }
  };
  assert.equal(await closeOnUnit(changed as Parameters<typeof closeOnUnit>[0], poll, "settings"), false);
  assert.deepEqual(taps, [[743, 30]], "the new popup neither proves closure nor permits a second check tap");
});

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
  assert.ok(!probes.includes("midi-out") && !probes.includes("directory"), "measured screens are no longer probe-needed");
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
const poll = { intervalMs: 1, timeoutMs: 2, scrollSettleMs: 0, rowDelayMs: 0, rowTimeoutMs: 2 };

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
  assert.deepEqual(settings.log, ["tap:764,22", "swipe:600,420,600,150", "swipe:600,420,600,150", "tap:600,406"], "Settings goes through the preset menu, scrolled to its bottom first");
  const fromMenu = unit("MainMenuPopup", { "tap:600,406:MainMenuPopup": "SettingsDialog" });
  assert.equal(await openOnUnit(fromMenu.gateway, poll, "settings"), "shown");
  assert.deepEqual(fromMenu.log, ["swipe:600,420,600,150", "swipe:600,420,600,150", "tap:600,406"], "already in the menu: the scroll, then only the row");
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
  assert.equal(await openOnUnit(moved.gateway, poll, "settings"), "mismatch");
  assert.deepEqual(moved.log, ["tap:764,22", "swipe:600,420,600,150", "swipe:600,420,600,150", "tap:600,406", "tap:764,22"], "the menu is closed again, not left open");
});

test("menu navigation preserves the measured top and bottom routes when the screen remains current", async () => {
  const routes = [
    { target: "settings", row: "600,406", screen: "SettingsDialog", drag: "600,420,600,150" },
    { target: "tempo", row: "610,286", screen: "MetronomeEditor", drag: "600,420,600,150" },
    { target: "midi-out", row: "580,406", screen: "MidiMatrixDialog", drag: "600,150,600,420" },
    { target: "copy-scene", row: "582,346", screen: "CopySceneDialog", drag: "600,150,600,420" }
  ] as const;
  for (const route of routes) {
    const live = unit("MainMenuPopup", { [`tap:${route.row}:MainMenuPopup`]: route.screen });
    assert.equal(await openOnUnit(live.gateway, poll, route.target, undefined, { swap: true }), "shown");
    assert.deepEqual(live.log, [`swipe:${route.drag}`, `swipe:${route.drag}`, `tap:${route.row}`]);
  }
});

test("each delayed menu touch is refused when the same session leaves the menu", async () => {
  for (const target of ["settings", "midi-out"] as const) {
    for (const afterSwipes of [0, 1, 2]) {
      let reads = 0, swipes = 0;
      let screen = "MainMenuPopup";
      const log: string[] = [];
      const gateway = {
        graphicsTree: async () => {
          if (++reads > 1 && afterSwipes === 0) screen = "SettingsDialog";
          return { tree: tree("Grid", screen, ...(screen === "SettingsDialog" ? ["UpdaterSettings"] : [])) };
        },
        swipeScreen: async (x: number, y: number, toX: number, toY: number) => {
          log.push(`swipe:${x},${y},${toX},${toY}`);
          if (++swipes === afterSwipes) screen = "SettingsDialog";
        },
        tapScreen: async () => { log.push("unexpected tap"); },
        longTapScreen: async () => { log.push("unexpected hold"); }
      } as never;
      assert.equal(await openOnUnit(gateway, poll, target), "mismatch", `${target}: switch after ${afterSwipes} swipes`);
      assert.equal(log.length, afterSwipes);
      assert.ok(log.every((entry) => entry.startsWith("swipe:")), "No row tap or cleanup tap may enter the new screen");
    }
  }
});

test("search rechecks Directory and the block menu rechecks the actual panel before tapping", async () => {
  for (const initial of ["DirectoryDialog", "ParameterEditor"]) {
    let reads = 0;
    const touches: string[] = [];
    const gateway = {
      graphicsTree: async () => ({ tree: ++reads === 1 ? tree("Grid", initial) : tree("Grid", "SettingsDialog") }),
      tapScreen: async () => { touches.push("tap"); },
      swipeScreen: async () => { touches.push("swipe"); },
      longTapScreen: async () => { touches.push("hold"); }
    } as never;
    assert.equal(await openOnUnit(gateway, poll, initial === "DirectoryDialog" ? "search" : "block-menu"), "mismatch");
    assert.deepEqual(touches, [], "Grid under Settings does not authorize the old panel's kebab or Directory search icon");
  }
  const directory = unit("DirectoryDialog", { "tap:634,30:DirectoryDialog": "SearchDialog" });
  assert.equal(await openOnUnit(directory.gateway, poll, "search"), "shown");
  assert.deepEqual(directory.log, ["tap:634,30"]);
});

test("a replacement lane or splitter panel cannot inherit the measured ParameterEditor kebab", async () => {
  for (const replacement of ["LaneInputControlEditor", "LaneOutputControlEditor", "SMParamsEditor", "ParameterEditorHeader"]) {
    let reads = 0;
    const touches: string[] = [];
    const gateway = {
      graphicsTree: async () => ({ tree: tree("Grid", ++reads === 1 ? "ParameterEditor" : replacement) }),
      tapScreen: async () => { touches.push("tap"); },
      swipeScreen: async () => { touches.push("swipe"); },
      longTapScreen: async () => { touches.push("hold"); }
    } as never;
    const result = await openOnUnit(gateway, poll, "block-menu");
    assert.ok(result === "no-route" || result === "mismatch", replacement);
    assert.deepEqual(touches, [], `${replacement} is not the measured ParameterEditor panel`);
  }
  const touches: string[] = [];
  let menuOpen = false;
  const normal = {
    graphicsTree: async () => ({ tree: tree("Grid", "ParameterEditor", ...(menuOpen ? ["_OptionsPopup"] : [])) }),
    tapScreen: async (x: number, y: number) => { touches.push(`tap:${x},${y}`); menuOpen = true; },
    swipeScreen: async () => undefined, longTapScreen: async () => undefined
  } as never;
  assert.equal(await openOnUnit(normal, poll, "block-menu"), "shown");
  assert.deepEqual(touches, ["tap:30,226"]);
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

test("Settings with a busy indicator (the Updates pane) is Settings, drawn by the app and never touched; a bare busy screen stays a picture", async () => {
  const nl = String.fromCharCode(10);
  const tree = (...names: string[]) => ["zenUI::RootGraphicsItem", ...names.map((name) => `zenUI::${name}`)].join(nl);
  assert.equal(classifyQcGraphicsTree(tree("SettingsDialog", "SettingsMenu", "UpdaterSettings", "RotatingBusyIndicator")), "settings");
  assert.equal(classifyQcGraphicsTree(tree("SettingsDialog", "RotatingBusyIndicator", "MessageDialog")), "message-dialog", "a confirmation over Settings is still its own screen");
  assert.equal(classifyQcGraphicsTree(tree("Grid", "RotatingBusyIndicator")), "busy");
  assert.equal(classifyQcGraphicsTree(tree("RotatingBusyIndicator")), "busy");
  const { untouchableUnitScreen } = await import("../src/qc-unit-screens.ts");
  for (const names of [["SettingsDialog", "RotatingBusyIndicator"], ["SettingsDialog", "Keyboard"], ["SettingsDialog", "MessageDialog"], ["SettingsDialog", "UpdaterSettings", "RotatingBusyIndicator"]]) {
    assert.ok(untouchableUnitScreen(tree(...names)), names.join("+"));
  }
});

test("the picture of the unit draws only a frame with the unit's 800x480 shape", async () => {
  const { isUnitFrame, pngSize } = await import("../src/qc-unit-screens.ts");
  const header = (width: number, height: number) => {
    const bytes = new Uint8Array(33);
    bytes.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52]);
    new DataView(bytes.buffer).setUint32(16, width);
    new DataView(bytes.buffer).setUint32(20, height);
    return Buffer.from(bytes).toString("base64");
  };
  assert.deepEqual(pngSize(header(800, 480)), { width: 800, height: 480 });
  assert.equal(isUnitFrame(header(800, 480)), true);
  assert.equal(isUnitFrame(header(400, 240)), true, "the same shape at another scale is the same screen");
  assert.equal(isUnitFrame(header(800, 120)), false, "a squashed strip is not drawn");
  assert.equal(isUnitFrame(header(800, 600)), false);
  assert.equal(isUnitFrame("not a png"), false);
});
