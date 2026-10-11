import assert from "node:assert/strict";
import test from "node:test";
import { samePresetAddress, trimTrailingSlashes } from "../src/index.ts";

test("trimTrailingSlashes removes only trailing slashes", () => {
  assert.equal(trimTrailingSlashes(""), "");
  assert.equal(trimTrailingSlashes("/"), "");
  assert.equal(trimTrailingSlashes("////"), "");
  assert.equal(trimTrailingSlashes("/user/setlist/Live"), "/user/setlist/Live");
  assert.equal(trimTrailingSlashes("/user/setlist/Live/"), "/user/setlist/Live");
  assert.equal(trimTrailingSlashes("/user/setlist/Live///"), "/user/setlist/Live");
  assert.equal(trimTrailingSlashes("//a//b//"), "//a//b");
  assert.equal(trimTrailingSlashes("/a /"), "/a ");
});

test("trimTrailingSlashes agrees with the regular expression it replaces", () => {
  for (const value of ["", "/", "a", "a/", "/a", "//", "a//b/", "a/b//", "/ /", "a\n/", "/\n"]) {
    assert.equal(trimTrailingSlashes(value), value.replace(/\/+$/, ""), JSON.stringify(value));
  }
});

test("trimTrailingSlashes stays linear on a very long run of slashes", () => {
  const started = performance.now();
  // The old quadratic replace needed about ten seconds for this input.
  assert.equal(trimTrailingSlashes("/".repeat(100_000) + "x"), "/".repeat(100_000) + "x");
  assert.equal(trimTrailingSlashes("x" + "/".repeat(100_000)), "x");
  assert.equal(trimTrailingSlashes("/".repeat(100_000)), "");
  assert.ok(performance.now() - started < 500, "100k slashes must trim in well under half a second");
});

test("samePresetAddress ignores trailing slashes and compares the slot", () => {
  const a = { setlistKey: "/user/setlist/Live/", presetPosition: 3 };
  assert.equal(samePresetAddress(a, { setlistKey: "/user/setlist/Live", presetPosition: 3 }), true);
  assert.equal(samePresetAddress(a, { setlistKey: "/user/setlist/Live", presetPosition: 4 }), false);
  assert.equal(samePresetAddress(a, { setlistKey: "/user/setlist/Other", presetPosition: 3 }), false);
  const long = { setlistKey: "/".repeat(100_000) + "x", presetPosition: 1 };
  const started = performance.now();
  assert.equal(samePresetAddress(long, { ...long }), true);
  assert.ok(performance.now() - started < 500);
});
