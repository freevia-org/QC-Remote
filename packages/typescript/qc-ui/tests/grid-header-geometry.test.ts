import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import { GRID_TITLE_RIGHT_EDGE, MODE_GLYPH_CENTRE, MODE_LABEL, modeGlyphInkRise, PRESET_BANK_BASELINE, PRESET_BANK_SIZE, PRESET_BANK_X, PRESET_TITLE_GUTTER, PRESET_TITLE_SIZES, presetTitleLayout, presetTitlePresentation, SCENE_BADGE } from "../src/coros-ui.ts";
import { findBrowser, makeWorkDirectory, uiSource, windowsStyles, withBrowser } from "./support/display-page.ts";

/*
 * The Grid header as the unit draws it, measured on a one-digit-bank frame (800x480, SCENE mode) and on grid-base
 * (a three-character bank, STOMP mode). A render of the unit's own strings at these sizes and origins differs from
 * its pixels by under three levels on average; the app's old header differed by about forty:
 *   bank and slot   one run, 70px IBM Plex Bold, x 12, baseline 77, untracked (the unit's 7 at ink 16, F at 60)
 *   name            starts 15px after that run; a size from the ladder 70, 63, 50, 45, 40 - the largest rung not above
 *                   the integer that fits the edge near x 590; baseline 69 at 50, 68 at 45, 66 at 40
 *   scene badge     24 yellow columns and rows from (656,12), a 23px letter on x 668 with its baseline at 33
 *   mode label      22px, x 692, baseline 80 (caps 16 rows tall)
 *   SCENE glyph     ink rows 60..83, so one row lower than the box-centred STOMP glyph
 */

// Advances in px of IBM Plex Bold at 70px, measured in Chrome. The layout scales them linearly with the size.
const ADVANCE_70 = { bankOneDigit: 82.95, bankThreeChar: 134.68 };

test("the Grid title takes the largest rung of the unit's size ladder that fits, and starts 15px after the bank", () => {
  assert.equal(PRESET_BANK_SIZE, 70);
  assert.equal(PRESET_TITLE_GUTTER, 15);
  assert.deepEqual([...PRESET_TITLE_SIZES].slice(0, 5), [70, 63, 50, 45, 40]);
  // Every unit frame measured so far: [bank advance, name advance at 70px, size on the unit, baseline on the unit (0 where it was not measured)]. The
  // sizes come from ink widths on 800x480 frames; the names are not recorded, only their advances.
  const FRAMES = [
    [90.79, 306.32, 70, 0],
    [ADVANCE_70.bankOneDigit, 443.73, 70, 0],
    [90.79, 516.67, 63, 0],
    [84.49, 519.05, 63, 0],
    [ADVANCE_70.bankOneDigit, 599.2, 50, 69],
    [ADVANCE_70.bankOneDigit + 5.7, 618.73, 50, 69],
    [84.49, 684.04, 45, 68],
    [ADVANCE_70.bankThreeChar, 712.32, 40, 66]
  ] as const;
  for (const [bank, name, size, baseline] of FRAMES) {
    const layout = presetTitleLayout(bank, name);
    assert.equal(layout.fontSize, size, `size for a ${name.toFixed(1)}px name after a ${bank.toFixed(1)}px bank`);
    if (baseline) assert.equal(layout.baseline, baseline, `baseline at size ${size}`);
    assert.equal(layout.squeeze, false);
    assert.ok(layout.start + name * size / 70 <= GRID_TITLE_RIGHT_EDGE + 1e-6, `${size}px fits`);
  }
  // The unit's name origin on three frames: 110 after "7F", 111 after "7E", 162 after "32H".
  assert.ok(Math.abs(presetTitleLayout(ADVANCE_70.bankOneDigit, 599.2).start - 110) <= 1.6);
  assert.ok(Math.abs(presetTitleLayout(84.49, 684.04).start - 111) <= 1.6);
  assert.ok(Math.abs(presetTitleLayout(ADVANCE_70.bankThreeChar, 712.32).start - 162) <= 1.6);
  // A 599px name fits a size in the fifties and is still drawn at 50: the rungs between 50 and 63 do not exist.
  const fifties = presetTitleLayout(ADVANCE_70.bankOneDigit, 599.2);
  assert.ok(fifties.start + 599.2 * 56 / 70 <= GRID_TITLE_RIGHT_EDGE, "56px would fit the edge");
  assert.equal(fifties.fontSize, 50);
});

test("a short name keeps the bank's own size and its baseline, and a very long one squeezes at the minimum size", () => {
  const short = presetTitleLayout(ADVANCE_70.bankOneDigit, 190);
  assert.equal(short.fontSize, PRESET_BANK_SIZE);
  assert.equal(short.baseline, PRESET_BANK_BASELINE);
  // A three-character bank starts the name 52px later than a one-digit one (162 against 110 on the unit).
  assert.ok(Math.abs(presetTitleLayout(ADVANCE_70.bankThreeChar, 190).start - 162) <= 1.6);
  const long = presetTitleLayout(ADVANCE_70.bankThreeChar, 4000);
  assert.equal(long.fontSize, 22);
  assert.equal(long.squeeze, true);
});

test("the displayed title is the name as sent, with the dirty marker, and is not padded for the fit", () => {
  assert.equal(presetTitlePresentation("Alpha", false).text, "Alpha");
  assert.equal(presetTitlePresentation("Alpha ", false).text, "Alpha", "a name's own trailing space draws nothing");
  assert.equal(presetTitlePresentation("Alpha  ", true).text, "Alpha*");
  assert.equal(presetTitlePresentation("  ", false).text, "Unsaved");
  assert.equal(presetTitlePresentation("Alpha*", false).text, "Alpha*");
});

test("the header constants are the unit's", () => {
  assert.deepEqual({ x: PRESET_BANK_X, baseline: PRESET_BANK_BASELINE }, { x: 12, baseline: 77 });
  assert.deepEqual({ ...SCENE_BADGE }, { x: 656, y: 12, size: 24, radius: 3, letterX: 668, letterBaseline: 33, letterSize: 23 });
  assert.deepEqual({ ...MODE_LABEL }, { x: 692, baseline: 80, size: 22 });
  assert.equal(modeGlyphInkRise("SCENE"), 1);
  assert.equal(modeGlyphInkRise("STOMP"), 0);
  assert.equal(modeGlyphInkRise("PRESET"), 0);
  assert.deepEqual({ ...MODE_GLYPH_CENTRE }, { x: 668, y: 72 });
});

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the Grid header render check" : false;

interface Header {
  location: { x: number; y: number; size: number; text: string; tracking: string; transform: string | null };
  title: { x: number; y: number; size: number; text: string; textLength: string | null; advance: number };
  badge: { x: number; y: number; width: number; height: number };
  letter: { x: number; y: number; size: number; text: string };
  mode: { x: number; y: number; size: number; text: string };
  glyph: { transform: string };
  groupTransform: string | null;
}

const PROBE = `(() => {
  const q = (name) => document.querySelector('svg.coros-vector-canvas [data-grid-header="' + name + '"]');
  const num = (element, name) => Number(element.getAttribute(name));
  const location = q("location");
  const title = document.querySelector('svg.coros-vector-canvas text.preset-title');
  const badge = q("scene-badge");
  const letter = q("scene-letter");
  const mode = q("mode-label");
  return {
    location: { x: num(location, "x"), y: num(location, "y"), size: Number(getComputedStyle(location).fontSize.replace("px", "")), text: location.textContent, tracking: getComputedStyle(location.firstChild).letterSpacing, transform: location.getAttribute("transform") },
    title: { x: num(title, "x"), y: num(title, "y"), size: num(title, "font-size"), text: title.textContent, textLength: title.getAttribute("textLength"), advance: title.getComputedTextLength() },
    badge: { x: num(badge, "x"), y: num(badge, "y"), width: num(badge, "width"), height: num(badge, "height") },
    letter: { x: num(letter, "x"), y: num(letter, "y"), size: num(letter, "font-size"), text: letter.textContent },
    mode: { x: num(mode, "x"), y: num(mode, "y"), size: num(mode, "font-size"), text: mode.textContent },
    glyph: { transform: q("mode-glyph").getAttribute("transform") },
    groupTransform: location.parentElement.getAttribute("transform")
  };
})()`;

test("the rendered Grid header sits where the unit's does, for any name, bank and mode", { skip, timeout: 600_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { build } = await import("esbuild");
    const entry = join(work.path, "header.tsx");
    writeFileSync(entry, `
import { createElement } from "react";
import { createRoot } from "react-dom/client";
import { QuadCortexSurface } from ${JSON.stringify(join(uiSource, "quad-cortex-surface.tsx"))};
import { IDLE_PLACEHOLDER_SNAPSHOT } from ${JSON.stringify(join(uiSource, "index.ts"))};
import { formFactors, skins } from "@qc-remote/form-factors";
import "@qc-remote/theme/theme.css";
const formFactor = formFactors[0];
const skin = skins.find((entry) => entry.id === formFactor.defaultSkinId) ?? skins[0];
const root = createRoot(document.getElementById("root"));
window.renderGrid = (partial) => root.render(createElement(QuadCortexSurface, { formFactor, skin, snapshot: { ...IDLE_PLACEHOLDER_SNAPSHOT, presetPosition: 40, ...partial }, selectedBlockId: "", presentation: "hardware", onAction() {} }));
`);
    await build({ entryPoints: [entry], outfile: join(work.path, "header.js"), bundle: true, format: "iife", platform: "browser", jsx: "automatic", loader: { ".ttf": "dataurl", ".svg": "dataurl", ".png": "dataurl", ".woff2": "dataurl" }, logLevel: "error", absWorkingDir: work.path, nodePaths: [join(uiSource, "../../../../node_modules")], define: { "process.env.NODE_ENV": '"production"', "import.meta.url": '"file:///header.js"' } });
    writeFileSync(join(work.path, "windows.css"), readFileSync(windowsStyles, "utf8"));
    const page = join(work.path, "header.html");
    // The script is inlined: a script loaded from file:// reports its errors as an opaque "Script error".
    writeFileSync(page, `<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="header.css"><link rel="stylesheet" href="windows.css"></head><body style="margin:0"><div id="root" style="width:1100px"></div><script>${readFileSync(join(work.path, "header.js"), "utf8").replace(/<\/script/g, "<\\/script")}</script></body></html>`);
    const SETTLE = `(async () => { await document.fonts.load('800 70px "QC CorOS IBM Plex Sans"'); await document.fonts.ready; await new Promise((resolve) => setTimeout(resolve, 250)); })()`;
    await withBrowser(browser!, work.path, [], async (_page, open) => {
      const view = await open([1200, 900], pathToFileURL(page).href, 1);
      const render = async (partial: object): Promise<Header> => {
        await view.evaluate(`window.renderGrid(${JSON.stringify(partial)})`);
        await view.evaluate(SETTLE);
        return await view.evaluate(PROBE) as Header;
      };
      const CASES = [
        { name: "Alpha", location: "7F", mode: "SCENE" },
        { name: "Medium Length Name", location: "7E", mode: "STOMP" },
        { name: "Another Longer Preset Name", location: "32H", mode: "PRESET" },
        { name: "Abcde fgh Abcdefg", location: "7F", mode: "SCENE" },
        { name: "Abcde fgh Abcdefg Ab", location: "32H", mode: "STOMP" }
      ];
      for (const { name, location, mode } of CASES) {
        const where = `${location} ${mode} "${name}"`;
        const header = await render({ presetName: name, presetLocation: location, mode });
        // Bank and slot: one untracked 70px run at (12,77) with no horizontal scaling around it.
        assert.equal(header.groupTransform, null, `${where}: no x-scale or offset around the header`);
        assert.deepEqual([header.location.x, header.location.y, header.location.size], [12, 77, 70], `${where}: bank run`);
        assert.equal(header.location.text, location);
        assert.ok(header.location.tracking === "normal" || header.location.tracking === "0px", `${where}: bank tracking ${header.location.tracking}`);
        // Name: a ladder size, fitted to the edge with the browser's own advance, on the baseline the size implies.
        assert.ok(header.title.size === 22 || (PRESET_TITLE_SIZES as readonly number[]).includes(header.title.size), `${where}: a ladder size, got ${header.title.size}`);
        const edge = GRID_TITLE_RIGHT_EDGE + 0.01;
        if (header.title.size > 22) {
          assert.ok(header.title.x + header.title.advance <= edge, `${where}: ${header.title.size}px ends at ${header.title.x + header.title.advance}, past ${GRID_TITLE_RIGHT_EDGE}`);
          const larger = PRESET_TITLE_SIZES.filter((size) => size > header.title.size).pop();
          assert.ok(larger === undefined || header.title.x + header.title.advance * larger / header.title.size > GRID_TITLE_RIGHT_EDGE - 0.01, `${where}: the next rung up (${larger}) would still fit`);
        }
        assert.equal(header.title.y, Math.ceil(77 - (70 - header.title.size) * 0.375 - 0.5), `${where}: baseline follows the size`);
        assert.equal(header.title.textLength, null, `${where}: no squeeze while an integer size fits`);
        // Scene badge and mode label.
        assert.deepEqual(header.badge, { x: 656, y: 12, width: 24, height: 24 }, `${where}: badge`);
        assert.deepEqual([header.letter.x, header.letter.y, header.letter.size], [668, 33, 23], `${where}: badge letter`);
        assert.deepEqual([header.mode.x, header.mode.y, header.mode.size, header.mode.text], [692, 80, 22, mode], `${where}: mode label`);
      }
      // A name too long for even the smallest size is squeezed into the space instead.
      const squeezed = await render({ presetName: "An exceedingly long preset name that no integer size can fit between the bank and the icons", presetLocation: "32H", mode: "STOMP" });
      assert.equal(squeezed.title.size, 22);
      assert.ok(Number(squeezed.title.textLength) > 0 && squeezed.title.x + Number(squeezed.title.textLength) <= GRID_TITLE_RIGHT_EDGE + 0.01, "squeezed to the edge");
      // The SCENE glyph's ink starts a row lower than the STOMP one's box does.
      const scene = await render({ presetName: "Alpha", presetLocation: "7F", mode: "SCENE" });
      const stomp = await render({ presetName: "Alpha", presetLocation: "7F", mode: "STOMP" });
      assert.equal(scene.glyph.transform, "translate(655 60)");
      assert.equal(stomp.glyph.transform, "translate(655 59)");
      // A name's own trailing space is neither drawn nor counted: the title is the same with and without it.
      const trimmed = await render({ presetName: "Abcde fgh", presetLocation: "8B", mode: "SCENE" });
      const spaced = await render({ presetName: "Abcde fgh ", presetLocation: "8B", mode: "SCENE" });
      assert.equal(spaced.title.text, "Abcde fgh");
      assert.equal(spaced.title.size, trimmed.title.size);
    });
  } finally {
    work.cleanup();
  }
});
