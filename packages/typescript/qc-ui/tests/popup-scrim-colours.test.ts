import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test from "node:test";
import { androidStyles, bundleModule, findBrowser, hostBody, makeWorkDirectory, uiSource, windowsStyles, withBrowser, type HostName } from "./support/display-page.ts";
import { decodePng } from "./support/png-decode.ts";

/*
 * What the unit shows behind and inside each popup, pinned as numbers. The unit's 800x480 screen is RGB565 and its
 * dim is a substitution on its own ladders, not an alpha blend, so each popup is rendered in headless Chrome and the
 * pixel at a flat point of the unit's own frame is read back. The points and values are measured on the unit (the
 * frames themselves are private and are not kept): a point is `x,y` on the 800x480 frame, and a value is what the
 * unit reads there. Every value must be met within two levels, in both hosts.
 *
 * The Grid dim is one wash (--qc-screen-wash-grid-menu): black reads 66,65,66 behind the Grid menu, both
 * route pickers and the Copy/Swap Scene dialog alike. A popup that draws its own scrim tint
 * lands somewhere else (the Copy/Swap scrim read 36,36,36 where the unit reads 66,65,66).
 */

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the scrim colour check" : false;

type Rgb = readonly [number, number, number];
interface Reading { what: string; at: readonly [number, number]; unit: Rgb }

const WASHED_BLACK: Rgb = [66, 65, 66];
const WASHED_SLOT: Rgb = [66, 69, 66];
const PANEL: Rgb = [16, 16, 16];

const READINGS: Record<string, Reading[]> = {
  // The unit dims the Grid behind Copy and Swap Scene exactly as it does behind its menus.
  "copy-scene": [
    { what: "Grid ground behind the dialog", at: [679, 248], unit: WASHED_BLACK },
    { what: "I/O slot behind the dialog", at: [29, 216], unit: WASHED_SLOT },
    { what: "dialog panel", at: [247, 172], unit: PANEL },
    { what: "CANCEL button", at: [250, 300], unit: [0, 0, 0] }
  ],
  "swap-scene": [
    { what: "Grid ground behind the dialog", at: [679, 248], unit: WASHED_BLACK },
    { what: "I/O slot behind the dialog", at: [29, 216], unit: WASHED_SLOT },
    { what: "dialog panel", at: [247, 172], unit: PANEL },
    { what: "CANCEL button", at: [250, 300], unit: [0, 0, 0] }
  ],
  "scene-destination-picker": [
    { what: "Grid ground behind the dialog", at: [679, 248], unit: WASHED_BLACK },
    { what: "I/O slot behind the dialog", at: [29, 216], unit: WASHED_SLOT }
  ],
  "input-picker": [
    { what: "Grid ground", at: [471, 322], unit: WASHED_BLACK },
    { what: "I/O slot", at: [29, 216], unit: WASHED_SLOT },
    { what: "picker panel", at: [265, 282], unit: PANEL }
  ],
  "output-picker": [
    { what: "Grid ground", at: [195, 322], unit: WASHED_BLACK },
    { what: "I/O slot", at: [29, 216], unit: WASHED_SLOT }
  ],
  "add-device-drawer": [
    { what: "Grid beside the drawer", at: [186, 313], unit: WASHED_SLOT }
  ],
  // The scene dropdown is not dimmed by the unit: the Grid stays black behind it.
  "scene-menu": [
    { what: "Grid ground (undimmed)", at: [195, 322], unit: [0, 0, 0] },
    { what: "dropdown panel", at: [622, 293], unit: [49, 52, 49] }
  ]
};

const settle = `(async () => {
  for (let waited = 0; waited < 5000 && document.querySelector('template[id^="B:"]'); waited += 50) await new Promise((resolve) => setTimeout(resolve, 50));
  await document.fonts.ready;
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const root = document.querySelector('.qc-screen-bezel > .qc-screen-fixture-root');
  const r = root.getBoundingClientRect();
  return [r.left, r.top, r.width, r.height];
})()`;

test("every popup's dim and panel colours are the ones the unit shows", { skip, timeout: 600_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { module: bundle, css } = await bundleModule<{ render(fixture: string, presentation: string): Promise<string> }>(work.path, "scrims", `
import { createElement } from "react";
import { prerenderToNodeStream } from "react-dom/static";
import { QuadCortexSurface } from ${JSON.stringify(join(uiSource, "quad-cortex-surface.tsx"))};
import { IDLE_PLACEHOLDER_SNAPSHOT } from ${JSON.stringify(join(uiSource, "index.ts"))};
import { POPUP_PROPS_BY_ID } from ${JSON.stringify(fileURLToPath(new URL("./support/popup-props.tsx", import.meta.url)))};
import { formFactors, skins } from "@qc-remote/form-factors";
import "@qc-remote/theme/theme.css";

export async function render(fixtureId, presentation) {
  const formFactor = formFactors[0];
  const skin = skins.find((entry) => entry.id === formFactor.defaultSkinId) ?? skins[0];
  const { prelude } = await prerenderToNodeStream(createElement(QuadCortexSurface, {
    formFactor, skin, snapshot: IDLE_PLACEHOLDER_SNAPSHOT, selectedBlockId: "", presentation, onAction() {}, ...POPUP_PROPS_BY_ID[fixtureId]()
  }));
  const chunks = [];
  for await (const chunk of prelude) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}
`);
    const pages = join(work.path, "pages");
    mkdirSync(pages, { recursive: true });
    writeFileSync(join(pages, "display.css"), css);
    writeFileSync(join(pages, "windows.css"), readFileSync(windowsStyles, "utf8"));
    writeFileSync(join(pages, "android.css"), readFileSync(androidStyles, "utf8"));
    const cases: Array<{ key: string; fixture: string; url: string }> = [];
    for (const fixture of Object.keys(READINGS)) {
      for (const host of ["windows", "android"] as HostName[]) {
        const markup = await bundle.render(fixture, host === "windows" ? "hardware" : "display");
        const key = `${fixture}-${host}`;
        const file = join(pages, `${key}.html`);
        writeFileSync(file, `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="display.css"><link rel="stylesheet" href="${host}.css"></head><body>${hostBody(host, markup)}</body></html>`);
        cases.push({ key, fixture, url: pathToFileURL(file).href });
      }
    }
    const failures: string[] = [];
    await withBrowser(browser, work.path, [], async (_page, open) => {
      for (const entry of cases) {
        // A large window, so the 800x480 screen is drawn at least at its own size and every sampled point is well inside a flat area.
        const page = await open([2182, 1286], entry.url, 1);
        const [left, top, width, height] = await page.evaluate(settle) as number[];
        const picture = decodePng(await page.screenshot());
        if (process.env.QC_SCRIM_DUMP) writeFileSync(join(process.env.QC_SCRIM_DUMP, `${entry.key}.png`), await page.screenshot());
        for (const reading of READINGS[entry.fixture]) {
          const x = Math.round(left + reading.at[0] / 800 * width), y = Math.round(top + reading.at[1] / 480 * height);
          const seen = [0, 1, 2].map((k) => picture.data[(y * picture.width + x) * 4 + k]);
          const worst = Math.max(...seen.map((value, k) => Math.abs(value - reading.unit[k])));
          if (worst > 2) failures.push(`${entry.key}: ${reading.what} at ${reading.at.join(",")} reads ${seen.join(",")}; the unit reads ${reading.unit.join(",")}`);
        }
      }
    });
    assert.deepEqual(failures, []);
  } finally {
    work.cleanup();
  }
});
