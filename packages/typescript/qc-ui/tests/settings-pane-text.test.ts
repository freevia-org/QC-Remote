import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import { bundleModule, findBrowser, makeWorkDirectory, uiSource, withBrowser } from "./support/display-page.ts";
import { decodePng } from "./support/png-decode.ts";

/*
 * The live Settings screen laid out at the unit's own 800x480 in headless Chrome and compared with what the unit
 * draws (corpus 4.1.0 frames settings-system, settings-device and settings-device-*). Every number is read off the
 * unit's frame; ink boxes are first and last columns and rows brighter than a third of the way from the pane's own
 * colour to white.
 *
 *  - A menu row whose pane has no gateway call is drawn like every other row: the unit draws Connection full white
 *    and the app drew it at half opacity.
 *  - The Global Bypass sentence has the unit's power mark after "bypass icon": ink x 391..410, y 166..185.
 *  - The Gig View sentences have the unit's down triangle in them (ink x 355..368, y 148..156 and x 548..561,
 *    y 194..202), where the app printed the words BANK DOWN and lost the second one altogether.
 *  - The Device panes other than Global Bypass and MIDI are laid out in the box their stylesheets position them in.
 *    Without it the copy of all six sat at the top-left of the screen, under the menu.
 *  - Text weight. The unit's glyph stems are the font's own: a one-pixel "i" inks 106,255,106 over a 42 background,
 *    1.60 pixels of coverage. Chrome inks the same glyph 172,255,172 (2.22 pixels) in grey antialiasing, which is
 *    about 17-25 percent more ink over the Brightness pane's body, title and menu text, at identical glyph widths.
 *    The cause is Skia's coverage table for light text, not the CSS: every Settings text node is weight 400 in the
 *    bundled Regular file, and neither the Medium nor the Bold file is fetched, so nothing here is drawn heavier than
 *    the unit's face. No stylesheet property reduces the ink without dimming the cores or depending on the pane's
 *    background (measured: text-rendering, font-smoothing, kerning, a 0.01px stroke and a lighter weight changed
 *    nothing; a background-coloured stroke or a dimmer colour lowered the mass only by eroding or dimming the
 *    strokes). The test pins what is true: the face and weight in use.
 */

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the Settings pane text check" : false;

const CASES = [["system", "brightness"], ["system", "wifi"], ["device", "bypass"], ["device", "gig-access"], ["device", "hold"], ["device", "stomp-bypass"], ["device", "swap"], ["device", "latency"], ["device", "scene-bypass"]] as const;

const MEASURE = `
(async () => {
  await document.fonts.ready;
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const root = document.querySelector(".coros-settings-official");
  const texts = [...root.querySelectorAll("*")].filter((element) => [...element.childNodes].some((node) => node.nodeType === 3 && node.textContent.trim()));
  const nav = [...root.querySelectorAll(":scope > main > nav > button")].map((button) => { const s = getComputedStyle(button); return { text: button.textContent.trim(), opacity: s.opacity, color: s.color }; });
  const para = root.querySelector("p");
  return {
    nav,
    weights: [...new Set(texts.map((element) => getComputedStyle(element).fontWeight))],
    families: [...new Set(texts.map((element) => getComputedStyle(element).fontFamily.split(",")[0].trim()))],
    faces: [...document.fonts].map((face) => ({ family: face.family, weight: face.weight, status: face.status })),
    paragraph: para ? para.textContent.replace(/\\s+/g, " ").trim() : "",
    glyphs: [...root.querySelectorAll(".inline-settings-power svg, .settings-body svg")].length
  };
})()
`;

interface Measured { nav: Array<{ text: string; opacity: string; color: string }>; weights: string[]; families: string[]; faces: Array<{ family: string; weight: string; status: string }>; paragraph: string; glyphs: number }
const measured = new Map<string, Measured>();
const frames = new Map<string, ReturnType<typeof decodePng>>();

/** The first and last ink column and row inside a box: brighter than a third of the way from the box's own median to white. */
function ink(frame: ReturnType<typeof decodePng>, [x0, y0, x1, y1]: readonly [number, number, number, number]) {
  const lumas: number[] = [];
  const at = (x: number, y: number) => { const i = (y * frame.width + x) * 4; return (frame.data[i] + frame.data[i + 1] + frame.data[i + 2]) / 3; };
  for (let y = y0; y < y1; y += 1) for (let x = x0; x < x1; x += 1) lumas.push(at(x, y));
  const background = [...lumas].sort((a, b) => a - b)[lumas.length >> 1];
  let left = Infinity, top = Infinity, right = -Infinity, bottom = -Infinity;
  for (let y = y0; y < y1; y += 1) for (let x = x0; x < x1; x += 1) if ((at(x, y) - background) / (255 - background) > 0.35) { left = Math.min(left, x); right = Math.max(right, x); top = Math.min(top, y); bottom = Math.max(bottom, y); }
  return { left, top, right, bottom };
}
const near = (actual: number, expected: number, tolerance: number, what: string) => assert.ok(Math.abs(actual - expected) <= tolerance, `${what}: ${actual} is not within ${tolerance} of ${expected}`);

test("lay out the live Settings panes at the unit's size", { skip, timeout: 240_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { module: bundle, css } = await bundleModule<{ html: (section: string, row: string) => string }>(work.path, "settings", `
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { CorOsDeviceSettings } from ${JSON.stringify(join(uiSource, "device-settings-screen.tsx"))};
import ${JSON.stringify(join(uiSource, "quad-cortex-surface.tsx"))};
import ${JSON.stringify(join(uiSource, "coros-screen-fixtures.tsx"))};
import "@qc-remote/theme/theme.css";
const noop = () => undefined;
const bypass = { row1: false, row2: false, row3: false, row4: false };
const settings = { stompModeAutoAssign: false, swapTempoTunerAccess: false, gigViewStompAccess: false, dynamicDelayCompensation: true, holdTimingIndex: 3, screenBrightness: 16, ledBrightness: 32, dimmedLedBrightness: 2, globalBypassCab: bypass, globalBypassIr: bypass };
export const html = (section, row) => renderToStaticMarkup(createElement("div", { id: "root" }, createElement("div", { className: "qc-screen-bezel" }, createElement("div", { className: "qc-screen-fixture-root is-canonical-canvas" }, createElement(CorOsDeviceSettings, { onClose: noop, workflow: {
  section, accountRow: "user", systemRow: section === "system" ? row : "wifi", deviceRow: section === "device" ? row : "bypass", supportRow: "about",
  setSection: noop, setAccountRow: noop, setSystemRow: noop, setDeviceRow: noop, setSupportRow: noop, loading: false, announce: noop, identity: undefined, settings, midiThru: undefined, pending: false
} })))));
`);
    const pages = join(work.path, "pages");
    mkdirSync(pages, { recursive: true });
    await withBrowser(browser, work.path, ["--disable-lcd-text"], async (_page, open) => {
      for (const [section, row] of CASES) {
        const file = join(pages, `${section}-${row}.html`);
        writeFileSync(file, `<!doctype html><html lang="en"><head><meta charset="utf-8"><style>html,body{margin:0;background:#000}${css}
.qc-screen-bezel{position:absolute;left:0;top:0;width:800px;height:480px}.qc-screen-fixture-root{position:absolute;left:0;top:0;width:800px;height:480px;container-type:size;overflow:hidden}</style></head><body>${bundle.html(section, row)}</body></html>`);
        const page = await open([800, 480], pathToFileURL(file).href);
        measured.set(`${section}/${row}`, await page.evaluate(MEASURE) as Measured);
        frames.set(`${section}/${row}`, decodePng(await page.screenshot()));
      }
    });
  } finally {
    work.cleanup();
  }
  assert.equal(measured.size, CASES.length);
});

test("an unavailable row is drawn like the unit draws it, not dimmed", { skip }, () => {
  for (const key of ["system/brightness", "system/wifi"]) {
    const { nav } = measured.get(key)!;
    assert.equal(nav[0].text, "Connection");
    assert.deepEqual(nav.map((row) => row.opacity), Array(7).fill("1"), `${key}: every System row is fully opaque`);
    // Selected, a row is the accent green; unselected it shares the other rows' ink.
    if (key === "system/brightness") assert.equal(nav[0].color, nav[1].color, "Connection is inked like Updates");
  }
});

test("Settings text is the bundled Regular face at weight 400, and no heavier face is fetched", { skip }, () => {
  for (const [key, m] of measured) {
    assert.deepEqual(m.weights, ["400"], `${key}: every text node is weight 400`);
    assert.deepEqual(m.families, ["\"QC CorOS IBM Plex Sans\""], `${key}: the bundled family`);
    const used = m.faces.filter((face) => face.status === "loaded").map((face) => face.weight);
    assert.deepEqual(used, ["100 400"], `${key}: only the Regular file is in use (Medium 500-600 and Bold 700-900 stay unloaded)`);
  }
});

test("Global Bypass draws the unit's power mark after 'bypass icon'", { skip }, () => {
  const m = measured.get("device/bypass")!;
  assert.match(m.paragraph, /will have a\s*bypass icon but will not appear bypassed on The Grid\.$/);
  assert.ok(m.glyphs >= 1, "an inline glyph is in the sentence");
  const box = ink(frames.get("device/bypass")!, [389, 160, 414, 192]);
  near(box.left, 391, 1, "mark left");
  near(box.right, 410, 1, "mark right");
  near(box.top, 166, 2, "mark top");
  near(box.bottom, 185, 2, "mark bottom");
});

test("Gig View Footswitch Access draws the unit's down triangle where the words BANK DOWN were", { skip }, () => {
  const m = measured.get("device/gig-access")!;
  assert.doesNotMatch(m.paragraph, /BANK DOWN/);
  assert.equal(m.glyphs, 2, "a triangle in each paragraph");
  const frame = frames.get("device/gig-access")!;
  const first = ink(frame, [350, 140, 372, 164]);
  const second = ink(frame, [544, 186, 563, 210]);
  near(first.left, 355, 1, "first triangle left"); near(first.right, 368, 1, "first triangle right");
  near(first.top, 148, 1, "first triangle top"); near(first.bottom, 156, 1, "first triangle bottom");
  near(second.left, 548, 1, "second triangle left"); near(second.right, 561, 1, "second triangle right");
  near(second.top, 194, 1, "second triangle top"); near(second.bottom, 202, 1, "second triangle bottom");
});

test("the Device panes with two options, a scale or three choices sit where the unit sets them", { skip }, () => {
  // Title and first copy line ink tops on the unit's frame, per pane: [title, copy].
  const unit: Record<string, [number, number]> = { "gig-access": [81, 121], hold: [82, 123], "stomp-bypass": [81, 125], swap: [82, 122], latency: [82, 122], "scene-bypass": [82, 123] };
  for (const [row, [title, copy]] of Object.entries(unit)) {
    const frame = frames.get(`device/${row}`)!;
    near(ink(frame, [286, 76, 640, 106]).top, title, 2, `${row} title top`);
    near(ink(frame, [286, 116, 770, 142]).top, copy, 3, `${row} copy top`);
  }
});
