import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import { bundleModule, findBrowser, makeWorkDirectory, uiSource, withBrowser } from "./support/display-page.ts";
import { gigSceneToolColor } from "../src/gig-view-colors.ts";

/*
 * Pins the Preset MIDI Out and Gig View screens to the pixels of a 4.1.0 unit frame: the real components are
 * laid out in headless Chrome on an 800x480 canvas, so every box below is a pixel of the unit's screen.
 */

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the screen geometry check" : false;

type Box = [number, number, number, number];
interface MidiMeasure { trash: Box; trashDisabled: boolean; trashColor: string; trashBackground: string; check: Box; checkDisabled: boolean; glyph: Box; panel: Box; pedals: Box[]; ribs: number[]; labels: Array<{ text: string; box: Box }>; presetLoad: { box: Box; border: string[]; color: string }; dots: Array<{ box: Box; background: string }>; trashRule: { left: string; top: string; width: string; height: string; background: string }; pedalFills: { body: string; plate: string; rib: string } }
interface GigMeasure { tiles: Box[]; toolColors: string[]; ghost: Array<{ textAlign: string }>; mode: Box; scene: Box; check: Box; rules: Array<{ left: string; top: string; width: string; height: string }> }

const MEASURE_MIDI = `(async () => {
  await document.fonts.ready;
  const box = (element) => { const r = element.getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; };
  const trash = document.querySelector('.midi-trash'); const check = document.querySelector('header button:not(.midi-trash)');
  const after = getComputedStyle(trash, '::after');
  const dots = [...document.querySelectorAll('.midi-switch i')].map((element) => ({ box: box(element), background: getComputedStyle(element).backgroundColor }));
  const load = document.querySelector('.midi-preset-load'); const loadStyle = getComputedStyle(load);
  const pedals = [...document.querySelectorAll('.qc-midi-out-pedal')];
  const css = (selector, property) => getComputedStyle(document.querySelector(selector))[property];
  return {
    trash: box(trash), trashDisabled: trash.disabled, trashColor: getComputedStyle(trash).color, trashBackground: getComputedStyle(trash).backgroundColor,
    check: box(check), checkDisabled: check.disabled, glyph: box(trash.querySelector('svg')), panel: box(document.querySelector('.midi-footswitches')),
    pedals: pedals.map(box), ribs: pedals.map((pedal) => pedal.querySelectorAll('.qc-midi-out-pedal-ribs rect').length),
    labels: [...document.querySelectorAll('.midi-expression-source > span')].map((element) => { const range = document.createRange(); range.selectNodeContents(element); const r = range.getBoundingClientRect(); return { text: element.textContent, box: [r.left, r.top, r.width, r.height] }; }),
    presetLoad: { box: box(load), border: [loadStyle.borderTopWidth, loadStyle.borderLeftWidth, loadStyle.borderTopColor], color: loadStyle.color },
    dots, trashRule: { left: after.left, top: after.top, width: after.width, height: after.height, background: after.backgroundColor },
    pedalFills: { body: css('.qc-midi-out-pedal-body', 'fill'), plate: css('.qc-midi-out-pedal-plate', 'fill'), rib: css('.qc-midi-out-pedal-ribs rect', 'fill') }
  };
})()`;

const MEASURE_GIG = `(async () => {
  await document.fonts.ready;
  const box = (element) => { const r = element.getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; };
  const tiles = [...document.querySelectorAll('.gig-scene-tile')];
  const rule = (selector) => { const s = getComputedStyle(document.querySelector(selector)); return { left: s.left, top: s.top, width: s.width, height: s.height }; };
  const scene = document.querySelector('.gig-scene');
  const before = getComputedStyle(scene, '::before'), after = getComputedStyle(scene, '::after');
  return {
    tiles: tiles.map(box), toolColors: tiles.map((tile) => getComputedStyle(tile.querySelector('.gig-scene-tools')).color),
    ghost: tiles.map((tile) => ({ textAlign: getComputedStyle(tile.querySelector('b')).textAlign })),
    mode: box(document.querySelector('.gig-mode')), scene: box(scene), check: box(document.querySelector('header button:last-child')),
    rules: [before, after].map((s) => ({ left: s.left, top: s.top, width: s.width, height: s.height }))
  };
})()`;

let midi: MidiMeasure;
let midiWithMessages: { trashDisabled: boolean };
let gig: GigMeasure;

test("lay out the Preset MIDI Out and Gig View screens at the unit's 800x480", { skip, timeout: 240_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { module: bundle, css } = await bundleModule<{ render(kind: string): Promise<string> }>(work.path, "screens", `
import { createElement } from "react";
import { prerenderToNodeStream } from "react-dom/static";
import { demoSnapshot } from "@qc-remote/core";
import "@qc-remote/theme/theme.css";
import { PresetMidiOutEditor } from ${JSON.stringify(join(uiSource, "preset-midi-out-editor.tsx"))};
import { CorOsGigView } from ${JSON.stringify(join(uiSource, "coros-gig-view.tsx"))};
import ${JSON.stringify(join(uiSource, "coros-screen-fixtures.tsx"))};
const noop = () => undefined;
const snapshot = { ...demoSnapshot, presetLocation: "7F", presetName: "Preset A", activeScene: 0, mode: "SCENE", scenes: ["", "", "", "", "", "", "", ""], sceneColors: undefined, midiOut: [], presetLoadMidiOut: [] };
const workflow = (value) => ({ snapshot: value, demo: false, pending: false, error: "", status: null, contextVersion: 0, contextCurrent: () => true, async save() {}, async clearAll() {}, clearError: noop, clearStatus: noop });
export async function render(kind) {
  const element = kind === "gig" ? createElement(CorOsGigView, { snapshot, onClose: noop })
    : kind === "midi-assigned" ? createElement(PresetMidiOutEditor, { workflow: workflow({ ...snapshot, midiOut: [{ source: 0, messages: [{ type: 1, channel: 1, param1: 7, param2: 100, param3: 0 }] }] }), onClose: noop })
    : createElement(PresetMidiOutEditor, { workflow: workflow(snapshot), onClose: noop });
  const { prelude } = await prerenderToNodeStream(element);
  const chunks = [];
  for await (const chunk of prelude) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}
`);
    const pages = join(work.path, "pages");
    mkdirSync(pages, { recursive: true });
    writeFileSync(join(pages, "screens.css"), css);
    const urls: Record<string, string> = {};
    for (const kind of ["midi", "midi-assigned", "gig"]) {
      const file = join(pages, `${kind}.html`);
      writeFileSync(file, `<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="screens.css"><style>html,body{margin:0;background:#000}.qc-screen-fixture-root{position:absolute;left:0;top:0;width:800px;height:480px;container-type:size}</style></head><body><div class="qc-screen-fixture-root">${await bundle.render(kind)}</div></body></html>`);
      urls[kind] = pathToFileURL(file).href;
    }
    await withBrowser(browser, work.path, [], async (page) => {
      midi = await page([800, 480], urls.midi, MEASURE_MIDI) as MidiMeasure;
      midiWithMessages = await page([800, 480], urls["midi-assigned"], MEASURE_MIDI) as { trashDisabled: boolean };
      gig = await page([800, 480], urls.gig, MEASURE_GIG) as GigMeasure;
    });
  } finally {
    work.cleanup();
  }
});

const near = (actual: readonly number[], expected: readonly number[], tolerance: number, label: string) => {
  assert.equal(actual.length, expected.length, label);
  expected.forEach((value, index) => assert.ok(Math.abs(actual[index] - value) <= tolerance, `${label}: ${actual.map((v) => v.toFixed(2)).join(", ")} is not ${expected.join(", ")}`));
};

test("MIDI Out header: delete tile 610,8 66x44, thin dim glyph at 633,18, 2x22 rule, Close tile 694,8 98x44", { skip }, () => {
  near(midi.trash, [610, 8, 66, 44], 0.6, "delete tile");
  near(midi.check, [694, 8, 98, 44], 0.6, "close tile");
  near(midi.glyph, [633, 18, 20, 24], 0.6, "delete glyph");
  assert.equal(midi.trashBackground, "rgb(8, 16, 8)");
  assert.equal(midi.trashColor, "rgb(49, 60, 49)", "the glyph is the unit's dim green-grey");
  assert.deepEqual([midi.trashRule.width, midi.trashRule.height, midi.trashRule.background], ["2px", "22px", "rgb(24, 28, 24)"]);
  assert.equal(Number.parseFloat(midi.trashRule.left), 66 + 8);
  assert.equal(Number.parseFloat(midi.trashRule.top), 11);
});

test("MIDI Out delete is inert while nothing is assigned and live once a message exists", { skip }, () => {
  assert.equal(midi.trashDisabled, true, "no source carries a message, as on the unit");
  assert.equal(midi.checkDisabled, false);
  assert.equal(midiWithMessages.trashDisabled, false, "with a message to clear the button is the ordinary enabled one");
});

test("MIDI Out expression pedals: art boxes 108x260 at (522,139) and (668,139), seven toe and five heel ribs", { skip }, () => {
  near(midi.pedals[0], [522, 139, 108, 260], 0.6, "EXP 1 art");
  near(midi.pedals[1], [668, 139, 108, 260], 0.6, "EXP 2 art");
  assert.deepEqual(midi.ribs, [12, 12]);
  assert.deepEqual(midi.pedalFills, { body: "rgb(8, 8, 8)", plate: "rgb(41, 44, 41)", rib: "rgb(33, 36, 33)" });
  // The unit's EXP 1 / EXP 2 caps sit at y 115..126, 4.5 px right of and 6 px left of each pedal's own centre.
  near(midi.labels[0].box.slice(0, 2), [559, 108], 4, "EXP 1 label");
  assert.ok(Math.abs(midi.labels[0].box[0] + midi.labels[0].box[2] / 2 - 580.5) <= 1.5, "EXP 1 centre");
  assert.ok(Math.abs(midi.labels[1].box[0] + midi.labels[1].box[2] / 2 - 715.5) <= 1.5, "EXP 2 centre");
});

test("MIDI Out footswitch panel: 458x289 at (25,109), 3 px black preset-load box, 24 px dots in the screen's base colour", { skip }, () => {
  near(midi.panel, [25, 109, 458, 289], 0.6, "panel");
  near(midi.presetLoad.box, [143, 121, 222, 127], 0.6, "preset load box");
  assert.deepEqual(midi.presetLoad.border.slice(0, 2), ["3px", "3px"]);
  assert.equal(midi.presetLoad.color, "rgb(222, 223, 222)");
  assert.equal(midi.dots.length, 8);
  midi.dots.forEach((dot, index) => {
    near(dot.box.slice(0, 2), [51 + (index % 4) * 95, index < 4 ? 279 : 350], 1.2, `dot ${index}`);
    near(dot.box.slice(2), [24, 24], 0.6, `dot ${index} size`);
    assert.equal(dot.background, "rgb(16, 16, 16)");
  });
});

test("Gig View scene tile tools are the tile's colour under a 60% wash, sampled for all eight tiles", { skip }, () => {
  const sampled = ["rgb(99, 12, 8)", "rgb(156, 162, 165)", "rgb(165, 166, 156)", "rgb(165, 158, 165)", "rgb(156, 166, 156)", "rgb(165, 158, 156)", "rgb(156, 158, 165)", "rgb(156, 166, 165)"];
  assert.deepEqual(gig.toolColors, sampled);
  assert.deepEqual(gig.ghost.map((entry) => entry.textAlign), Array(8).fill("center"), "the big letter is centred in its tile");
  near(gig.tiles[0], [8, 60, 190, 202], 0.6, "tile A");
  near(gig.tiles[7], [602, 270, 190, 202], 0.6, "tile H");
});

test("Gig View header: mode tile 66 wide ending at 634, Close tile 694,8 98x44, 2x22 rules 8 px either side of the scene badge", { skip }, () => {
  near([gig.mode[0] + gig.mode[2], gig.mode[2]], [634, 66], 0.6, "mode tile");
  near(gig.check, [694, 8, 98, 44], 0.6, "close tile");
  near(gig.scene.slice(0, 2), [644, 8], 0.6, "scene slot");
  assert.deepEqual(gig.rules.map((rule) => [rule.width, rule.height]), [["2px", "22px"], ["2px", "22px"]]);
});

test("gigSceneToolColor derives each tile's glyph colour from the tile fill on the 5/6/5 ladder", () => {
  assert.equal(gigSceneToolColor("#ff2421", true), "#630c08");
  assert.equal(gigSceneToolColor("var(--qc-palette-101c21)", false), "#9ca2a5");
  assert.equal(gigSceneToolColor("var(--qc-palette-292410)", false), "#a5a69c");
  assert.equal(gigSceneToolColor("var(--qc-palette-181829)", false), "#9c9ea5");
});
