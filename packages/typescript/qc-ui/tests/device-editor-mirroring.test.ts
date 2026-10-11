import assert from "node:assert/strict";
import test from "node:test";
import { closeGridBlockOnDevice } from "../src/device-editor-mirroring.ts";
import type { GatewayTransport } from "@qc-remote/client";

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
