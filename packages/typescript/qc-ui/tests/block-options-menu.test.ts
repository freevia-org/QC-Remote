import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test from "node:test";
import { androidStyles, bundleModule, findBrowser, hostBody, makeWorkDirectory, uiSource, windowsStyles, withBrowser, type HostName } from "./support/display-page.ts";
import { decodePng } from "./support/png-decode.ts";

/*
 * The block editor's options menu is a layer of the screen, not a dropdown of the editor. The unit (zenUI::_OptionsPopup
 * over zenUI::ParameterEditor) draws a 320 x 420 panel at 32,30 of its 800x480 frame over a screen it has dimmed. The
 * numbers below were measured on the unit's frame (private, not kept) with the menu open over the Amp editor:
 *
 *   panel      x 32..351, y 30..449, radius 8, 16,16,16          seven 60px rows from the panel's top, no dividers
 *   icons      24px glyphs, x 47..71 (a 54px column)             labels start at x 86/87
 *   scrollbar  2px at x 342..343, y 42..317 (276 long), 49,52,49 (ten rows of 60 in a 420px panel)
 *   dim        the Grid black reads 66,65,66 and the editor body (41,44,41 undimmed) 74,77,74, split at y 196
 *
 * Every value is met within a few levels (the unit's screen is RGB565 and its dim a substitution on its own ladders, so
 * an alpha blend fits within a few levels at the points sampled, never exactly), in both hosts, and the box is
 * checked in canvas units at several window sizes.
 */

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the options menu check" : false;

const settle = `(async () => {
  for (let waited = 0; waited < 5000 && document.querySelector('template[id^="B:"]'); waited += 50) await new Promise((resolve) => setTimeout(resolve, 50));
  await document.fonts.ready;
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const root = document.querySelector('.qc-screen-bezel > .qc-screen-fixture-root');
  const r = root.getBoundingClientRect();
  return [r.left, r.top, r.width, r.height];
})()`;

/** Everything the page reports about the layer, in canvas units (800x480), whatever the window size. */
const inspect = `(() => {
  const root = document.querySelector('.qc-screen-bezel > .qc-screen-fixture-root');
  root.scrollIntoView({ block: 'start', inline: 'start' });
  const rr = root.getBoundingClientRect();
  const unit = (r) => [(r.left - rr.left) / rr.width * 800, (r.top - rr.top) / rr.height * 480, r.width / rr.width * 800, r.height / rr.height * 480];
  const one = (selector) => { const e = root.querySelector(selector); return e ? unit(e.getBoundingClientRect()) : null; };
  const at = (x, y) => { const e = document.elementFromPoint(rr.left + x / 800 * rr.width, rr.top + y / 480 * rr.height); return e ? { tag: e.tagName, cls: e.className && e.className.baseVal === undefined ? e.className : "", label: e.getAttribute('aria-label') || "", inScrim: Boolean(e.closest('[data-qc-options-scrim]')), inPanel: Boolean(e.closest('.qc-device-options-panel')), inKebab: Boolean(e.closest('.editor-more, .parameter-more')), text: e.textContent.slice(0, 30) } : null; };
  const rows = [...root.querySelectorAll('.qc-device-options-list > button')].map((b) => { const range = document.createRange(); const t = [...b.childNodes].find((n) => n.nodeType === 3); if (t) range.selectNode(t); return { label: b.textContent, box: unit(b.getBoundingClientRect()), text: t ? unit(range.getBoundingClientRect()) : null, glyph: unit(b.querySelector('svg').getBoundingClientRect()), font: parseFloat(getComputedStyle(b).fontSize) * 800 / rr.width * (rr.width / 800), disabled: b.classList.contains('is-disabled') }; });
  const panel = root.querySelector('.qc-device-options-panel');
  return { panel: one('.qc-device-options-panel'), scrim: one('[data-qc-options-scrim]'), thumb: one('.qc-device-options-thumb'), rows, labels: rows.map((r) => r.label), role: panel && panel.getAttribute('role'),
    editor: one('.coros-parameter-editor'), kebab: one('.editor-more, .parameter-more'),
    viewport: [innerWidth, innerHeight, rr.left, rr.top, rr.width, rr.height], hits: { outside: at(450, 215), overKebab: at(30, 226), besideKebab: at(20, 226), inPanel: at(200, 60), onGrid: at(700, 100), onEditor: at(700, 400) }, expanded: (root.querySelector('.editor-more, .parameter-more') || {}).getAttribute?.('aria-expanded') };
})()`;

type Rgb = readonly [number, number, number];
interface Reading { what: string; at: readonly [number, number]; unit: Rgb; within?: number }
const PANEL: Rgb = [16, 16, 16];

const READINGS: Record<string, Reading[]> = {
  "amp-editor-menu": [
    { what: "panel", at: [200, 100], unit: PANEL, within: 0 },
    { what: "Grid ground, dimmed", at: [700, 100], unit: [66, 65, 66], within: 3 },
    { what: "editor body, dimmed", at: [700, 400], unit: [74, 77, 74], within: 3 }
  ]
};

const FIXTURES = ["amp-editor-menu", "amp-editor-closed", "cab-editor-menu"] as const;

async function build(work: string) {
  const { module: bundle, css } = await bundleModule<{ render(fixture: string, presentation: string): Promise<string> }>(work, "options-menu", `
import { createElement } from "react";
import { prerenderToNodeStream } from "react-dom/static";
import { QuadCortexSurface } from ${JSON.stringify(join(uiSource, "quad-cortex-surface.tsx"))};
import { IDLE_PLACEHOLDER_SNAPSHOT } from ${JSON.stringify(join(uiSource, "index.ts"))};
import { POPUP_PROPS_BY_ID } from ${JSON.stringify(fileURLToPath(new URL("./support/popup-props.tsx", import.meta.url)))};
import { formFactors, skins } from "@qc-remote/form-factors";
import "@qc-remote/theme/theme.css";

const WHICH = { "amp-editor-menu": ["amp-editor-panel", true], "amp-editor-closed": ["amp-editor-panel", false], "cab-editor-menu": ["cab-editor-full-screen", true] };
export async function render(fixtureId, presentation) {
  const formFactor = formFactors[0];
  const skin = skins.find((entry) => entry.id === formFactor.defaultSkinId) ?? skins[0];
  const [source, open] = WHICH[fixtureId];
  const props = POPUP_PROPS_BY_ID[source]();
  const { prelude } = await prerenderToNodeStream(createElement(QuadCortexSurface, {
    formFactor, skin, snapshot: IDLE_PLACEHOLDER_SNAPSHOT, selectedBlockId: "", presentation, onAction() {}, ...props,
    parameterEditor: { ...props.parameterEditor, initialMenuOpen: open, contextActionEnabled: {} }
  }));
  const chunks = [];
  for await (const chunk of prelude) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}
`);
  const pages = join(work, "pages");
  mkdirSync(pages, { recursive: true });
  writeFileSync(join(pages, "display.css"), css);
  writeFileSync(join(pages, "windows.css"), readFileSync(windowsStyles, "utf8"));
  writeFileSync(join(pages, "android.css"), readFileSync(androidStyles, "utf8"));
  const urls = new Map<string, string>();
  for (const fixture of FIXTURES) {
    for (const host of ["windows", "android"] as HostName[]) {
      const markup = await bundle.render(fixture, host === "windows" ? "hardware" : "display");
      const file = join(pages, `${fixture}-${host}.html`);
      writeFileSync(file, `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="display.css"><link rel="stylesheet" href="${host}.css"></head><body>${hostBody(host, markup)}</body></html>`);
      urls.set(`${fixture}-${host}`, pathToFileURL(file).href);
    }
  }
  return urls;
}

test("the options menu is a screen layer with the unit's panel, dim and rows", { skip, timeout: 600_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const urls = await build(work.path);
    const failures: string[] = [];
    await withBrowser(browser, work.path, [], async (_page, open) => {
      for (const host of ["windows", "android"] as HostName[]) {
        for (const fixture of Object.keys(READINGS)) {
          const page = await open([2182, 1286], urls.get(`${fixture}-${host}`)!, 1);
          const [left, top, width, height] = await page.evaluate(settle) as number[];
          const shot = await page.screenshot();
          if (process.env.QC_OPTIONS_DUMP) writeFileSync(join(process.env.QC_OPTIONS_DUMP, `${fixture}-${host}.png`), shot);
          const picture = decodePng(shot);
          for (const reading of READINGS[fixture]) {
            const x = Math.round(left + reading.at[0] / 800 * width), y = Math.round(top + reading.at[1] / 480 * height);
            const seen = [0, 1, 2].map((k) => picture.data[(y * picture.width + x) * 4 + k]);
            const worst = Math.max(...seen.map((value, k) => Math.abs(value - reading.unit[k])));
            if (worst > (reading.within ?? 2)) failures.push(`${fixture}-${host}: ${reading.what} at ${reading.at.join(",")} reads ${seen.join(",")}; the unit reads ${reading.unit.join(",")}`);
          }
        }
      }
    });
    assert.deepEqual(failures, []);
  } finally { work.cleanup(); }
});

const near = (actual: number[] | null, expected: number[], tolerance: number, what: string, failures: string[]) => {
  if (!actual || actual.some((value, index) => Math.abs(value - expected[index]) > tolerance)) failures.push(`${what}: ${actual ? actual.map((v) => v.toFixed(1)).join(",") : "missing"}; the unit has ${expected.join(",")} (+-${tolerance})`);
};

test("the menu's box, rows, scrollbar and hit targets are the unit's in canvas units at every window size", { skip, timeout: 600_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const urls = await build(work.path);
    const failures: string[] = [];
    await withBrowser(browser, work.path, [], async (_page, open) => {
      for (const host of ["windows", "android"] as HostName[]) {
        for (const viewport of [[1440, 820], [920, 640], [390, 844], [844, 390], [2182, 1286]] as const) {
          const tag = `${host} ${viewport.join("x")}`;
          const page = await open(viewport, urls.get(`amp-editor-menu-${host}`)!, 1);
          await page.evaluate(settle);
          const got = await page.evaluate(inspect) as any;
          const tol = 0.75;
          near(got.panel, [32, 30, 320, 420], tol, `${tag}: panel box`, failures);
          near(got.scrim, [0, 0, 800, 480], tol, `${tag}: scrim covers the screen`, failures);
          near(got.thumb, [342, 42, 2, 276], 1.5, `${tag}: scrollbar thumb`, failures);
          if (got.role !== "menu") failures.push(`${tag}: panel role ${got.role}`);
          const expected = ["Change device", "Copy device", "Paste device", "Reset to defaults", "Save Current Parameters as...", "Assign expression pedal", "Mute/bypass", "Model update available", "Change to legacy version", "Remove block from the grid"];
          if (JSON.stringify(got.labels) !== JSON.stringify(expected)) failures.push(`${tag}: rows ${JSON.stringify(got.labels)}`);
          got.rows.slice(0, 7).forEach((row: any, index: number) => {
            near(row.box, [32, 30 + index * 60, 320, 60], tol, `${tag}: row ${index} box`, failures);
            near(row.glyph, [47, 30 + index * 60 + 18, 24, 24], tol + 1, `${tag}: row ${index} glyph`, failures);
            if (!row.text || Math.abs(row.text[0] - 86) > 1.5) failures.push(`${tag}: row ${index} label starts at ${row.text?.[0]}; the unit starts it at 86/87`);
            if (Math.abs(row.font - 16) > 0.2) failures.push(`${tag}: row ${index} font ${row.font}`);
          });
          if (!got.rows[2].disabled || got.rows.slice(0, 7).some((row: any, index: number) => index !== 2 && row.disabled)) failures.push(`${tag}: only Paste device is dim among the seven visible rows`);
          // A tap beside the panel lands on the scrim (it closes the menu, as the unit's empty-area tap does), a tap in the panel on a row,
          // and the kebab (at 30,226; the panel's left edge is at 32, so only a sliver of it stays outside the panel) is covered by the scrim while the menu is open.
          if (got.hits.outside && !got.hits.outside.inScrim) failures.push(`${tag}: (450,215) is ${JSON.stringify(got.hits.outside)}, not the scrim`);
          if (!got.hits.besideKebab?.inScrim) failures.push(`${tag}: (20,226) is ${JSON.stringify(got.hits.besideKebab)}, not the scrim`);
          // (A point the Windows host's phone-width window pushes past its viewport cannot be hit-tested there; the other sizes cover it.)
          if ((got.hits.onGrid && !got.hits.onGrid.inScrim) || (got.hits.onEditor && !got.hits.onEditor.inScrim)) failures.push(`${tag}: the Grid and the editor are not both under the scrim`);
          if (!got.hits.inPanel?.inPanel) failures.push(`${tag}: (200,60) is ${JSON.stringify(got.hits.inPanel)}, not a row`);
          // A full-screen editor (no Grid above it) gets the same panel and a dim over the whole screen; its own colours are not measured.
          const cab = await open(viewport, urls.get(`cab-editor-menu-${host}`)!, 1);
          await cab.evaluate(settle);
          const full = await cab.evaluate(inspect) as any;
          near(full.panel, [32, 30, 320, 420], tol, `${tag}: full-screen editor's panel box`, failures);
          if (!await cab.evaluate(`document.querySelector('[data-qc-options-scrim]').classList.contains('is-covering')`)) failures.push(`${tag}: a full-screen editor's dim covers only its lower part`);
          const closed = await open(viewport, urls.get(`amp-editor-closed-${host}`)!, 1);
          await closed.evaluate(settle);
          const shut = await closed.evaluate(inspect) as any;
          if (shut.panel || shut.scrim) failures.push(`${tag}: a closed menu draws a panel or a scrim`);
          if (!shut.hits.overKebab?.inKebab) failures.push(`${tag}: with the menu closed (30,226) is ${JSON.stringify(shut.hits.overKebab)}, not the kebab`);
        }
      }
    });
    assert.deepEqual(failures, []);
  } finally { work.cleanup(); }
});
