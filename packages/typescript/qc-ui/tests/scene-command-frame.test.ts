import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import { findBrowser, makeWorkDirectory, uiSource, withBrowser } from "./support/display-page.ts";
import { titleFrame } from "./support/scene-command-frame.ts";
import { greyPngBase64 } from "./support/unit-png.ts";

/*
 * The hosts are web views: the unit's frame is decoded by the browser's own PNG decoder, then read. This runs the real
 * reader, bundled the way the app bundles it, on real PNGs in headless Chrome.
 */
const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the frame decoding check" : false;

test("a captured frame of the Copy or Swap Scene dialog is read by the browser's decoder", { skip, timeout: 120_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { build } = await import("esbuild");
    const bundled = await build({ entryPoints: [join(uiSource, "scene-command-reader.ts")], bundle: true, format: "iife", globalName: "Reader", platform: "browser", write: false, logLevel: "error" });
    const page = join(work.path, "reader.html");
    writeFileSync(page, `<!doctype html><html><head><meta charset="utf-8"><script>${bundled.outputFiles[0].text.replace(/<\/script/gi, "<\/script")}</script></head><body></body></html>`);
    const frames = { copy: greyPngBase64(800, 480, titleFrame("copy", 341)), swap: greyPngBase64(800, 480, titleFrame("swap", 335)), none: greyPngBase64(800, 480, titleFrame("none")), grid: greyPngBase64(800, 480, titleFrame("swap", 339, false)), small: greyPngBase64(400, 240, new Uint8Array(400 * 240)) };
    const read = await withBrowser(browser, work.path, [], async (open) => open([1000, 700], pathToFileURL(page).href, `(async () => {
      const frames = ${JSON.stringify(frames)};
      const out = {};
      for (const [name, pngBase64] of Object.entries(frames)) out[name] = (await Reader.sceneCommandVerbFromFrame({ pngBase64 })) ?? null;
      out.notPng = (await Reader.sceneCommandVerbFromFrame({ pngBase64: "AAAA" })) ?? null;
      return out;
    })()`));
    assert.deepEqual(read, { copy: "copy", swap: "swap", none: null, grid: null, small: null, notPng: null });
  } finally {
    work.cleanup();
  }
});
