import assert from "node:assert/strict";
import test from "node:test";
import { findBrowser, makeWorkDirectory, measureLayouts, renderDisplays, SCREENS, VIEWPORTS, type DisplayMeasurement } from "./support/display-page.ts";

/*
 * Lays the real shared display out in headless Chrome, inside the layout each
 * host puts around it (the Windows workspace, the Android phone shell), at the
 * four viewports the hosts are used at. No app, dev server or device is
 * involved: this is QuadCortexSurface's own markup and every stylesheet it
 * loads, plus each host's own stylesheet.
 *
 * It needs a Chrome or Chromium. QC_BROWSER_EXECUTABLE names one; the stock
 * install paths are tried otherwise. CI always has one, so there a missing
 * browser fails the test instead of skipping it.
 */

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the display layout check" : false;

const ASPECT = 5 / 3;
const aspectTolerance = 0.005;

let measured: DisplayMeasurement[] = [];
let clamped: DisplayMeasurement[] = [];

test("lay out the shared display in both hosts", { skip, timeout: 240_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { css, markup } = await renderDisplays(work.path);
    measured = await measureLayouts(browser, work.path, css, markup);
    clamped = await measureLayouts(browser, work.path, css, markup, ["--blink-settings=minimumFontSize=8"]);
  } finally {
    work.cleanup();
  }
  assert.equal(measured.length, 2 * VIEWPORTS.length * SCREENS.length);
  assert.equal(clamped.length, measured.length);
});

test("the display box is 5:3 within 0.5% at every viewport, in both hosts", { skip }, () => {
  for (const sample of measured) {
    const [, , width, height] = sample.bezel;
    const aspect = width / height;
    assert.ok(Math.abs(aspect / ASPECT - 1) <= aspectTolerance, `${sample.key}: display box is ${width.toFixed(2)}x${height.toFixed(2)} (${aspect.toFixed(4)}), not 5:3`);
    assert.deepEqual(sample.border, ["0px", "0px"], `${sample.key}: the display box has no border or padding of its own`);
  }
});

test("the canvas scale is one factor on both axes and fills the display box exactly", { skip }, () => {
  for (const sample of measured) {
    const [a, b, c, d] = sample.matrix;
    assert.equal(b, 0);
    assert.equal(c, 0);
    assert.ok(Math.abs(a - d) < 1e-9, `${sample.key}: scale is ${a} on x and ${d} on y`);
    assert.ok(Math.abs(a * 800 - sample.bezel[2]) < 0.01, `${sample.key}: the canvas is 800 * ${a} wide, the display box ${sample.bezel[2]}`);
    for (let index = 0; index < 4; index += 1) assert.ok(Math.abs(sample.root[index] - sample.bezel[index]) < 0.6, `${sample.key}: the canvas covers the display box (${sample.root} vs ${sample.bezel})`);
    if (sample.svg) for (let index = 0; index < 4; index += 1) assert.ok(Math.abs(sample.svg[index] - sample.bezel[index]) < 0.6, `${sample.key}: the Grid's SVG fills the canvas undistorted`);
  }
});

test("the same screen has the same shape and scale in every host at a given display width", { skip }, () => {
  for (const screen of SCREENS) {
    const samples = measured.filter((sample) => sample.key.endsWith(`-${screen}`));
    for (const sample of samples) {
      const reference = samples.find((other) => other !== sample && Math.abs(other.bezel[2] - sample.bezel[2]) < 0.01);
      if (!reference) continue;
      assert.ok(Math.abs(sample.matrix[0] - reference.matrix[0]) < 1e-9, `${sample.key} vs ${reference.key}`);
    }
  }
  // Every text box, normalised by the display's own width, is a function of the screen alone.
  for (const screen of SCREENS) {
    const samples = measured.filter((sample) => sample.key.endsWith(`-${screen}`));
    const base = samples[0];
    for (const sample of samples.slice(1)) {
      assert.equal(sample.texts.length, base.texts.length, `${sample.key}: same text runs as ${base.key}`);
      sample.texts.forEach((text, index) => {
        const norm = (s: DisplayMeasurement, t: DisplayMeasurement["texts"][number]) => t.box.map((v, k) => (k < 2 ? v - s.bezel[k] : v) / s.bezel[2] * 800);
        const a = norm(sample, text);
        const b = norm(base, base.texts[index]);
        a.forEach((value, k) => assert.ok(Math.abs(value - b[k]) < 0.5, `${sample.key} vs ${base.key}: "${text.text}" box[${k}] ${value.toFixed(2)} vs ${b[k].toFixed(2)} canvas px`));
      });
    }
  }
});

test("a minimum font size changes no layout or text box in the display", { skip }, () => {
  for (const sample of measured) {
    const other = clamped.find((candidate) => candidate.key === sample.key);
    assert.ok(other, sample.key);
    assert.deepEqual(other.bezel, sample.bezel, `${sample.key}: display box`);
    assert.deepEqual(other.root, sample.root, `${sample.key}: canvas`);
    assert.equal(other.texts.length, sample.texts.length, `${sample.key}: text runs`);
    sample.texts.forEach((text, index) => {
      const clampedText = other.texts[index];
      assert.equal(clampedText.fontSize, text.fontSize, `${sample.key}: "${text.text}" font size`);
      text.box.forEach((value, k) => assert.ok(Math.abs(value - clampedText.box[k]) < 0.01, `${sample.key}: "${text.text}" box[${k}] ${value} -> ${clampedText.box[k]}`));
    });
  }
});

test("no display text is nominally smaller than a WebView's 8px minimum", { skip }, () => {
  const small = new Set<string>();
  for (const sample of measured) for (const text of sample.texts) if (text.fontSize < 8 && text.fontSize > 0) small.add(`${text.text} (${text.fontSize}px)`);
  assert.deepEqual([...small], []);
});
