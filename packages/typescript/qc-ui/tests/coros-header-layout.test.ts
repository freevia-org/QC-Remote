import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";
import { COROS_HEADER_FONT, COROS_HEADER_GLYPHS, COROS_HEADER_KERNS } from "../src/coros-header-glyphs.ts";
import { corOsHeaderOutlineFallback, layoutCorOsHeaderGlyphs } from "../src/coros-header-layout.ts";

test("the catalog is bound to the existing bundled bold font and its original design metrics", () => {
  const font = readFileSync(new URL("../../qc-theme/assets/fonts/IBMPlexSans-Bold.ttf", import.meta.url));
  assert.equal(createHash("sha256").update(font).digest("hex"), COROS_HEADER_FONT.sourceSha256);
  assert.equal(COROS_HEADER_FONT.unitsPerEm, 1000);
  assert.equal(COROS_HEADER_GLYPHS.length, 95);
  // Independent hmtx/outline-bound samples from the bundled TrueType font.
  assert.equal(COROS_HEADER_GLYPHS["7".charCodeAt(0) - 32].advanceUnits, 600);
  assert.deepEqual(COROS_HEADER_GLYPHS["7".charCodeAt(0) - 32].bounds, [55, 0, 549, 698]);
  assert.equal(COROS_HEADER_GLYPHS["F".charCodeAt(0) - 32].advanceUnits, 585);
  assert.equal(COROS_HEADER_GLYPHS["i".charCodeAt(0) - 32].advanceUnits, 286);
  assert.equal(COROS_HEADER_GLYPHS[0].advanceUnits, 236);
  assert.equal(COROS_HEADER_GLYPHS[0].path, "");
  assert.equal(COROS_HEADER_GLYPHS[0].bounds, null);
  assert.equal(COROS_HEADER_KERNS["65,86"], -54, "A/V pair is in font units");
  assert.equal(COROS_HEADER_KERNS["83,84"], -23, "S/T pair is in font units");
  assert.equal(COROS_HEADER_KERNS["84,79"], -25, "T/O pair is in font units");
});

test("70px keeps fractional advances and rounds the running paint pen without cumulative drift", () => {
  const bank = layoutCorOsHeaderGlyphs("7F", 70, 12)!;
  assert.deepEqual(bank.glyphs.map(glyph => glyph.pen), [12, 54]);
  assert.ok(Math.abs(bank.advance - 82.95) < 1e-9);
  assert.equal(bank.advanceMode, "fractional");
  const repeated = layoutCorOsHeaderGlyphs("i".repeat(26), 70)!;
  assert.equal(repeated.glyphs[25].pen, 501, "25 fractional 20.02px advances reach 500.5px");
  assert.ok(Math.abs(repeated.advance - 520.52) < 1e-9);
  const pair = layoutCorOsHeaderGlyphs("AV", 70)!;
  assert.deepEqual(pair.glyphs.map(glyph => glyph.pen), [0, 44]);
  assert.ok(Math.abs(pair.advance - 89.67) < 1e-9);
});

test("the 68px bank-size probe preserves explicit half-pixel tracking", () => {
  const untracked = layoutCorOsHeaderGlyphs("A1", 68, 14)!;
  const tracked = layoutCorOsHeaderGlyphs("A1", 68, 14, undefined, 0.5)!;
  assert.equal(untracked.advanceMode, "fractional");
  assert.equal(tracked.advance - untracked.advance, 1, "explicit tracking follows both glyphs, including the trailing advance");
  assert.ok(tracked.glyphs[1].pen >= untracked.glyphs[1].pen && tracked.glyphs[1].pen - untracked.glyphs[1].pen <= 1,
    "the inter-glyph half pixel changes the rounded paint pen by at most one pixel");
  assert.equal(corOsHeaderOutlineFallback("A1", 68, { enabled: true }), undefined);
});

test("continuous admitted sizes preserve fractional advances and pair positioning", () => {
  const run = layoutCorOsHeaderGlyphs("iii", 50, 10)!;
  assert.deepEqual(run.glyphs.map(glyph => glyph.pen), [10, 24, 39]);
  assert.ok(Math.abs(run.advance - 42.9) < 1e-9);
  assert.equal(run.advanceMode, "fractional");
  assert.deepEqual(layoutCorOsHeaderGlyphs("ST", 22)!.glyphs.map(glyph => glyph.pen), [0, 13]);
  assert.deepEqual(layoutCorOsHeaderGlyphs("TO", 22)!.glyphs.map(glyph => glyph.pen), [0, 12]);
  const dynamic = layoutCorOsHeaderGlyphs("Test Preset", 63.75, 150, 410)!;
  assert.equal(dynamic.advanceMode, "fractional");
  assert.ok(dynamic.fitsMaxWidth);
});

test("outline admission covers the Grid's complete title-size ladder and intermediate sizes", () => {
  const text = "Test Preset (#02) AVATAR 0123456789";
  for (const size of [22, 22.25, 25, 30, 32, 35, 40, 45, 50, 63, 63.75, 68, 70]) {
    const layout = layoutCorOsHeaderGlyphs(text, size, 151.5);
    assert.ok(layout, `${size}px is admitted`);
    assert.equal(layout.advanceMode, "fractional");
    assert.equal(layout.glyphs.length, text.length);
    assert.equal(layout.glyphs[0].pen, 152, `${size}px leading pen rounds consistently`);
    assert.ok(layout.advance > 0);
  }
});

test("width admission uses original outline bounds, including overhang, rather than advance alone", () => {
  const fitting = layoutCorOsHeaderGlyphs("F", 70, 12, 37.8)!;
  assert.equal(fitting.fitsMaxWidth, true);
  assert.ok(Math.abs(fitting.inkBounds!.right - 49.8) < 1e-9);
  assert.equal(layoutCorOsHeaderGlyphs("F", 70, 12, 37.79)!.fitsMaxWidth, false);
  const overhang = layoutCorOsHeaderGlyphs("(", 70, 12, 24)!;
  assert.ok(overhang.advance < 24, "advance-only guard would admit this run");
  assert.equal(overhang.fitsMaxWidth, false, "actual contour crosses the available edge");
  const spaced = layoutCorOsHeaderGlyphs("A A", 50)!;
  assert.deepEqual(spaced.glyphs.map(glyph => glyph.pen), [0, 34, 46]);
  assert.equal(spaced.glyphs[1].path, "");
  assert.equal(spaced.glyphs[2].pen - spaced.glyphs[1].pen, 12, "one internal space keeps its font advance without inventing ink");
  const empty = layoutCorOsHeaderGlyphs("", 50, 0, 0)!;
  assert.equal(empty.advance, 0);
  assert.deepEqual(empty.glyphs, []);
  assert.equal(empty.inkBounds, undefined);
  assert.equal(empty.fitsMaxWidth, true);
});

test("unsupported shaping, sizes and invalid geometry refuse the complete run", () => {
  for (const text of ["AéB", "A\u0301", "A\tB", "A\nB", "🙂"]) {
    assert.equal(corOsHeaderOutlineFallback(text, 50, { enabled: true }), "unsupported-text");
    assert.equal(layoutCorOsHeaderGlyphs(text, 50), undefined);
  }
  assert.deepEqual([...COROS_HEADER_FONT.fallbackLigatureSequences], ["fi"]);
  assert.equal(corOsHeaderOutlineFallback("office", 50, { enabled: true }), "ligature");
  assert.equal(layoutCorOsHeaderGlyphs("office", 50), undefined);
  for (const text of [" A", "A ", "A  B", " ", "  "]) {
    assert.equal(corOsHeaderOutlineFallback(text, 50, { enabled: true }), "whitespace");
    assert.equal(layoutCorOsHeaderGlyphs(text, 50), undefined, "edge/repeated spaces retain whole-run SVG text shaping");
  }
  for (const size of [21.99, 70.01, 71]) assert.equal(layoutCorOsHeaderGlyphs("Public", size), undefined);
  for (const size of [0, -1, NaN, Infinity]) assert.equal(layoutCorOsHeaderGlyphs("Public", size), undefined);
  assert.equal(layoutCorOsHeaderGlyphs("Public", 50, NaN), undefined);
  assert.equal(layoutCorOsHeaderGlyphs("Public", 50, 0, -1), undefined);
  assert.equal(layoutCorOsHeaderGlyphs("Public", 50, 0, Infinity), undefined);
  const options = { enabled: true };
  assert.equal(corOsHeaderOutlineFallback("Public", 50), "disabled");
  assert.equal(corOsHeaderOutlineFallback("Public", 50, { ...options, dirty: true }), "dirty");
  assert.equal(corOsHeaderOutlineFallback("Public", 50, { ...options, transformed: true }), "transformed");
  assert.equal(corOsHeaderOutlineFallback("Public", 50, { ...options, squeezed: true }), "squeezed");
  assert.equal(corOsHeaderOutlineFallback("Public", 50, { ...options, spaced: true }), "spaced");
});
