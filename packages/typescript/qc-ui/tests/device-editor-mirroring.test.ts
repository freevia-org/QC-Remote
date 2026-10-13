import assert from "node:assert/strict";
import test from "node:test";
import { closeGridBlockOnDevice, openGridBlockOnDevice } from "../src/device-editor-mirroring.ts";
import type { GatewayTransport, GridBlock } from "@qc-remote/client";

function unit(classes: string[], withGrid = true) {
  const taps: string[] = [];
  const gateway = {
    graphicsTree: async () => ({ tree: ["zenUI::RootGraphicsItem", ...(withGrid ? ["zenUI::Grid"] : []), ...classes.map((name) => `zenUI::${name}`)].join("\n") }),
    tapScreen: async (x: number, y: number) => { taps.push(`${x},${y}`); }
  } as unknown as GatewayTransport;
  return { gateway, taps };
}

test("closing the app's editor taps the check button only while the unit shows the parameter editor", async () => {
  const editor = unit(["ParameterEditor"]);
  assert.equal(await closeGridBlockOnDevice(editor.gateway), true);
  assert.deepEqual(editor.taps, ["742,30"]);
  const grid = unit([]);
  const left: string[] = [];
  assert.equal(await closeGridBlockOnDevice(grid.gateway, () => true, (screen) => left.push(screen)), false);
  assert.deepEqual([grid.taps, left], [[], []], "a Grid with nothing over it: nothing to close and nothing to report");
});

const block = { id: "amp", name: "Amp", kind: "amp", row: 1, column: 2 } as GridBlock;

test("opening a block taps its native cell only on the bare Grid, with normal Grid children", async () => {
  const host = unit(["GridCell", "GridBlock", "GridRouting", "Label", "Path", "GridHeader", "ChainItemContainer",
    "ModelItem", "ModelItemOverlays", "NavigationBar", "NavMenuBlock", "PressAnimationView", "SplitControlPointGrid"]);
  assert.equal(await openGridBlockOnDevice(host.gateway, block), true);
  assert.deepEqual(host.taps, ["272,241"]);
});

test("opening a block refuses known and unknown screens over the Grid", async () => {
  for (const classes of [
    ["ParameterEditor"], ["MainMenuPopup"], ["SceneSelectorPopup"], ["CpuLoadIndicator"],
    ["PortSelector"], ["SettingsDialog"], ["DirectoryDialog"], ["CheatSheetDialog"],
    ["MessageDialog"], ["Keyboard"], ["ModelMenu"], ["SMParamsEditor"],
    ["SomeNewPopup"], ["SomeNewDialog"], ["SomeNewOverlay"], ["SomeNewPanel"]
  ]) {
    const host = unit(classes);
    await assert.rejects(openGridBlockOnDevice(host.gateway, block), /covered by another screen/, classes.join());
    assert.deepEqual(host.taps, [], `${classes.join()}: a Grid in the covered tree is not permission to touch it`);
  }
});

test("opening a block refuses a missing Grid, Grid-prefixed impostor, stale operation and invalid cell", async () => {
  for (const classes of [[], ["GridHeader"], ["GridCell"]]) {
    const host = unit(classes, false);
    await assert.rejects(openGridBlockOnDevice(host.gateway, block), /not showing the Grid/);
    assert.deepEqual(host.taps, []);
  }
  const stale = unit([]);
  assert.equal(await openGridBlockOnDevice(stale.gateway, block, () => false), false);
  assert.deepEqual(stale.taps, []);
  const invalid = unit([]);
  await assert.rejects(openGridBlockOnDevice(invalid.gateway, { ...block, column: 8 }), /outside the Quad Cortex Grid/);
  assert.deepEqual(invalid.taps, []);
});

test("an editor the app cannot close on the unit is named, never tapped, and never silently left open", async () => {
  for (const [classes, expected] of [
    [["SMParamsEditor"], "parameter-editor"],
    [["LaneInputControlEditor"], "parameter-editor"],
    [["Keyboard"], "keyboard"],
    [["SomeNewPanel"], "grid"]
  ] as const) {
    const host = unit([...classes]);
    const left: string[] = [];
    assert.equal(await closeGridBlockOnDevice(host.gateway, () => true, (screen) => left.push(screen)), false, classes.join());
    assert.deepEqual(host.taps, [], `${classes.join()}: no touch on a screen the check button is not proven on`);
    if (expected === "grid") assert.deepEqual(left, [], "an unclassified class over the Grid is the probe's job, not a guess");
    else assert.deepEqual(left, [expected]);
  }
});

test("a superseded close reports nothing and taps nothing", async () => {
  const host = unit(["SMParamsEditor"]);
  const left: string[] = [];
  assert.equal(await closeGridBlockOnDevice(host.gateway, () => false, (screen) => left.push(screen)), false);
  assert.deepEqual([host.taps, left], [[], []]);
});

test("the Cab editor, whose tree has no Grid at all, is closed by the check tap", async () => {
  const cab = unit(["ParameterEditor", "ModelEditor", "CabsimModelEditor", "CabsimPanel", "CabsimPanel", "TabBar", "ParameterEditorTitle"], false);
  assert.equal(await closeGridBlockOnDevice(cab.gateway), true);
  assert.deepEqual(cab.taps, ["742,30"]);
});

test("a popup over the editor is named, not tapped over, whether or not the Grid is in the tree", async () => {
  for (const withGrid of [true, false]) {
    const host = unit(["ParameterEditor", "_OptionsPopup"], withGrid);
    const left: string[] = [];
    assert.equal(await closeGridBlockOnDevice(host.gateway, () => true, (screen) => left.push(screen)), false);
    assert.deepEqual([host.taps, left], [[], ["block-menu"]]);
  }
});
