import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { parseCss } from "./support/css-rules.ts";

/*
 * The generic block editor (amp, drive, cab and the other 5 x 2 control grids) fills every control
 * cell with --qc-device-control-surface. The unit draws that cell 41,44,41; the app drew 46,46,46.
 * The fix lives in the one token, so every consumer follows and no host overrides it. The unit
 * screen is RGB565, so the value must also be a colour that survives a 565 round trip.
 */

const root = fileURLToPath(new URL("../../../../", import.meta.url));
const read = (relative: string) => readFileSync(join(root, relative), "utf8").replaceAll("\r\n", "\n");

const UNIT_CELL = [41, 44, 41];
const hex = (rgb: number[]) => `#${rgb.map((v) => v.toString(16).padStart(2, "0")).join("")}`;
const snap565 = ([r, g, b]: number[]) => {
  const r5 = r >> 3, g6 = g >> 2, b5 = b >> 3;
  return [(r5 << 3) | (r5 >> 2), (g6 << 2) | (g6 >> 4), (b5 << 3) | (b5 >> 2)];
};

const themeRules = parseCss(read("packages/typescript/qc-theme/src/theme.css"));
const tokenIn = (name: string) => {
  const values = themeRules.map((rule) => rule.declarations.get(name)).filter((value): value is string => value !== undefined);
  assert.equal(values.length, 1, `${name} is declared exactly once`);
  return values[0];
};

test("the control cell token is the unit's 41,44,41 and sits on the RGB565 ladder", () => {
  assert.equal(tokenIn("--qc-device-control-surface"), hex(UNIT_CELL));
  assert.deepEqual(snap565(UNIT_CELL), UNIT_CELL);
});

test("the token agrees with the screen control surface and the colour table", () => {
  assert.equal(tokenIn("--qc-device-control-surface"), tokenIn("--qc-screen-surface-control"));
  const colors = JSON.parse(read("packages/typescript/qc-theme/src/colors.json"));
  assert.equal(colors.device.controlSurface, hex(UNIT_CELL));
});

test("the editor cells take the token and nothing paints the old grey or overrides the fill", () => {
  const live = parseCss(read("packages/typescript/qc-ui/src/live-surface.css"));
  for (const selector of [".parameter-controls", ".coros-parameter", ".parameter-empty-slot"]) {
    const rule = live.find((candidate) => candidate.selector === selector && candidate.declarations.has("background"));
    assert.ok(rule, `${selector} paints a fill`);
    assert.equal(rule.declarations.get("background"), "var(--qc-device-control-surface)", selector);
  }
  for (const file of ["theme.css", "colors.json"]) {
    assert.ok(!/#2e2e2e/i.test(read(`packages/typescript/qc-theme/src/${file}`)), `${file} no longer carries #2e2e2e`);
  }
});
