import assert from "node:assert/strict";
import test from "node:test";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { bundleModule, findBrowser, makeWorkDirectory, uiSource, withBrowser } from "./support/display-page.ts";
import { decodePng } from "./support/png-decode.ts";

/*
 * The live Modes Configuration screen (zenUI::HybridModeConfigDialog, opened on the unit by a long press on the mode indicator)
 * laid out at the unit's own 800x480 in headless Chrome and compared with what the unit draws. The numbers are measured from the
 * unit's frame with the cycle Scene, Preset, Stomp: the header (title ink 18..179 x 24..40 in #949694; a 66x44 info button at
 * x 610, y 8 filled #181c18 with a 2x22 divider of the same colour at x 684, y 19; the 98x44 confirm plate at x 694), the two
 * description lines (ink y 87..101 and 108..122, a 21px pitch), three 170x80 tiles at x 121, 315 and 509 on y 194..273, the
 * track (2px #292c29 at y 159 and y 235 closed by a half ellipse at each end, the top line broken for the 14px CYCLE label,
 * and a downward arrowhead on the left end and an upward one on the right end at y 190..206), and the underlined
 * "Reset to Defaults" link at x 24 (ink y 440..454, #949694). It needs a Chrome or Chromium (QC_BROWSER_EXECUTABLE); CI always
 * has one. No frame from the unit is stored: only the numbers read from it.
 */

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the Modes Configuration layout check" : false;

const MEASURE = `
(async () => {
  await document.fonts.ready;
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const canvas = document.querySelector(".qc-screen-fixture-root").getBoundingClientRect();
  const box = (element) => { const r = element.getBoundingClientRect(); return [r.left - canvas.left, r.top - canvas.top, r.width, r.height]; };
  const style = (element) => { const s = getComputedStyle(element); return { color: s.color, size: parseFloat(s.fontSize), weight: s.fontWeight, lineHeight: parseFloat(s.lineHeight), background: s.backgroundColor, decoration: s.textDecorationLine, pointerEvents: s.pointerEvents, userSelect: s.userSelect, cursor: s.cursor }; };
  const screen = document.querySelector(".coros-modes-configuration");
  const info = screen.querySelector(".modes-information-glyph");
  const reset = screen.querySelector(".modes-reset-to-defaults");
  const resetBox = box(reset);
  const under = document.elementFromPoint(canvas.left + resetBox[0] + resetBox[2] / 2, canvas.top + resetBox[1] + resetBox[3] / 2);
  const track = screen.querySelector(".modes-cycle-track");
  return {
    title: style(screen.querySelector("header > span")),
    info: { box: box(info), ...style(info) },
    confirm: box(screen.querySelector("header button")),
    description: { ...style(screen.querySelector(":scope > p")), box: box(screen.querySelector(":scope > p")) },
    tiles: [...screen.querySelectorAll(".modes-cycle button")].map((button) => ({ label: button.textContent, box: box(button), background: getComputedStyle(button).backgroundColor, ...style(button) })),
    label: style(screen.querySelector(".modes-cycle > span")),
    track: { box: box(track), pointerEvents: getComputedStyle(track).pointerEvents, stroke: getComputedStyle(track.querySelector(".modes-cycle-line")).stroke, strokeWidth: getComputedStyle(track.querySelector(".modes-cycle-line")).strokeWidth, arrow: getComputedStyle(track.querySelector(".modes-cycle-arrow")).fill },
    reset: { ...style(reset), box: resetBox, ariaHidden: reset.getAttribute("aria-hidden"), tag: reset.tagName, role: reset.getAttribute("role"), tabindex: reset.getAttribute("tabindex"), underControl: under ? under.closest("button, a, [role=button], [tabindex]:not(.coros-modes-configuration)") !== null : false, underTag: under ? under.tagName : null },
    interactive: [...screen.querySelectorAll("button, a, [role=button], [tabindex]")].map((element) => element.className || element.getAttribute("aria-label"))
  };
})()
`;

type Style = { color: string; size: number; weight: string; lineHeight: number; background: string; decoration: string; pointerEvents: string; userSelect: string; cursor: string };
interface Measured {
  title: Style; info: { box: number[] } & Style; confirm: number[];
  description: Style & { box: number[] }; tiles: Array<{ label: string; box: number[]; background: string } & Style>; label: Style;
  track: { box: number[]; pointerEvents: string; stroke: string; strokeWidth: string; arrow: string };
  reset: Style & { box: number[]; ariaHidden: string | null; tag: string; role: string | null; tabindex: string | null; underControl: boolean; underTag: string | null };
  interactive: string[];
}

let measured: Measured | undefined;
let shot: { width: number; height: number; data: Uint8Array } | undefined;
let behaviour: { handlers: string[]; commits: number; refreshes: number; closes: number } | undefined;

const near = (actual: number, expected: number, tolerance: number, what: string) => assert.ok(Math.abs(actual - expected) <= tolerance, `${what}: ${actual} is not within ${tolerance} of ${expected}`);
const BG = [16, 16, 16];
const px = (x: number, y: number) => { const at = (y * shot!.width + x) * 4; return [shot!.data[at], shot!.data[at + 1], shot!.data[at + 2]]; };
const same = (a: number[], b: number[], tolerance = 3) => a.every((value, channel) => Math.abs(value - b[channel]) <= tolerance);
/** The bounding box of every pixel in the region the predicate accepts (inclusive), or undefined. */
function ink(x0: number, y0: number, x1: number, y1: number, accept: (rgb: number[]) => boolean): [number, number, number, number] | undefined {
  let box: [number, number, number, number] | undefined;
  for (let y = y0; y <= y1; y += 1) for (let x = x0; x <= x1; x += 1) {
    if (!accept(px(x, y))) continue;
    box = box ? [Math.min(box[0], x), Math.min(box[1], y), Math.max(box[2], x), Math.max(box[3], y)] : [x, y, x, y];
  }
  return box;
}
const bright = (rgb: number[]) => rgb[0] + rgb[1] + rgb[2] > 330;
const notBackground = (rgb: number[]) => Math.abs(rgb[0] - 16) + Math.abs(rgb[1] - 16) + Math.abs(rgb[2] - 16) > 8;
function expectInk(box: [number, number, number, number] | undefined, expected: [number, number, number, number], toleranceX: number, toleranceY: number, what: string) {
  assert.ok(box, `${what}: nothing is drawn there`);
  near(box[0], expected[0], toleranceX, `${what}, left edge`);
  near(box[2], expected[2], toleranceX, `${what}, right edge`);
  near(box[1], expected[1], toleranceY, `${what}, top edge`);
  near(box[3], expected[3], toleranceY, `${what}, bottom edge`);
}
/** Where the stroke's brightness sits along a row inside [x0, x1): its centre of mass, in pixel-centre units. */
function strokeCentre(y: number, x0: number, x1: number): number {
  let weight = 0, moment = 0;
  for (let x = x0; x < x1; x += 1) { const [r, g, b] = px(x, y); const w = Math.max(0, (r + g + b) / 3 - 16); weight += w; moment += w * x; }
  return moment / weight;
}

test("lay out the live Modes Configuration screen at the unit's size", { skip, timeout: 240_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { module: bundle, css } = await bundleModule<{ html: string; behaviour(): { handlers: string[]; commits: number; refreshes: number; closes: number } }>(work.path, "modes-screen", `
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { act, create } from "react-test-renderer";
import { ModesConfigurationScreen } from ${JSON.stringify(join(uiSource, "modes-configuration-screen.tsx"))};
import { QuadCortexSurface } from ${JSON.stringify(join(uiSource, "quad-cortex-surface.tsx"))};
import "@qc-remote/theme/theme.css";
globalThis.__keep = QuadCortexSurface;
let commits = 0, refreshes = 0, closes = 0;
const workflow = { slots: [1, 0, 2], loading: false, pending: false, refresh() { refreshes += 1; }, commit() { commits += 1; } };
const onClose = () => { closes += 1; };
export const html = renderToStaticMarkup(createElement("div", { className: "qc-screen-fixture-root is-canonical-canvas" }, createElement(ModesConfigurationScreen, { workflow, onClose })));
// Every pointer, mouse, touch, click or drag handler on the Reset to Defaults node and on each element above it is fired with an
// event: nothing may reach the workflow, because the link is a picture of the unit's destructive control, never a control.
export function behaviour() {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  let renderer;
  act(() => { renderer = create(createElement(ModesConfigurationScreen, { workflow, onClose })); });
  const reset = renderer.root.findByProps({ className: "modes-reset-to-defaults" });
  const handlers = [];
  for (let node = reset; node; node = node.parent) {
    for (const [name, value] of Object.entries(node.props)) {
      if (typeof value !== "function" || !/^on(Click|DoubleClick|ContextMenu|Pointer|Mouse|Touch|Drag|Drop)/.test(name)) continue;
      handlers.push(name);
      const event = { type: name, preventDefault() {}, stopPropagation() {}, currentTarget: {}, target: {}, dataTransfer: { getData: () => "", setData() {} } };
      act(() => { value(event); });
    }
  }
  return { handlers, commits, refreshes, closes };
}
`);
    behaviour = bundle.behaviour();
    const pages = join(work.path, "pages");
    mkdirSync(pages, { recursive: true });
    writeFileSync(join(pages, "modes.html"), `<!doctype html><html lang="en"><head><meta charset="utf-8"><style>*{box-sizing:border-box}html,body{margin:0;background:#000}${css}
.qc-screen-fixture-root{position:absolute;left:0;top:0;width:800px;height:480px;container-type:size;overflow:hidden}</style></head><body>${bundle.html}</body></html>`);
    await withBrowser(browser!, work.path, [], async (_page, open) => {
      const page = await open([800, 480], pathToFileURL(join(pages, "modes.html")).href);
      await page.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))");
      measured = await page.evaluate(MEASURE) as Measured;
      shot = decodePng(await page.screenshot());
    });
  } finally {
    work.cleanup();
  }
  assert.equal(shot!.width, 800);
  assert.equal(shot!.height, 480);
  assert.equal(measured!.tiles.length, 3);
});

test("the header: a muted title, a dark green info button with its divider, and the 98x44 confirm plate", { skip }, () => {
  const m = measured!;
  assert.equal(m.title.color, "rgb(148, 150, 148)", "the title is the screen's muted grey, as the unit's");
  expectInk(ink(0, 5, 400, 60, bright), [18, 24, 179, 40], 2, 1, "the title");
  assert.deepEqual(m.info.box.map(Math.round), [610, 8, 66, 44], "the info button");
  assert.equal(m.info.background, "rgb(24, 28, 24)", "the info button is the unit's dark green tint, not the control grey");
  assert.deepEqual(m.confirm.map(Math.round), [694, 8, 98, 44], "the confirm plate");
  assert.ok(same(px(640, 15), [24, 28, 24], 1) && same(px(700, 15), [41, 44, 41], 1), "the two plates' fills as drawn");
  // The divider: 2px wide at x 684..685 and 22px tall at y 19..40, in the info button's own colour.
  for (const x of [684, 685]) for (const y of [19, 30, 40]) assert.ok(same(px(x, y), [24, 28, 24], 1), `the divider covers (${x}, ${y})`);
  for (const [x, y] of [[683, 30], [686, 30], [684, 18], [685, 41]]) assert.ok(same(px(x, y), BG, 1), `the divider stops short of (${x}, ${y})`);
});

test("the two description lines are white, 21px apart, at the unit's size and rows", { skip }, () => {
  const m = measured!;
  assert.equal(m.description.color, "rgb(255, 255, 255)");
  near(m.description.size, 16.2, 0.1, "description size");
  near(m.description.lineHeight, 21, 0.01, "the pitch between the two lines");
  expectInk(ink(0, 80, 800, 103, bright), [224, 87, 575, 101], 2, 1, "first line");
  expectInk(ink(0, 104, 800, 128, bright), [208, 108, 590, 122], 2, 1, "second line");
});

test("the tiles sit on the unit's rows, 170x80 at 194px, with the glyph and the name centred on the tile", { skip }, () => {
  const m = measured!;
  assert.deepEqual(m.tiles.map((tile) => tile.label), ["Scene", "Preset", "Stomp"]);
  assert.deepEqual(m.tiles.map((tile) => tile.box.map(Math.round)), [[121, 194, 170, 80], [315, 194, 170, 80], [509, 194, 170, 80]]);
  for (const tile of m.tiles) {
    assert.equal(tile.background, "rgb(41, 44, 41)");
    assert.equal(tile.color, "rgb(255, 255, 255)");
    near(tile.size, 18, 0.01, `${tile.label} size`);
    assert.equal(tile.pointerEvents, "auto", "the tiles take the pointer");
  }
  // The glyph ink and the name's ink, from the unit's frame (name left edges are the text's own, within two pixels).
  const glyphs: Array<[number, [number, number, number, number]]> = [[121, [137, 222, 160, 245]], [315, [331, 222, 354, 245]], [509, [526, 225, 547, 245]]];
  const names: Array<[number, [number, number, number, number]]> = [[121, [178, 227, 224, 239]], [315, [373, 227, 423, 239]], [509, [566, 227, 616, 243]]];
  glyphs.forEach(([x0, expected], index) => expectInk(ink(x0 + 10, 200, x0 + 42, 270, bright), expected, 1, 1, `${m.tiles[index].label}'s glyph`));
  names.forEach(([x0, expected], index) => expectInk(ink(x0 + 46, 200, x0 + 165, 270, bright), expected, 2, 1, `${m.tiles[index].label}'s name`));
});

test("the track: a 2px #292c29 line round the cycle, broken for the 14px CYCLE label, with an arrowhead on each end", { skip }, () => {
  const m = measured!;
  assert.equal(m.track.pointerEvents, "none", "the track is a picture");
  assert.deepEqual(m.track.box.map(Math.round), [0, 0, 800, 480]);
  assert.equal(m.track.stroke, "rgb(41, 44, 41)");
  assert.equal(m.track.strokeWidth, "2px");
  assert.equal(m.track.arrow, "rgb(41, 44, 41)");
  const LINE = [41, 44, 41];
  // The straight runs: rows 158..159 above the tiles and 234..235 behind them, with nothing on the rows either side.
  for (const x of [200, 300, 600]) {
    for (const y of [158, 159]) assert.ok(same(px(x, y), LINE, 2), `the top line covers (${x}, ${y})`);
    for (const y of [157, 160]) assert.ok(same(px(x, y), BG, 2), `the top line is two rows thick: (${x}, ${y}) is background`);
  }
  for (const x of [100, 303, 497, 700]) {
    for (const y of [234, 235]) assert.ok(same(px(x, y), LINE, 2), `the lower line covers (${x}, ${y}), through the gaps between the tiles and past them`);
    for (const y of [233, 236]) assert.ok(same(px(x, y), BG, 2), `the lower line is two rows thick: (${x}, ${y}) is background`);
  }
  // The top line stops at x 365 and resumes at x 432: the label's break is 66px wide.
  for (const y of [158, 159]) {
    assert.ok(same(px(365, y), LINE, 2) && same(px(432, y), LINE, 2), `the line reaches the break on row ${y}`);
    for (const x of [367, 372, 378, 421, 426, 431]) assert.ok(same(px(x, y), BG, 2), `the break is clear at (${x}, ${y})`);
  }
  // The label: dim like the track, 14px, ink x 380..419 and y 153..162.
  assert.equal(m.label.color, "rgb(41, 44, 41)");
  near(m.label.size, 14, 0.01, "label size");
  expectInk(ink(372, 148, 428, 166, notBackground), [380, 153, 419, 162], 1, 1, "the CYCLE label");
  // The curve at each end follows the unit's: the stroke's centre of mass on rows measured from the frame.
  const LEFT: Array<[number, number]> = [[166, 54.0], [170, 49.2], [175, 45.2], [180, 42.4], [185, 40.4], [188, 39.4], [208, 40.4], [212, 41.8], [217, 44.6], [222, 48.2], [227, 54.0]];
  const RIGHT: Array<[number, number]> = [[166, 743.0], [170, 747.7], [175, 751.7], [180, 754.6], [185, 756.6], [188, 757.6], [208, 756.6], [212, 755.1], [217, 752.4], [222, 748.8], [227, 743.0]];
  // Rows 190..205 carry the arrowheads over the curve, so the stroke is read only on the rows outside them.
  for (const [y, x] of LEFT) near(strokeCentre(y, 26, 115), x, 1.6, `the left end's curve on row ${y}`);
  for (const [y, x] of RIGHT) near(strokeCentre(y, 680, 775), x, 1.6, `the right end's curve on row ${y}`);
  // The arrowheads: the left one points down (a 19px top edge on row 190, its tip at x 39 on row 205), the right one points up.
  for (const [x, y] of [[32, 191], [39, 191], [45, 191], [39, 204], [39, 200], [753, 204], [759, 204], [765, 204], [759, 195]]) assert.ok(same(px(x, y), LINE, 6), `the arrowhead covers (${x}, ${y})`);
  for (const [x, y] of [[28, 191], [50, 191], [33, 200], [45, 200], [747, 204], [770, 204], [752, 195], [766, 195]]) assert.ok(same(px(x, y), BG, 6), `the arrowhead stops short of (${x}, ${y})`);
  assert.ok(same(px(39, 189), LINE, 22) && same(px(39, 207), LINE, 22), "the left end's curve passes the arrowhead's tip");
});

test("Reset to Defaults is drawn as the unit draws it and is only a picture", { skip }, () => {
  const m = measured!;
  assert.equal(m.reset.color, "rgb(148, 150, 148)", "the link's grey");
  assert.equal(m.reset.decoration, "underline");
  near(m.reset.size, 16.25, 0.1, "link size");
  expectInk(ink(0, 420, 300, 479, (rgb) => rgb[0] + rgb[1] + rgb[2] > 150), [24, 440, 148, 454], 1, 1.5, "the link's ink, underline included");
  // View only: a span with no role and no tab stop, aria-hidden, taking no pointer, with nothing interactive beneath it.
  assert.equal(m.reset.tag, "SPAN");
  assert.equal(m.reset.ariaHidden, "true");
  assert.equal(m.reset.role, null);
  assert.equal(m.reset.tabindex, null);
  assert.equal(m.reset.pointerEvents, "none");
  assert.equal(m.reset.userSelect, "none");
  assert.equal(m.reset.underControl, false, `a press on the link lands on ${m.reset.underTag}, which is no control`);
  assert.ok(!m.interactive.some((entry) => /reset to defaults/i.test(entry ?? "")), `no control is named Reset to Defaults: ${m.interactive.join(", ")}`);
  assert.deepEqual(m.interactive.filter((entry) => !/cycle position|Close Modes Configuration|modes-refresh-a11y/.test(entry ?? "")), [], "only the three tiles, the close plate and the hidden refresh button are controls");
});

test("a press on Reset to Defaults reaches neither the workflow nor the unit", { skip }, () => {
  const b = behaviour!;
  assert.equal(b.commits, 0, "no change to the cycle");
  assert.equal(b.refreshes, 0, "no refresh");
  assert.equal(b.closes, 0, "the screen is not closed");
  // The screen's surface forwards a user's touches to the unit only on screens it does not draw itself; Modes is drawn by the app.
  const surface = readFileSync(join(uiSource, "quad-cortex-surface.tsx"), "utf8");
  const forwarding = surface.split("\n").find((line) => line.includes("const forwardsDeviceScreenGestures"));
  assert.ok(forwarding, "the surface decides which screens forward a touch");
  assert.ok(!/modes/i.test(forwarding), `Modes is not a screen whose touches are forwarded to the unit: ${forwarding}`);
});

test("the fixture of the same screen draws the same track and link", { skip }, () => {
  const fixtures = readFileSync(join(uiSource, "coros-screen-fixtures.tsx"), "utf8");
  const start = fixtures.indexOf("function CorOsModesConfiguration");
  const body = fixtures.slice(start, fixtures.indexOf("function CorOsOfficialModes"));
  assert.match(body, /<ModesCycleTrack \/>/);
  assert.match(body, /<ModesResetToDefaults \/>/);
  assert.doesNotMatch(body, /<i \/>/, "the old two-border track is gone");
});
