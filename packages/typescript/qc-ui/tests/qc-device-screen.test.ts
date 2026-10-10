import assert from "node:assert/strict";
import test from "node:test";
import type { GridBlock } from "@qc-remote/client";
import type { CorOsScreenView } from "../src/coros-screen-fixture-data.ts";
import { closeQcLooperScreen, findLooperXBlock, qcLooperScreenActions, resolveQcDeviceScreen } from "../src/qc-device-screen.ts";

const closed = { deviceSettings: false, tempo: false };

test("one precedence chooses the device screen for both hosts", () => {
  const cases: [CorOsScreenView | null, typeof closed, string | undefined][] = [
    [null, closed, undefined],
    ["tuner", closed, "tuner"],
    ["looper-editor", closed, "looper-editor"],
    ["modes", closed, "modes"],
    ["midi-out", closed, "midi-out"],
    ["global-eq", closed, "global-eq"],
    ["io-overview", closed, "io"],
    ["io-usb", closed, "io"],
    // Views the Grid itself draws (or the fixture dispatcher owns) never take over the content slot.
    ["gig", closed, undefined],
    ["cpu-monitor", closed, undefined],
    ["settings-info", closed, undefined],
    ["grid", closed, undefined],
    // The two workflows with their own open flag win over a navigated view, Device Settings first.
    ["tuner", { deviceSettings: true, tempo: false }, "device-settings"],
    ["tuner", { deviceSettings: false, tempo: true }, "tempo"],
    [null, { deviceSettings: true, tempo: true }, "device-settings"]
  ];
  for (const [view, open, expected] of cases) assert.equal(resolveQcDeviceScreen(view, open), expected, `${view} ${JSON.stringify(open)}`);
});

const block = (over: Partial<GridBlock>) => ({ row: 0, column: 0, name: "Amp", modelId: 1, ...over }) as GridBlock;

test("the Looper X block is found by model or by name", () => {
  assert.equal(findLooperXBlock({ blocks: [block({}), block({ name: "Looper X", modelId: 5 })] })?.modelId, 5);
  assert.equal(findLooperXBlock({ blocks: [block({ modelId: 27_000, name: "Other" })] })?.modelId, 27_000);
  assert.equal(findLooperXBlock({ blocks: [block({})] }), undefined);
});

function looperHarness(blocks: GridBlock[] = [block({ modelId: 27_000, name: "Looper X" })]) {
  const log: string[] = [];
  const gate: { release?: () => void } = {};
  const options = {
    snapshot: { blocks },
    looper: { run: async (command: string, value?: number | null) => { log.push(`run:${command}${value ?? ""}`); if (command === "close") await new Promise<void>((resolve) => { gate.release = resolve; }); } },
    grid: { openBlock: async () => { log.push("open-block"); } },
    setView: (next: CorOsScreenView | null | ((current: CorOsScreenView | null) => CorOsScreenView | null)) => { log.push(`view:${typeof next === "function" ? "fn" : next}`); },
    setParameterMenuInitiallyOpen: (open: boolean) => { log.push(`menu:${open}`); },
    notice: (message: string) => { log.push(`notice:${message}`); }
  };
  return { log, gate, options, actions: qcLooperScreenActions(options) };
}

test("closing Looper X tells the unit first and clears the view only afterwards, on both hosts", async () => {
  const { log, gate, options } = looperHarness();
  const closing = closeQcLooperScreen(options);
  await Promise.resolve();
  assert.deepEqual(log, ["run:close"], "the view stays until the unit has been told");
  gate.release!();
  await closing;
  assert.deepEqual(log, ["run:close", "view:null"]);
});

test("the Looper X screen's own Close awaits the same close", async () => {
  const { log, gate, actions } = looperHarness();
  const closing = actions.onClose();
  await Promise.resolve();
  assert.deepEqual(log, ["run:close"]);
  gate.release!();
  await closing;
  assert.deepEqual(log, ["run:close", "view:null"]);
});

test("Looper X Parameters and Menu enter performance mode, leave the screen, then open the block editor", async () => {
  const parameters = looperHarness();
  await parameters.actions.onParameters();
  assert.deepEqual(parameters.log, ["run:performMode1", "view:null", "open-block"]);
  const menu = looperHarness();
  await menu.actions.onMenu();
  assert.deepEqual(menu.log, ["run:performMode1", "menu:true", "view:null", "open-block"]);
});

test("Looper X Parameters and Menu explain themselves when the preset has no Looper X block", async () => {
  const none = looperHarness([block({})]);
  await none.actions.onParameters();
  await none.actions.onMenu();
  assert.deepEqual(none.log, [
    "notice:Add a Looper X block to this preset before opening its parameters.",
    "notice:Add a Looper X block to this preset before opening its menu."
  ]);
});
