import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { bundleTestModule } from "./support/display-page.ts";

const uiSource = fileURLToPath(new URL("../src/", import.meta.url));

interface Fixture {
  render(props: Record<string, unknown>): string;
  layout(text: string, size: number, x?: number, maxWidth?: number, letterSpacing?: number): {
    glyphs: { index: number; path: string; pen: number }[];
    advance: number;
    inkBounds?: { left: number; right: number };
    fitsMaxWidth: boolean;
  } | undefined;
  font: { sourceSha256: string; unitsPerEm: number };
}

const { module: fixture } = await bundleTestModule<Fixture>("qc-header-outline-", "header", `
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { CorOsHeaderOutlineText } from ${JSON.stringify(join(uiSource, "coros-header-outline-text.tsx"))};
import { layoutCorOsHeaderGlyphs } from ${JSON.stringify(join(uiSource, "coros-header-layout.ts"))};
import { COROS_HEADER_FONT } from ${JSON.stringify(join(uiSource, "coros-header-glyphs.ts"))};
export const render = props => renderToStaticMarkup(createElement(CorOsHeaderOutlineText, props));
export const layout = layoutCorOsHeaderGlyphs;
export const font = COROS_HEADER_FONT;
`, cleanup => test.after(cleanup));

const base = { runs: [{ text: "A A" }], x: 12, y: 77, fontSize: 50, enabled: true };
const transforms = (markup: string) => [...markup.matchAll(/<path\b[^>]*transform="([^"]+)"/g)].map(match => match[1]);

test("outline data stays bound to the bundled font, with independently read hmtx advances", () => {
  const font = readFileSync(join(uiSource, "../../qc-theme/assets/fonts/IBMPlexSans-Bold.ttf"));
  assert.equal(createHash("sha256").update(font).digest("hex"), fixture.font.sourceSha256);
  assert.equal(fixture.font.unitsPerEm, 1000);
  // Independent TTFont/hmtx inspection: A=685 units and space=236 units.
  // At 50px whole-pixel advances are 34 and 12; at 70px retain 47.95 and 16.52.
  assert.deepEqual(fixture.layout("A A", 50, 12)?.glyphs.map(glyph => glyph.pen), [12, 46, 58]);
  assert.deepEqual(fixture.layout("A A", 70, 12)?.glyphs.map(glyph => glyph.pen), [12, 60, 76]);
  assert.equal(transforms(fixture.render(base)).length, 2, "space advances without painted outline");
});

test("letter-spaced bank headings retain the original SVG text behavior", () => {
  const markup = fixture.render({ ...base, runs: [{ text: "A1" }], fontSize: 68, letterSpacing: 0.5 });
  assert.match(markup, /data-outline-mode="text"/);
  assert.match(markup, /data-outline-fallback="spaced"/);
  assert.match(markup, /letter-spacing="0.5"/);
  const styled = fixture.render({ ...base, runs: [{ text: "A1" }], fontSize: 68, style: { letterSpacing: "0.5px" } });
  assert.match(styled, /data-outline-mode="text"/);
  assert.match(styled, /data-outline-fallback="spaced"/);
  assert.match(styled, /style="[^"]*letter-spacing:0\.5px/);
});

test("all Grid preset-title sizes paint paths inside a 400px width cap", () => {
  const text = "Test Preset";
  for (const fontSize of [70, 63, 50, 45, 40, 35, 30, 25, 22]) {
    const markup = fixture.render({ ...base, runs: [{ text }], x: 164, y: 75, fontSize, maxWidth: 400, "aria-hidden": true });
    assert.match(markup, /data-outline-mode="paths"/, `${fontSize}px uses outlines`);
    assert.match(markup, /data-outline-fallback="none"/, `${fontSize}px has no fallback`);
    assert.equal((markup.match(/<path\b/g) ?? []).length, text.replaceAll(" ", "").length, `${fontSize}px paints each non-space glyph`);
    assert.doesNotMatch(markup, /role="img"|aria-label=/, "the live Grid's outer SVG owns the accessible name");
  }
});

test("color runs preserve pair placement and provide a single accessible name", () => {
  const whole = fixture.render({ ...base, runs: [{ text: "AV", fill: "white" }] });
  const split = fixture.render({ ...base, runs: [{ text: "A", fill: "white" }, { text: "V", fill: "gold" }] });
  assert.deepEqual(transforms(split), transforms(whole), "color boundary cannot reset pen or drop kerning");
  assert.equal((split.match(/role="img"/g) ?? []).length, 1);
  assert.equal((split.match(/aria-label="AV"/g) ?? []).length, 1);
  assert.equal((split.match(/<path\b[^>]*aria-hidden="true"/g) ?? []).length, 2);
  assert.match(split, /fill="gold"/);
  const nested = fixture.render({ ...base, "aria-hidden": true, "data-grid-header": "location" });
  assert.doesNotMatch(nested, /role=|aria-label=/, "outer device SVG already supplies the complete accessible label");
  assert.match(nested, /data-grid-header="location"/);
});

test("unsupported shaping, style and fitting keep the entire original text run", () => {
  const cases: [Record<string, unknown>, string][] = [
    [{ enabled: false }, "disabled"],
    [{ runs: [{ text: "A\u0301" }] }, "unsupported-text"],
    [{ runs: [{ text: "A\u{1f3b8}" }] }, "unsupported-text"],
    [{ runs: [{ text: "fi" }] }, "ligature"],
    [{ runs: [{ text: " A" }] }, "whitespace"],
    [{ runs: [{ text: "A " }] }, "whitespace"],
    [{ runs: [{ text: "A  A" }] }, "whitespace"],
    [{ fontSize: 71 }, "unsupported-size"],
    [{ dirty: true }, "dirty"],
    [{ transform: "skewX(-12)" }, "transformed"],
    [{ textLength: 40, lengthAdjust: "spacingAndGlyphs" }, "squeezed"],
    [{ letterSpacing: "0.1em" }, "spaced"],
    [{ style: { wordSpacing: "2px" } }, "spaced"],
    [{ onClick: () => {} }, "unsupported-style"],
    [{ style: { fontFamily: "serif" } }, "unsupported-style"],
    [{ style: { fontSize: "10px" } }, "unsupported-style"],
    [{ style: { fontWeight: 400 } }, "unsupported-style"],
    [{ fontFamily: "serif" }, "unsupported-style"],
    [{ dominantBaseline: "middle" }, "unsupported-style"],
    [{ textAnchor: "middle" }, "unsupported-style"],
  ];
  for (const [props, reason] of cases) {
    const markup = fixture.render({ ...base, className: "preset-title", ...props });
    assert.match(markup, /^<text\b/);
    assert.match(markup, new RegExp(`data-outline-fallback="${reason}"`));
    assert.doesNotMatch(markup, /<path\b/);
    assert.match(markup, /class="preset-title"/);
  }
});

test("available width guards exact ink overhang rather than a guessed advance", () => {
  const layout = fixture.layout("A", 50, 12);
  assert.ok(layout?.inkBounds);
  const width = layout.inkBounds.right - 12;
  assert.match(fixture.render({ ...base, runs: [{ text: "A" }], maxWidth: width }), /data-outline-mode="paths"/);
  assert.match(fixture.render({ ...base, runs: [{ text: "A" }], maxWidth: width - 0.01 }), /data-outline-fallback="outline-overflow"/);
  assert.match(fixture.render({ ...base, maxWidth: -1 }), /data-outline-fallback="invalid-geometry"/);
});
