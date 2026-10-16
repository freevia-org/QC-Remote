import assert from "node:assert/strict";
import test from "node:test";
import { readdirSync } from "node:fs";
import { findBrowser, makeWorkDirectory, withBrowser } from "./support/display-page.ts";

// Blank documents only: no app rendering, screenshots, reference corpus or HW.
const browser = findBrowser();
test("browser helper retires its profile on normal completion and callback failure", { skip: !browser }, async () => {
  const work = makeWorkDirectory("qc-browser-lifecycle-");
  try {
    assert.equal(await withBrowser(browser!, work.path, [], async page => {
      return page([320, 240], "data:text/html,<title>cleanup</title>", "document.title");
    }), "cleanup");
    assert.deepEqual(readdirSync(work.path), []);
    const failure = new Error("deliberate callback failure");
    await assert.rejects(withBrowser(browser!, work.path, [], async () => { throw failure; }), error => error === failure);
    assert.deepEqual(readdirSync(work.path), []);
  } finally { work.cleanup(); }
});

test("browser startup failure retires its partial profile and preserves the spawn error", async () => {
  const work = makeWorkDirectory("qc-browser-startup-");
  try {
    await assert.rejects(withBrowser(`${work.path}/missing-browser`, work.path, [], async () => {
      assert.fail("missing executable must not dispatch a page");
    }), { code: "ENOENT" });
    assert.deepEqual(readdirSync(work.path), []);
  } finally { work.cleanup(); }
});
