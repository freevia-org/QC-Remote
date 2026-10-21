import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { GRID_ADD_HIT_SIZE, gridAddHitOrigin } from "../src/grid-add-hit-geometry.ts";

const columns = [101, 187, 272, 357, 443, 529, 615, 701];
const rows = [147, 241, 335, 429];

test("empty Grid targets use fixed canvas-sized bounds with no overlaps", () => {
  assert.equal(GRID_ADD_HIT_SIZE, 70);
  const boxes = rows.flatMap((centerY) => columns.map((centerX) => {
    const { left, top } = gridAddHitOrigin(centerX, centerY);
    return { left, top, right: left + GRID_ADD_HIT_SIZE, bottom: top + GRID_ADD_HIT_SIZE };
  }));
  for (const box of boxes) {
    assert.ok(box.left >= 0 && box.top >= 0 && box.right <= 800 && box.bottom <= 480, JSON.stringify(box));
  }
  for (let index = 0; index < boxes.length; index += 1) {
    for (let other = index + 1; other < boxes.length; other += 1) {
      const a = boxes[index]!;
      const b = boxes[other]!;
      assert.ok(a.right <= b.left || b.right <= a.left || a.bottom <= b.top || b.bottom <= a.top, `targets ${index} and ${other} overlap`);
    }
  }
  assert.deepEqual(gridAddHitOrigin(columns[7]!, rows[3]!), { left: 666, top: 394 });
});

test("Add-a-block buttons render the measured canvas bounds without percentage sizing or a center transform", () => {
  const component = readFileSync(new URL("../src/quad-cortex-surface.tsx", import.meta.url), "utf8");
  const css = readFileSync(new URL("../src/surface-shell.css", import.meta.url), "utf8");
  assert.match(component, /const hitOrigin = gridAddHitOrigin\(hitX, hitY\)/);
  assert.match(component, /style=\{\{ left: `\$\{hitOrigin\.left\}px`, top: `\$\{hitOrigin\.top\}px` \}\}/);
  assert.match(css, /\.coros-vector-add-hit \{ width: 70px; height: 70px; border-radius: 12%; \}/);
  assert.doesNotMatch(css, /\.coros-vector-add-hit[^{]*\{[^}]*transform:/);
});
