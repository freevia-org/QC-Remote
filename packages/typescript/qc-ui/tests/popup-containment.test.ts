import assert from "node:assert/strict";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import test from "node:test";
import { androidStyles, bundleModule, findBrowser, hostBody, makeWorkDirectory, uiSource, VIEWPORTS, windowsStyles, withBrowser, type HostName } from "./support/display-page.ts";
import { POPUP_FIXTURES, POPUP_PROPS } from "./support/popup-fixtures.ts";

/*
 * Every popup, menu, drawer, dialog and overlay the display draws must sit inside the display's box and keep
 * the unit's proportions at any window size, device model (skin) and presentation, in both hosts. The real
 * components are laid out in headless Chrome inside each host's own layout and every popup element is compared
 * with the display element's box. A surface prop that opens a popup without a fixture in
 * support/popup-fixtures.tsx fails the registry test, and the source scan fails any stylesheet that sizes
 * something against the window.
 */

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the popup containment check" : false;

type Box = [number, number, number, number];
interface Sample { key: string; fixture: string; viewport: readonly [number, number]; bezel: Box; found: Record<string, Box[]>; fonts: number[] }

const MEASURE = (selectors: string[]) => `
(async () => {
  for (let waited = 0; waited < 5000 && document.querySelector('template[id^="B:"]'); waited += 50) await new Promise((resolve) => setTimeout(resolve, 50));
  const scaleSettled = () => { const box = document.querySelector('.qc-screen-bezel'); const canvas = box.querySelector(':scope > .qc-screen-fixture-root'); return !canvas || Math.abs(new DOMMatrix(getComputedStyle(canvas).transform).a * 800 - box.getBoundingClientRect().width) < 0.01; };
  for (let waited = 0; waited < 5000 && !scaleSettled(); waited += 50) await new Promise((resolve) => setTimeout(resolve, 50));
  await document.fonts.ready;
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const bezel = document.querySelector('.qc-screen-bezel').getBoundingClientRect();
  const found = {};
  for (const selector of ${JSON.stringify(selectors)}) found[selector] = [...document.querySelectorAll(selector)].map((element) => { const r = element.getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; });
  const fonts = [];
  for (const element of document.querySelectorAll(${JSON.stringify(selectors[0])})) for (const node of [element, ...element.querySelectorAll('*')]) if ([...node.childNodes].some((child) => child.nodeType === 3 && child.textContent.trim())) fonts.push(parseFloat(getComputedStyle(node).fontSize));
  return { bezel: [bezel.left, bezel.top, bezel.width, bezel.height], found, fonts };
})()
`;

let samples: Sample[] = [];

test("lay out every popup in both hosts, at four window sizes, every device model and both presentations", { skip, timeout: 600_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { module: bundle, css } = await bundleModule<{ render(fixture: string, skin: number, presentation: string): Promise<string>; skins: number }>(work.path, "popups", `
import { createElement } from "react";
import { prerenderToNodeStream } from "react-dom/static";
import { QuadCortexSurface } from ${JSON.stringify(join(uiSource, "quad-cortex-surface.tsx"))};
import { IDLE_PLACEHOLDER_SNAPSHOT } from ${JSON.stringify(join(uiSource, "index.ts"))};
import { POPUP_PROPS_BY_ID } from ${JSON.stringify(fileURLToPath(new URL("./support/popup-props.tsx", import.meta.url)))};
import { formFactors, skins } from "@qc-remote/form-factors";
import "@qc-remote/theme/theme.css";

export const modelCount = formFactors.length * skins.length;
export const fixtureIds = Object.keys(POPUP_PROPS_BY_ID);
export async function render(fixtureId, model, presentation) {
  const formFactor = formFactors[Math.floor(model / skins.length)];
  const skin = skins[model % skins.length];
  const props = POPUP_PROPS_BY_ID[fixtureId];
  const { prelude } = await prerenderToNodeStream(createElement(QuadCortexSurface, {
    formFactor, skin, snapshot: IDLE_PLACEHOLDER_SNAPSHOT, selectedBlockId: "", presentation, onAction() {}, ...props()
  }));
  const chunks = [];
  for await (const chunk of prelude) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}
`);
    assert.deepEqual([...(bundle as unknown as { fixtureIds: string[] }).fixtureIds].sort(), POPUP_FIXTURES.map((fixture) => fixture.id).sort(), "every registered popup has props, and every set of props is registered");
    const models = (bundle as unknown as { modelCount: number }).modelCount;
    const pages = join(work.path, "pages");
    mkdirSync(pages, { recursive: true });
    writeFileSync(join(pages, "display.css"), css);
    writeFileSync(join(pages, "windows.css"), readFileSync(windowsStyles, "utf8"));
    writeFileSync(join(pages, "android.css"), readFileSync(androidStyles, "utf8"));
    const cases: Array<{ key: string; fixture: string; viewport: readonly [number, number]; url: string; selectors: string[] }> = [];
    for (const fixture of POPUP_FIXTURES) {
      for (let model = 0; model < (fixture.firstModelOnly ? 1 : models); model += 1) {
        for (const host of ["windows", "android"] as HostName[]) {
          const markup = await bundle.render(fixture.id, model, host === "windows" ? "hardware" : "display");
          for (const viewport of VIEWPORTS) {
            const key = `${fixture.id}/${host}/model${model}/${viewport[0]}x${viewport[1]}`;
            const file = join(pages, `${key.replace(/\//g, "_")}.html`);
            writeFileSync(file, `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="display.css"><link rel="stylesheet" href="${host}.css"></head><body>${hostBody(host, markup)}</body></html>`);
            cases.push({ key, fixture: fixture.id, viewport, url: pathToFileURL(file).href, selectors: fixture.selectors });
          }
        }
      }
    }
    samples = await withBrowser(browser, work.path, [], async (page) => {
      const results: Sample[] = [];
      for (const entry of cases) results.push({ key: entry.key, fixture: entry.fixture, viewport: entry.viewport, ...(await page(entry.viewport, entry.url, MEASURE(entry.selectors)) as Pick<Sample, "bezel" | "found" | "fonts">) });
      return results;
    });
  } finally {
    work.cleanup();
  }
  assert.ok(samples.length >= POPUP_FIXTURES.length * 2 * VIEWPORTS.length);
});

/** QC_POPUP_REPORT=1 prints each popup's box as a percentage of the display, per window size, for the audit table. */
test("report", { skip: skip || process.env.QC_POPUP_REPORT !== "1" }, () => {
  const rows: string[] = [];
  for (const fixture of POPUP_FIXTURES) {
    for (const viewport of VIEWPORTS) {
      const sample = samples.find((entry) => entry.fixture === fixture.id && entry.key.includes("/windows/model0/") && entry.viewport === viewport);
      const box = sample?.found[fixture.selectors[0]]?.[0];
      if (!sample || !box) { rows.push(`${fixture.id} ${viewport.join("x")}: not drawn`); continue; }
      const [bx, by, bw, bh] = sample.bezel;
      rows.push(`${fixture.id} ${viewport.join("x")}: x ${((box[0] - bx) / bw * 100).toFixed(1)}% y ${((box[1] - by) / bh * 100).toFixed(1)}% w ${(box[2] / bw * 100).toFixed(1)}% h ${(box[3] / bh * 100).toFixed(1)}%`);
    }
  }
  console.log(["POPUP-REPORT", ...rows].join(String.fromCharCode(10)));
});

const EPSILON = 0.75;

test("every popup element lies inside the display's box", { skip }, () => {
  const failures: string[] = [];
  for (const sample of samples) {
    const [bx, by, bw, bh] = sample.bezel;
    const fixture = POPUP_FIXTURES.find((entry) => entry.id === sample.fixture)!;
    const primary = fixture.selectors[0];
    if (!sample.found[primary]?.length) failures.push(`${sample.key}: ${primary} was not drawn`);
    for (const [selector, boxes] of Object.entries(sample.found)) {
      for (const [x, y, w, h] of boxes) {
        if (x < bx - EPSILON || y < by - EPSILON || x + w > bx + bw + EPSILON || y + h > by + bh + EPSILON) {
          failures.push(`${sample.key}: ${selector} is ${(w / bw * 100).toFixed(1)}% x ${(h / bh * 100).toFixed(1)}% of the display at ${((x - bx) / bw * 100).toFixed(1)}%,${((y - by) / bh * 100).toFixed(1)}%, outside it`);
        }
      }
    }
  }
  assert.deepEqual(failures.slice(0, 20), [], `${failures.length} popup boxes escape the display`);
});

test("every popup keeps the same proportions of the display at every window size, device model and host", { skip }, () => {
  const failures: string[] = [];
  for (const fixture of POPUP_FIXTURES) {
    const rows = samples.filter((sample) => sample.fixture === fixture.id);
    const fractions = (sample: Sample) => fixture.selectors.map((selector) => (sample.found[selector] ?? []).map(([x, y, w, h]) => [(x - sample.bezel[0]) / sample.bezel[2], (y - sample.bezel[1]) / sample.bezel[3], w / sample.bezel[2], h / sample.bezel[3]]));
    const base = fractions(rows[0]);
    for (const sample of rows.slice(1)) {
      const current = fractions(sample);
      current.forEach((boxes, selectorIndex) => {
        if (boxes.length !== base[selectorIndex].length) { failures.push(`${sample.key}: ${fixture.selectors[selectorIndex]} count ${boxes.length} vs ${base[selectorIndex].length}`); return; }
        boxes.forEach((box, boxIndex) => box.forEach((value, part) => {
          if (Math.abs(value - base[selectorIndex][boxIndex][part]) > 0.02) failures.push(`${sample.key}: ${fixture.selectors[selectorIndex]}[${boxIndex}] part ${part} is ${value.toFixed(3)} of the display, ${base[selectorIndex][boxIndex][part].toFixed(3)} in ${rows[0].key}`);
        }));
      });
    }
  }
  assert.deepEqual(failures, [], `${failures.length} popup boxes change proportion`);
});

test("the text in every popup scales with the display, not with the window", { skip }, () => {
  const failures: string[] = [];
  for (const fixture of POPUP_FIXTURES) {
    const rows = samples.filter((sample) => sample.fixture === fixture.id);
    // The display's canvas is 800 px wide whatever its size on screen, so a size in canvas pixels is the proportion.
    const sizes = (sample: Sample) => sample.fonts;
    const base = sizes(rows[0]);
    for (const sample of rows.slice(1)) {
      const current = sizes(sample);
      if (current.length !== base.length) { failures.push(`${sample.key}: ${current.length} text runs, ${base.length} in ${rows[0].key}`); continue; }
      current.forEach((size, index) => { if (Math.abs(size - base[index]) > 0.05) failures.push(`${sample.key}: text run ${index} is ${size.toFixed(1)}px of an 800px display, ${base[index].toFixed(1)}px in ${rows[0].key}`); });
    }
  }
  assert.deepEqual(failures, [], `${failures.length} popup texts change size with the window`);
});

test("popups with a measured geometry match the unit's", { skip }, () => {
  const failures: string[] = [];
  for (const sample of samples) {
    const fixture = POPUP_FIXTURES.find((entry) => entry.id === sample.fixture)!;
    if (!fixture.fraction) continue;
    const box = sample.found[fixture.selectors[0]]?.[0];
    if (!box) continue;
    const [bx, by, bw, bh] = sample.bezel;
    const { width, height, centered, tolerance } = fixture.fraction;
    if (width !== undefined && Math.abs(box[2] / bw - width) > tolerance) failures.push(`${sample.key}: width ${(box[2] / bw).toFixed(3)} of the display, the unit's is ${width}`);
    if (height !== undefined && Math.abs(box[3] / bh - height) > tolerance) failures.push(`${sample.key}: height ${(box[3] / bh).toFixed(3)} of the display, the unit's is ${height}`);
    if (centered && (Math.abs((box[0] + box[2] / 2 - bx) / bw - 0.5) > tolerance || Math.abs((box[1] + box[3] / 2 - by) / bh - 0.5) > tolerance)) failures.push(`${sample.key}: not centred in the display`);
  }
  assert.deepEqual(failures.slice(0, 20), [], `${failures.length} popups differ from the unit`);
});

// ------------------------------------------------------------------------------------------ no browser needed

test("every surface prop that opens a popup has a fixture or a stated reason", () => {
  const source = readFileSync(join(uiSource, "quad-cortex-surface.tsx"), "utf8");
  const body = source.slice(source.indexOf("export interface QcHostSurfaceProps"), source.indexOf("export type QuadCortexSurfaceProps"));
  const declared = new Set([...body.matchAll(/^\s{2}([A-Za-z]+)\??:/gm)].map((match) => match[1]));
  for (const prop of POPUP_PROPS) assert.ok(declared.has(prop), `${prop} is no longer a surface prop: update POPUP_PROPS`);
  const fixtureProps = new Set(POPUP_FIXTURES.map((fixture) => fixture.propKey));
  for (const prop of POPUP_PROPS) assert.ok(fixtureProps.has(prop), `${prop} opens a popup but has no fixture in popup-fixtures.tsx`);
  // A prop that looks like a popup (a picker, editor, directory, overlay, dialog or panel) must be registered above.
  const looksLikePopup = [...declared].filter((name) => /(Picker|Editor|Directory|Monitor|Command|Assignment|Dialog|Overlay|Drawer|Popup|Menu|Panel|savePreset)$/i.test(name) || name === "popup");
  for (const name of looksLikePopup) assert.ok((POPUP_PROPS as readonly string[]).includes(name), `${name} looks like a popup but is not in POPUP_PROPS`);
  const ids = POPUP_FIXTURES.map((fixture) => fixture.id);
  assert.equal(new Set(ids).size, ids.length);
});

const POPUP_RULE = /^[^{]*(save-|route|dropdown|screen-menu|context-menu|scene-command|scene-label|device-browser|cpu|tooltip|keyboard|directory|dimmer|backdrop|popover|drawer|toast|dialog|overlay|popup)[^{]*\{/;

test("no stylesheet sizes or places anything against the window", () => {
  const offenders: string[] = [];
  const check = (name: string, css: string) => {
    for (const [index, line] of css.split("\n").entries()) {
      const withoutComments = line.replace(/\/\*.*?\*\//g, "");
      if (/position:\s*fixed/.test(withoutComments) && !/:focus-visible/.test(withoutComments)) offenders.push(`${name}:${index + 1}: position: fixed`);
      const viewportUnit = /[0-9.]\s*(vw|vh|vmin|vmax|dvh|dvw|svh|svw|lvh|lvw)\b/.test(withoutComments);
      // surface-shell.css draws the chassis around the display, which does scale with the window; its popup rules are inside the display and may not.
      const chassisRule = /^(surface-shell|display-frame)\.css$/.test(name) && !POPUP_RULE.test(withoutComments);
      if (viewportUnit && !chassisRule) offenders.push(`${name}:${index + 1}: viewport unit`);
    }
  };
  for (const file of readdirSync(uiSource).filter((name) => name.endsWith(".css"))) check(file, readFileSync(join(uiSource, file), "utf8"));
  assert.deepEqual(offenders, []);
});

test("every full-screen device screen fills the display's own canvas, never the window", () => {
  const css = readdirSync(uiSource).filter((name) => name.endsWith(".css")).map((name) => readFileSync(join(uiSource, name), "utf8")).join("\n");
  // The screens the unit takes the whole display for (Tempo, MIDI Out, CPU, Gig View, I/O, Global EQ, Modes, Looper, Tuner, Settings, Directory, Save, Expression).
  for (const root of [".coros-tempo", ".coros-midi-out", ".coros-gig-view", ".coros-io-settings", ".coros-global-eq", ".coros-modes-configuration", ".coros-looper", ".coros-tuner", ".coros-save-as", ".directory-official", ".expression-bypass-official", ".coros-settings-official"]) {
    const rule = css.split("}").find((block) => block.includes(root) && /position:\s*absolute/.test(block) && /inset:\s*0\b/.test(block.split("{")[1] ?? ""));
    assert.ok(rule, `${root} is not placed with position: absolute; inset: 0 inside the display's canvas`);
  }
});
