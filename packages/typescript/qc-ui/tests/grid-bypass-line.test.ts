import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import { bundleModule, findBrowser, hostBody, makeWorkDirectory, uiSource, windowsStyles, androidStyles, withBrowser, type HostName } from "./support/display-page.ts";
import { decodePng } from "./support/png-decode.ts";

/*
 * A bypassed block on the Grid does not interrupt the signal line. Measured on the unit (grid-base, 800x480,
 * "Money for Nothing", third block of row 1 bypassed): the line is two rows, y 146 and 147, 198,195,198 on both,
 * and it is unbroken from x 222 to x 321 - across the 3px of black the tile keeps around itself (236..238 and
 * 303..305), across the tile's own border (239..240 and 301..302, which read 198,195,198 under the line rows), and
 * over the tile's mark, which it crosses at full brightness. An enabled tile, by contrast, stops the line three
 * pixels short of its border (66..68 black before the tile at 69), so that gap stays.
 *
 * The app drew the tile's black surround over the cable and began the bypass line at the tile's own edge, which
 * left the cable cut for the three surround pixels on each side. The test samples the real rendered pixels along
 * the line through every bypassed tile, in several rows, categories, selection, split and hover states.
 */

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the Grid bypass-line check" : false;

// The unit's own measurements, in screen pixels of the 800x480 frame.
const UNIT_LINE = { rows: [146, 147], colour: [198, 195, 198], tile: 64, surround: 3 };
const ROW_Y = [147, 241, 335, 429];

interface Probe { cx: number; y: number; ctm: [number, number, number, number]; d: string; strokeWidth: string; stroke: string; bboxLeft: number; bboxRight: number }
interface Surround { cx: number; y: number; ctm: [number, number, number, number] }

const block = (id: string, category: string, row: number, column: number, bypassed: boolean) => ({ id, name: category, kind: category.toLowerCase(), category, row, column, bypassed });
const routes = (extra: Record<number, object> = {}) => [0, 1, 2, 3].map((row) => ({ row, inputId: 1, outputId: 19, input: "In 1", output: "Multi Out", splitMuted: false, ...(extra[row] ?? {}) }));

const SCENARIOS: Array<{ name: string; blocks: object[]; routes: object[]; selected?: string; hover?: boolean }> = [
  {
    name: "row 1, enabled neighbours, four categories",
    blocks: [block("a", "Amp", 0, 0, false), block("b", "Overdrive", 0, 1, true), block("c", "Cab", 0, 2, false), block("d", "Modulation", 0, 3, true), block("e", "Reverb", 0, 4, true), block("f", "Delay", 0, 5, false), block("g", "Equalizer", 0, 6, true), block("h", "Compressor", 0, 7, true)],
    routes: routes()
  },
  {
    name: "rows 2 to 4",
    blocks: [block("a", "Pitch", 1, 2, true), block("b", "Filter", 2, 3, true), block("c", "Synth", 3, 4, true), block("d", "Wah", 3, 0, false), block("e", "Utility", 1, 6, false)],
    routes: routes()
  },
  { name: "selected", blocks: [block("a", "Overdrive", 0, 2, true), block("b", "Amp", 0, 3, false)], routes: routes(), selected: "a" },
  { name: "hovered", blocks: [block("a", "Overdrive", 0, 2, true), block("b", "Amp", 0, 3, false)], routes: routes(), hover: true },
  {
    name: "row split into a parallel path and rejoined",
    blocks: [block("a", "Overdrive", 0, 2, true), block("b", "Amp", 0, 1, false), block("c", "Modulation", 1, 2, true), block("d", "Cab", 0, 5, true)],
    routes: routes({ 0: { splitColumn: 0, mixColumn: 6 }, 1: {} })
  }
];

const SETTLE = `(async () => {
  for (let waited = 0; waited < 5000 && document.querySelector('template[id^="B:"]'); waited += 50) await new Promise((resolve) => setTimeout(resolve, 50));
  await document.fonts.ready;
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
})()`;

// Every bypass line and every enabled tile's surround, with the matrix that maps its user units to client pixels.
const PROBE = `(() => {
  const ctm = (element) => { const m = element.getScreenCTM(); return [m.a, m.e, m.d, m.f]; };
  const lines = [...document.querySelectorAll('svg.coros-vector-canvas .official-block-bypass-line')].map((line) => {
    const box = line.getBBox();
    return { cx: box.x + box.width / 2, y: box.y, ctm: ctm(line), d: line.getAttribute('d'), strokeWidth: line.getAttribute('stroke-width'), stroke: line.getAttribute('stroke'), bboxLeft: box.x, bboxRight: box.x + box.width };
  });
  const surrounds = [...document.querySelectorAll('svg.coros-vector-canvas .official-block-tile:not(.is-bypassed) .official-block-surround')].map((rect) => {
    const box = rect.getBBox();
    return { cx: box.x + box.width / 2, y: box.y + box.height / 2, ctm: ctm(rect) };
  });
  return { lines, surrounds };
})()`;

const colourAt = (shot: { width: number; data: Uint8Array }, [a, e, d, f]: [number, number, number, number], ratio: number, sx: number, sy: number) => {
  const px = Math.floor((a * sx + e) * ratio);
  const centre = (d * sy + f) * ratio;
  // The line is two screen rows thick on the unit and at least that here, so the row containing its centre and the
  // one above it both lie inside it; take the brighter so a rounding of the centre cannot read black.
  let best = [0, 0, 0];
  for (const py of [Math.floor(centre), Math.floor(centre) - 1]) {
    const at = (py * shot.width + px) * 4;
    const rgb = [shot.data[at], shot.data[at + 1], shot.data[at + 2]];
    if (rgb[0] + rgb[1] + rgb[2] > best[0] + best[1] + best[2]) best = rgb;
  }
  return best;
};

test("a bypassed Grid block lets the signal line run unbroken across it, as on the unit", { skip, timeout: 600_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { module: bundle, css } = await bundleModule<{ render(presentation: string, snapshot: string, selected: string): Promise<string> }>(work.path, "bypassline", `
import { createElement } from "react";
import { prerenderToNodeStream } from "react-dom/static";
import { QuadCortexSurface } from ${JSON.stringify(join(uiSource, "quad-cortex-surface.tsx"))};
import { IDLE_PLACEHOLDER_SNAPSHOT } from ${JSON.stringify(join(uiSource, "index.ts"))};
import { formFactors, skins } from "@qc-remote/form-factors";
import "@qc-remote/theme/theme.css";

const formFactor = formFactors[0];
const skin = skins.find((entry) => entry.id === formFactor.defaultSkinId) ?? skins[0];
export async function render(presentation, snapshotJson, selected) {
  const snapshot = { ...IDLE_PLACEHOLDER_SNAPSHOT, presetLocation: "7F", presetPosition: 40, presetName: "Money for Nothing", mode: "STOMP", ...JSON.parse(snapshotJson) };
  const { prelude } = await prerenderToNodeStream(createElement(QuadCortexSurface, { formFactor, skin, snapshot, selectedBlockId: selected, presentation, onAction() {} }));
  const chunks = [];
  for await (const chunk of prelude) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}
`);
    const pages = join(work.path, "pages");
    mkdirSync(pages, { recursive: true });
    writeFileSync(join(pages, "display.css"), css);
    writeFileSync(join(pages, "windows.css"), readFileSync(windowsStyles, "utf8"));
    writeFileSync(join(pages, "android.css"), readFileSync(androidStyles, "utf8"));
    const files = new Map<string, string>();
    for (const [index, scenario] of SCENARIOS.entries()) {
      for (const host of ["windows", "android"] as const) {
        const file = join(pages, `${host}-${index}.html`);
        const markup = await bundle.render(host === "windows" ? "hardware" : "display", JSON.stringify({ blocks: scenario.blocks, routes: scenario.routes }), scenario.selected ?? "");
        writeFileSync(file, `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="display.css"><link rel="stylesheet" href="${host}.css"></head><body>${hostBody(host as HostName, markup)}</body></html>`);
        files.set(`${host}-${index}`, pathToFileURL(file).href);
      }
    }
    const CASES = [{ host: "windows", viewport: [1440, 820], ratio: 3 }, { host: "windows", viewport: [1920, 1080], ratio: 2 }, { host: "android", viewport: [412, 900], ratio: 4 }] as const;
    await withBrowser(browser!, work.path, [], async (_page, open) => {
      for (const [index, scenario] of SCENARIOS.entries()) {
        for (const { host, viewport, ratio } of CASES) {
          const where = `${scenario.name} / ${host} ${viewport.join("x")} @${ratio}`;
          const page = await open(viewport, files.get(`${host}-${index}`)!, ratio);
          await page.evaluate(SETTLE);
          let probe = await page.evaluate(PROBE) as { lines: Probe[]; surrounds: Surround[] };
          const expected = (scenario.blocks as Array<{ bypassed: boolean }>).filter((entry) => entry.bypassed).length;
          assert.equal(probe.lines.length, expected, `${where}: one bypass line per bypassed block`);
          if (scenario.hover) {
            const [a, e, d, f] = probe.lines[0].ctm;
            await page.move(a * probe.lines[0].cx + e, d * probe.lines[0].y + f);
            await new Promise((resolve) => setTimeout(resolve, 400));
            probe = await page.evaluate(PROBE) as { lines: Probe[]; surrounds: Surround[] };
          }
          const shot = decodePng(await page.screenshot());
          for (const line of probe.lines) {
            const at = `${where}, tile at x ${line.cx}, y ${line.y}`;
            // The unit's geometry: a 2px line of 198,195,198 spanning the tile and its 3px surround, on the row's cable.
            assert.ok(ROW_Y.includes(line.y), `${at}: the line sits on its row's cable`);
            assert.equal(Number(line.strokeWidth), UNIT_LINE.rows.length, `${at}: two pixels thick`);
            assert.equal(line.stroke.toLowerCase(), "#c6c3c6", `${at}: the unit's line colour`);
            assert.ok(line.cx - line.bboxLeft >= UNIT_LINE.tile / 2 + UNIT_LINE.surround && line.bboxRight - line.cx >= UNIT_LINE.tile / 2 + UNIT_LINE.surround, `${at}: the line covers the tile and its black surround (${line.d})`);
            // The rendered pixels: from the cable before the tile to the cable after it, every sample is the line.
            assert.ok(line.ctm[0] * ratio >= 1.4, `${at}: the page draws the 800px screen at ${line.ctm[0] * ratio}x, enough to resolve a two-pixel line`);
            const reference = colourAt(shot, line.ctm, ratio, line.cx - 44, line.y);
            assert.ok(reference.every((value, channel) => Math.abs(value - UNIT_LINE.colour[channel]) <= 30), `${at}: the cable beside the tile reads ${reference} (the unit's line is ${UNIT_LINE.colour})`);
            for (let sx = line.cx - 44; sx <= line.cx + 44; sx += 0.5) {
              const rgb = colourAt(shot, line.ctm, ratio, sx, line.y);
              // The pointer's own hover outline may lighten the line where it crosses; nothing may darken it.
              assert.ok(rgb.every((value, channel) => (scenario.hover ? reference[channel] - value : Math.abs(value - reference[channel])) <= 16), `${at}: the line is interrupted at ${sx - line.cx} from the tile centre, reading ${rgb} instead of ${reference}`);
            }
          }
          // An enabled tile still stops the line three pixels short of its border.
          for (const surround of probe.surrounds) {
            for (const side of [-1, 1]) {
              const rgb = colourAt(shot, surround.ctm, ratio, surround.cx + side * 33.5, surround.y);
              assert.ok(Math.max(...rgb) < 60, `${where}: an enabled tile at x ${surround.cx} keeps its black margin on the line (${rgb})`);
            }
          }
        }
      }
    });
  } finally {
    work.cleanup();
  }
});
