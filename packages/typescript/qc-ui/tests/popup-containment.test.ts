import assert from "node:assert/strict";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import test from "node:test";
import { androidStyles, bundleModule, findBrowser, hostBody, makeWorkDirectory, uiSource, VIEWPORTS, windowsStyles, withBrowser, type HostName } from "./support/display-page.ts";
import { POPUP_FIXTURES, POPUP_PROPS, type CloseControl } from "./support/popup-fixtures.ts";
import { QC_UNIT_MIRROR_KINDS, QC_UNIT_SCREENS } from "../src/qc-unit-screens.ts";

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
interface Hit { label: string; inView: boolean; ok: boolean; topmost: string | null }
interface Behind { inView: boolean; overlay: boolean; topmost: string | null }
interface Close { label: string; scrim: boolean; declared: number | null; found: boolean; inView: boolean; ok: boolean; topmost: string | null; leaks: string | null }
interface Sample { key: string; fixture: string; viewport: readonly [number, number]; bezel: Box; found: Record<string, Box[]>; fonts: number[]; hits: Hit[]; behind: Behind; closes: Close[] }

const MEASURE = (selectors: string[], declared: CloseControl[]) => `
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
  // A control in a popup must be what the pointer reaches at its own centre: a Grid control stacked above it takes the click instead.
  const hits = [];
  for (const element of document.querySelectorAll(${JSON.stringify(selectors[0])} + ' button')) {
    const r = element.getBoundingClientRect();
    const cx = r.left + r.width / 2, cy = r.top + r.height / 2;
    const top = document.elementFromPoint(cx, cy);
    hits.push({ inView: cx >= 0 && cy >= 0 && cx < innerWidth && cy < innerHeight, label: element.getAttribute('aria-label') || element.textContent.trim(), ok: Boolean(top) && (top === element || element.contains(top)), topmost: top ? (top.getAttribute('aria-label') || top.className || top.tagName) : null });
  }
  // What a click reaches on the Grid outside the popup, at the display's left edge halfway down: a modal popup must take it.
  const bx = bezel.left + bezel.width * 0.04, by = bezel.top + bezel.height * 0.5;
  const under = document.elementFromPoint(bx, by);
  const behind = { inView: bx >= 0 && by >= 0 && bx < innerWidth && by < innerHeight, overlay: Boolean(under && under.closest('.scene-command-overlay')), topmost: under ? (under.getAttribute('aria-label') || under.className || under.tagName) : null };
  // Every app-drawn way to dismiss what is open: a control (hit at its centre) or a scrim (hit at the points it covers). The registry names
  // the ones it expects; every other close, cancel, done or scrim element in the rendering is found by its own markup and held to the same test.
  const name = (element) => element.getAttribute('aria-label') || element.textContent.trim() || String(element.className.baseVal ?? element.className).split(' ')[0];
  const topName = (top) => top ? (top.getAttribute('aria-label') || String(top.className.baseVal ?? top.className).split(' ')[0] || top.tagName) : null;
  const SCRIM = '[class*="scrim"], [class*="dimmer"], [class*="backdrop"], [class*="dismiss"], [class*="-wash"]';
  const CLOSE = 'button[aria-label^="Close"], button[aria-label="Done"], button[aria-label="Cancel"]';
  const screen = document.querySelector('.qc-screen-bezel');
  const grid = (element) => Boolean(element && element.closest('.coros-vector-actions'));
  const inside = (cx, cy) => cx >= 0 && cy >= 0 && cx < innerWidth && cy < innerHeight;
  const measureClose = (element, declaredIndex) => {
    const isScrim = element.matches(SCRIM);
    const r = element.getBoundingClientRect();
    const box = isScrim ? [Math.max(r.left, bezel.left), Math.max(r.top, bezel.top), Math.min(r.right, bezel.right), Math.min(r.bottom, bezel.bottom)] : [r.left, r.top, r.right, r.bottom];
    const points = [];
    if (isScrim) for (let i = 0; i < 7; i++) for (let j = 0; j < 7; j++) points.push([box[0] + (box[2] - box[0]) * (i + 0.5) / 7, box[1] + (box[3] - box[1]) * (j + 0.5) / 7]);
    else points.push([(box[0] + box[2]) / 2, (box[1] + box[3]) / 2]);
    const visible = points.filter(([x, y]) => inside(x, y));
    const tops = visible.map(([x, y]) => document.elementFromPoint(x, y));
    const reached = (top) => Boolean(top) && (top === element || element.contains(top));
    const leaking = isScrim ? tops.find(grid) : undefined;
    // A control is hit at its centre; a scrim only has to be what a click reaches somewhere (a panel covers part of it) and must never let one through to the Grid.
    return { label: name(element), scrim: isScrim, declared: declaredIndex, found: true, inView: visible.length > 0,
      ok: isScrim ? tops.some(reached) : reached(tops[0]), topmost: isScrim ? null : topName(tops[0]), leaks: leaking ? topName(leaking) : null };
  };
  const closes = [];
  const seen = new Set();
  const declared = ${JSON.stringify(declared)};
  declared.forEach((control, index) => {
    const matches = [...screen.querySelectorAll(control.selector)].filter((element) => control.text === undefined || element.textContent.trim() === control.text);
    if (!matches.length) closes.push({ label: control.text ?? control.selector, scrim: Boolean(control.scrim), declared: index, found: false, inView: false, ok: false, topmost: null, leaks: null });
    for (const element of matches) { seen.add(element); closes.push(measureClose(element, index)); }
  });
  for (const element of screen.querySelectorAll(CLOSE + ', ' + SCRIM + ', button')) {
    if (seen.has(element) || element.closest('.coros-vector-actions')) continue;
    // A scrim that draws a wash but takes no pointer input (pointer-events: none) dismisses nothing.
    if (element.matches(SCRIM) && getComputedStyle(element).pointerEvents === 'none') continue;
    if (!element.matches(CLOSE + ', ' + SCRIM) && !/^(cancel|done)$/i.test(element.textContent.trim())) continue;
    seen.add(element);
    closes.push(measureClose(element, null));
  }
  return { bezel: [bezel.left, bezel.top, bezel.width, bezel.height], found, fonts, hits, behind, closes };
})()
`;

let samples: Sample[] = [];

test("lay out every popup in both hosts, at four window sizes, every device model and both presentations", { skip, timeout: 600_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { module: bundle, css } = await bundleModule<{ render(fixture: string, skin: number, presentation: string): Promise<string>; renderScenario(host: string, scenario: object): Promise<string>; skins: number }>(work.path, "popups", `
import { createElement } from "react";
import { prerenderToNodeStream } from "react-dom/static";
import { QuadCortexSurface } from ${JSON.stringify(join(uiSource, "quad-cortex-surface.tsx"))};
import { IDLE_PLACEHOLDER_SNAPSHOT } from ${JSON.stringify(join(uiSource, "index.ts"))};
import { POPUP_PROPS_BY_ID } from ${JSON.stringify(fileURLToPath(new URL("./support/popup-props.tsx", import.meta.url)))};
import { formFactors, skins } from "@qc-remote/form-factors";
import { render as renderHost } from ${JSON.stringify(fileURLToPath(new URL("./support/display-host-entry.tsx", import.meta.url)))};
import "@qc-remote/theme/theme.css";

export const renderScenario = (host, scenario) => renderHost(host, scenario);
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
    assert.deepEqual([...(bundle as unknown as { fixtureIds: string[] }).fixtureIds].sort(), POPUP_FIXTURES.filter((fixture) => !fixture.scenario).map((fixture) => fixture.id).sort(), "every registered popup has props, and every set of props is registered");
    const models = (bundle as unknown as { modelCount: number }).modelCount;
    const pages = join(work.path, "pages");
    mkdirSync(pages, { recursive: true });
    writeFileSync(join(pages, "display.css"), css);
    writeFileSync(join(pages, "windows.css"), readFileSync(windowsStyles, "utf8"));
    writeFileSync(join(pages, "android.css"), readFileSync(androidStyles, "utf8"));
    const cases: Array<{ key: string; fixture: string; viewport: readonly [number, number]; url: string; selectors: string[]; declared: CloseControl[] }> = [];
    for (const fixture of POPUP_FIXTURES) {
      for (let model = 0; model < (fixture.firstModelOnly ? 1 : models); model += 1) {
        for (const host of ["windows", "android"] as HostName[]) {
          const presentation = host === "windows" ? "hardware" : "display";
          const markup = fixture.scenario ? await bundle.renderScenario(host, { ...fixture.scenario, presentation }) : await bundle.render(fixture.id, model, presentation);
          for (const viewport of VIEWPORTS) {
            const key = `${fixture.id}/${host}/model${model}/${viewport[0]}x${viewport[1]}`;
            const file = join(pages, `${key.replace(/\//g, "_")}.html`);
            writeFileSync(file, `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="display.css"><link rel="stylesheet" href="${host}.css"></head><body>${hostBody(host, markup)}</body></html>`);
            cases.push({ key, fixture: fixture.id, viewport, url: pathToFileURL(file).href, selectors: fixture.selectors, declared: fixture.closeControls ?? [] });
          }
        }
      }
    }
    samples = await withBrowser(browser, work.path, [], async (page) => {
      const results: Sample[] = [];
      for (const entry of cases) results.push({ key: entry.key, fixture: entry.fixture, viewport: entry.viewport, ...(await page(entry.viewport, entry.url, MEASURE(entry.selectors, entry.declared)) as Pick<Sample, "bezel" | "found" | "fonts" | "hits" | "behind" | "closes">) });
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
    if (fixture.closeOnly) continue;
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

test("the CPU Monitor's close control is what a click reaches: no Grid control sits above it", { skip }, () => {
  const cpu = samples.filter((sample) => sample.fixture === "cpu-monitor");
  assert.ok(cpu.length >= 2 * VIEWPORTS.length, "the CPU Monitor was laid out in both hosts at every window size");
  const failures: string[] = [];
  let checked = 0;
  for (const sample of cpu) {
    const close = sample.hits.find((hit) => hit.label === "Close CPU Monitor");
    if (!close) failures.push(`${sample.key}: the close control was not drawn`);
    // A narrow window can scroll the display out of view, where nothing can be hit at all.
    else if (!close.inView) continue;
    else if (checked++ >= 0 && !close.ok) failures.push(`${sample.key}: a click on the close control reaches "${close.topmost}" instead`);
  }
  assert.ok(checked >= cpu.length / 2, `the close control was in view in ${checked} of ${cpu.length} layouts`);
  assert.deepEqual(failures.slice(0, 10), [], `${failures.length} layouts bury the close control under the Grid`);
});

test("every control that closes or cancels a popup, dialog, overlay or screen is what a click reaches at its centre, and no scrim lets a click through to the Grid", { skip }, () => {
  const failures: string[] = [];
  const perFixture = new Map<string, { checked: number; layouts: number }>();
  for (const sample of samples) {
    const fixture = POPUP_FIXTURES.find((entry) => entry.id === sample.fixture)!;
    const tally = perFixture.get(fixture.id) ?? { checked: 0, layouts: 0 };
    perFixture.set(fixture.id, tally);
    tally.layouts += 1;
    let inViewHere = false;
    for (const close of sample.closes) {
      if (close.declared !== null && !close.found) { failures.push(`${sample.key}: the registry names ${close.scrim ? "the scrim" : "the control"} ${JSON.stringify(close.label)} but it was not drawn`); continue; }
      // A narrow window can scroll the display out of view, where nothing can be hit at all.
      if (!close.inView) continue;
      inViewHere = true;
      if (!close.ok) failures.push(`${sample.key}: a click on ${close.scrim ? "the scrim" : "the control"} ${JSON.stringify(close.label)} reaches ${close.topmost === null ? "something else" : JSON.stringify(close.topmost)} instead`);
      if (close.leaks) failures.push(`${sample.key}: a click on the scrim ${JSON.stringify(close.label)} reaches the Grid control ${JSON.stringify(close.leaks)} behind it`);
    }
    if (inViewHere) tally.checked += 1;
    if (!fixture.closedBy && !sample.closes.length) failures.push(`${sample.key}: nothing in the rendering closes or cancels it; name why in closedBy, or draw one`);
  }
  for (const [id, { checked, layouts }] of perFixture) {
    if (POPUP_FIXTURES.find((entry) => entry.id === id)!.closedBy) continue;
    assert.ok(checked >= layouts / 2, `${id}: its close controls were in view in ${checked} of ${layouts} layouts`);
  }
  // One line per distinct control, not per layout: the layout is in the first failure's key.
  const distinct = new Map<string, string>();
  for (const failure of failures) distinct.set(failure.replace(/^[^:]*\/(windows|android)\/model\d+\/\d+x\d+: /, ""), failure);
  assert.deepEqual([...distinct.values()].slice(0, 30), [], `${failures.length} close or cancel controls are buried in ${distinct.size} distinct ways`);
});

test("every popup names how it is closed: its controls and scrims in the registry, or the unit that closes it", () => {
  for (const fixture of POPUP_FIXTURES) {
    const named = fixture.closeControls?.length ?? 0;
    assert.ok(named > 0 || fixture.closedBy, `${fixture.id} lists no close control and gives no closedBy reason`);
    assert.ok(!(named > 0 && fixture.closedBy), `${fixture.id} lists close controls and also says it is closed by something else`);
  }
});

test("every popup keeps the same proportions of the display at every window size, device model and host", { skip }, () => {
  const failures: string[] = [];
  for (const fixture of POPUP_FIXTURES.filter((entry) => !entry.closeOnly)) {
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
  for (const fixture of POPUP_FIXTURES.filter((entry) => !entry.closeOnly)) {
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

test("the unit's captured picture, and the picture inside it, are exactly the display's box at every window size and host", { skip }, () => {
  // Every captured-picture fallback (see the registry test below) draws this one element. Its picture once took the 20.5% first
  // row of the shared `.qc-screen` grid for its height, so the unit's frame showed as a strip with the rest of the display black.
  const rows = samples.filter((sample) => sample.fixture === "unit-mirror-picture");
  assert.ok(rows.length >= 2 * VIEWPORTS.length, "the picture was laid out in both hosts at every window size");
  const failures: string[] = [];
  for (const sample of rows) {
    const [bx, by, bw, bh] = sample.bezel;
    for (const selector of [".coros-unit-mirror", ".coros-unit-mirror > img"]) {
      const box = sample.found[selector]?.[0];
      if (!box) { failures.push(`${sample.key}: ${selector} was not drawn`); continue; }
      const [x, y, w, h] = box;
      if (Math.abs(x - bx) > EPSILON || Math.abs(y - by) > EPSILON || Math.abs(w - bw) > EPSILON || Math.abs(h - bh) > EPSILON) failures.push(`${sample.key}: ${selector} is ${w.toFixed(1)}x${h.toFixed(1)} at ${(x - bx).toFixed(1)},${(y - by).toFixed(1)}; the display is ${bw.toFixed(1)}x${bh.toFixed(1)}`);
    }
  }
  assert.deepEqual(failures, []);
});

test("the Copy/Swap Scene panel is the unit's dialog: only CANCEL is there to click, it is what a click reaches, and the Grid behind cannot be", { skip }, () => {
  const failures: string[] = [];
  let checked = 0;
  for (const id of ["copy-scene", "swap-scene"]) {
    const panel = samples.filter((sample) => sample.fixture === id);
    assert.ok(panel.length >= 2 * VIEWPORTS.length, `${id} was laid out in both hosts at every window size`);
    for (const sample of panel) {
      assert.deepEqual(sample.hits.map((hit) => hit.label), ["CANCEL"], `${sample.key}: the panel's only control is CANCEL (no destination to pick)`);
      // A narrow window can scroll the display out of view, where nothing can be hit at all.
      const cancel = sample.hits[0];
      if (!cancel.inView) continue;
      checked += 1;
      if (!cancel.ok) failures.push(`${sample.key}: a click on CANCEL reaches "${cancel.topmost}" instead`);
      if (sample.behind.inView && !sample.behind.overlay) failures.push(`${sample.key}: a click beside the panel reaches "${sample.behind.topmost}" on the Grid behind it`);
    }
  }
  assert.ok(checked >= 2 * 2 * VIEWPORTS.length / 2, `CANCEL was in view in ${checked} layouts`);
  assert.deepEqual(failures.slice(0, 10), [], `${failures.length} layouts leave the Grid reachable or bury CANCEL`);
  // Gig View's Copy/Swap tile has no unit dialog behind it and keeps its own destination list.
  assert.ok(samples.filter((sample) => sample.fixture === "scene-destination-picker").every((sample) => sample.hits.length > 1), "the Gig tile's panel still lists destinations");
});

// ------------------------------------------------------------------------------------------ no browser needed

test("every route that falls back to the captured picture draws the one measured picture element", () => {
  // The unit screens the app has no screen of its own for are listed once (QC_UNIT_MIRROR_KINDS); each of them, and the block editor
  // the unit opened, follows into the "unit-mirror" view, which draws QcUnitScreenMirror and nothing else.
  const mirrored = Object.entries(QC_UNIT_SCREENS).filter(([, entry]) => /unit mirror|unit-mirror/i.test(entry.app)).map(([kind]) => kind).sort();
  const listed = [...QC_UNIT_MIRROR_KINDS, "parameter-editor"].sort();
  assert.deepEqual(mirrored, listed, "a screen the table says is mirrored as a picture must be in QC_UNIT_MIRROR_KINDS, and the reverse");
  // The Copy/Swap Scene dialog is drawn as the app's own panel, not as a picture (a title that cannot be read is the only exception, in the follow hook).
  assert.ok(!(QC_UNIT_MIRROR_KINDS as readonly string[]).includes("copy-scene"), "copy-scene is not in the picture-fallback table");
  assert.doesNotMatch(QC_UNIT_SCREENS["copy-scene"].app, /unit mirror|unit-mirror/i);
  const content = readFileSync(join(uiSource, "qc-device-screen-content.tsx"), "utf8");
  assert.equal([...content.matchAll(/QcUnitScreenMirror/g)].length, 2, "the picture is drawn in one place");
  const follow = readFileSync(join(uiSource, "use-qc-screen-follow.ts"), "utf8");
  assert.match(follow, /QC_UNIT_MIRROR_KINDS\.includes\(kind\)\) setView\("unit-mirror"\)/);
  const mirror = readFileSync(join(uiSource, "qc-unit-screen-mirror.tsx"), "utf8");
  assert.equal([...mirror.matchAll(/<img /g)].length, 1, "one captured-picture element");
  assert.match(mirror, /unit-mirror\.css/);
});

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
