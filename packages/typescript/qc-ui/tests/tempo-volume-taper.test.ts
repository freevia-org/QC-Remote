import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { TEMPO_VOLUME_KNOTS, tempoVolumePosition } from "../src/tempo-volume-taper.ts";

const rust = readFileSync(fileURLToPath(new URL("../../../rust/quadcortex/src/commands.rs", import.meta.url)), "utf8").replaceAll("\r\n", "\n");

test("the Tempo volume knots are the Rust gateway's measured table, knot for knot", () => {
  const start = rust.indexOf("pub(crate) const TEMPO_VOLUME_KNOTS");
  assert.ok(start > 0, "the Rust table exists");
  const block = rust.slice(start, rust.indexOf("];", start));
  const knots = [...block.matchAll(/\(\s*(\d+\.\d+),\s*(-?\d+\.\d+)\s*\)/g)].map((match) => [Number(match[1]), Number(match[2])]);
  assert.deepEqual(TEMPO_VOLUME_KNOTS.map((knot) => [...knot]), knots);
});

test("0.0 dB sits at raw 0.613 (1 o'clock on the unit), the floor and the ceiling at the sweep's ends", () => {
  assert.equal(tempoVolumePosition(0), 0.613);
  assert.equal(tempoVolumePosition(-60), 0);
  assert.equal(tempoVolumePosition(9), 1);
  assert.equal(tempoVolumePosition(-100), 0);
  assert.equal(tempoVolumePosition(20), 1);
  // Not the linear -60..+9 position (0.87) a plain min/max dial would draw.
  assert.ok(Math.abs(tempoVolumePosition(0) - 60 / 69) > 0.2);
});

test("the position rises with every dB and interpolates between knots", () => {
  let previous = -1;
  for (let db = -60; db <= 9; db += 0.5) {
    const position = tempoVolumePosition(db);
    assert.ok(position > previous, `${db} dB rises`);
    previous = position;
  }
  assert.ok(Math.abs(tempoVolumePosition(-3.4) - 0.5) < 1e-9);
  assert.ok(Math.abs(tempoVolumePosition(-2.6) - 0.525) < 1e-9);
});
