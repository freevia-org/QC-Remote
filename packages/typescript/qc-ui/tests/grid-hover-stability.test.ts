import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import { bundleModule, findBrowser, hostBody, makeWorkDirectory, uiSource, windowsStyles, androidStyles, withBrowser, type HostName } from "./support/display-page.ts";
import { decodePng } from "./support/png-decode.ts";

/*
 * Moving the pointer over the Grid must not change how the preset number, the preset name or the IN/OUT port
 * labels are drawn. They are SVG text inside the scaled canvas, so they are the first thing to show any change in
 * how that canvas is composited: a forced compositing layer (will-change: transform on the screen) is rasterised
 * again, with different glyph placement, on the first repaint after load, and hovering a Grid control is that
 * repaint. The real Windows window showed the three labels changing size when the pointer entered the Grid.
 *
 * The test hovers Grid controls in the real surface, in both hosts, at several window sizes and device pixel
 * ratios, and compares before and after: computed font size, layout box, glyph pixels, and the compositing
 * reasons of every layer the browser built (none may be a forced layer for the screen).
 */

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the Grid hover check" : false;

const CASES: Array<{ host: HostName; viewport: readonly [number, number]; ratio: number }> = [
  { host: "windows", viewport: [1440, 820], ratio: 1 },
  { host: "windows", viewport: [2182, 1286], ratio: 1 },
  { host: "windows", viewport: [920, 640], ratio: 1.25 },
  { host: "windows", viewport: [1440, 820], ratio: 1.5 },
  { host: "android", viewport: [390, 844], ratio: 2 }
];

type Box = [number, number, number, number];
interface Label { label: string; fontSize: string; box: Box }
interface Probe { labels: Label[]; hits: Array<{ kind: string; box: Box }> }

const SETTLE = `(async () => {
  for (let waited = 0; waited < 5000 && document.querySelector('template[id^="B:"]'); waited += 50) await new Promise((resolve) => setTimeout(resolve, 50));
  await document.fonts.ready;
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
})()`;

const PROBE = `(() => {
  const box = (element) => { const r = element.getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; };
  const labels = [...document.querySelectorAll('svg.coros-vector-canvas text')]
    .filter((text) => /^(7|Money|In|Multi)/.test(text.textContent.trim()))
    .map((text) => ({ label: text.textContent.trim().slice(0, 12), fontSize: getComputedStyle(text).fontSize, box: box(text) }));
  const hits = [...document.querySelectorAll('.coros-vector-actions button')].map((element) => ({ kind: element.className, box: box(element) }));
  return { labels, hits };
})()`;

const overlaps = (a: Box, b: Box, margin: number) => !(a[0] + a[2] < b[0] - margin || a[0] > b[0] + b[2] + margin || a[1] + a[3] < b[1] - margin || a[1] > b[1] + b[3] + margin);

test("hovering the Grid leaves the preset number, name and port labels exactly as drawn", { skip, timeout: 600_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { module: bundle, css } = await bundleModule<{ render(presentation: string): Promise<string> }>(work.path, "hover", `
import { createElement } from "react";
import { prerenderToNodeStream } from "react-dom/static";
import { QuadCortexSurface } from ${JSON.stringify(join(uiSource, "quad-cortex-surface.tsx"))};
import { IDLE_PLACEHOLDER_SNAPSHOT } from ${JSON.stringify(join(uiSource, "index.ts"))};
import { formFactors, skins } from "@qc-remote/form-factors";
import "@qc-remote/theme/theme.css";

const formFactor = formFactors[0];
const skin = skins.find((entry) => entry.id === formFactor.defaultSkinId) ?? skins[0];
const snapshot = {
  ...IDLE_PLACEHOLDER_SNAPSHOT, presetLocation: "7F", presetPosition: 40, presetName: "Money for Nothing", mode: "STOMP",
  blocks: [{ id: "amp", name: "Amp", kind: "amp", category: "Amp", row: 0, column: 2, bypassed: false }, { id: "drive", name: "Drive", kind: "drive", category: "Drive", row: 0, column: 0, bypassed: false }],
  routes: [0, 1, 2, 3].map((row) => ({ row, inputId: 1, outputId: 19, input: "In 1", output: "Multi Out", splitMuted: false }))
};
export async function render(presentation) {
  const { prelude } = await prerenderToNodeStream(createElement(QuadCortexSurface, { formFactor, skin, snapshot, selectedBlockId: "", presentation, onAction() {} }));
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
    const files = new Map<HostName, string>();
    for (const host of ["windows", "android"] as const) {
      const file = join(pages, `${host}.html`);
      writeFileSync(file, `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="display.css"><link rel="stylesheet" href="${host}.css"></head><body>${hostBody(host, await bundle.render(host === "windows" ? "hardware" : "display"))}</body></html>`);
      files.set(host, pathToFileURL(file).href);
    }
    await withBrowser(browser!, work.path, [], async (_page, open) => {
      for (const { host, viewport, ratio } of CASES) {
        const where = `${host} ${viewport.join("x")} @${ratio}`;
        const page = await open(viewport, files.get(host)!, ratio);
        await page.cdp("LayerTree.enable");
        await page.evaluate(SETTLE);
        await page.move(1, 1);
        await new Promise((resolve) => setTimeout(resolve, 600));
        const before = await page.evaluate(PROBE) as Probe;
        assert.equal(before.labels.length, 10, `${where}: the number, the name and four port labels per side are found (${before.labels.map((label) => label.label).join(", ")})`);
        // The unit draws a port label as 16px caps in two lines whose tops are 21 canvas px apart (measured on grid-base and nine more corpus frames).
        for (const label of before.labels.filter((entry) => /^(In|Multi)/.test(entry.label))) assert.equal(label.fontSize, "16px", `${where}: "${label.label}" is the unit's 16px`);
        const steps = await page.evaluate("[...document.querySelectorAll('svg.coros-vector-canvas text')].filter((text) => /^(In|Multi)/.test(text.textContent.trim())).map((text) => text.querySelectorAll('tspan')[1].getAttribute('dy'))") as string[];
        assert.deepEqual([...new Set(steps)], ["21"], `${where}: the second line of a port label is 21 canvas px below the first`);
        const idle = decodePng(await page.screenshot());
        // Targets whose own hover outline cannot touch a label: Grid cells and controls that do not overlap any label box.
        const clear = before.hits.filter((hit) => /coros-vector-(add|block)-hit/.test(hit.kind) && !before.labels.some((label) => overlaps(hit.box, label.box, 8)));
        const onTitle = before.hits.find((hit) => /preset-title-hit/.test(hit.kind));
        assert.ok(clear.length >= 2 && onTitle, `${where}: Grid targets to hover exist`);
        const targets = [...clear.slice(0, 3), onTitle!];
        for (const hit of targets) {
          await page.move(hit.box[0] + hit.box[2] / 2, hit.box[1] + hit.box[3] / 2);
          await new Promise((resolve) => setTimeout(resolve, 400));
          const during = await page.evaluate(PROBE) as Probe;
          assert.deepEqual(during.labels, before.labels, `${where}: hovering ${hit.kind} changed a label's font size or box`);
          const layers = (page.events.filter((event) => event.method === "LayerTree.layerTreeDidChange").at(-1)?.params.layers ?? []) as Array<{ layerId: string }>;
          for (const layer of layers) {
            const reasons = (await page.cdp("LayerTree.compositingReasons", { layerId: layer.layerId }).catch(() => ({ compositingReasonIds: [] }))).compositingReasonIds as string[];
            assert.ok(!reasons.includes("WillChangeTransform"), `${where}: hovering ${hit.kind}: a layer is forced by will-change (${reasons.join(",")}); its tiles are rasterised again on the first repaint`);
          }
          if (hit === onTitle) continue;
          // The hover outline is the only pixel change allowed, and it stays inside the hovered control: every label is pixel-identical.
          const shot = decodePng(await page.screenshot());
          for (const label of before.labels) {
            let changed = 0;
            for (let y = Math.floor(label.box[1] * ratio); y < Math.ceil((label.box[1] + label.box[3]) * ratio); y += 1) {
              for (let x = Math.floor(label.box[0] * ratio); x < Math.ceil((label.box[0] + label.box[2]) * ratio); x += 1) {
                const at = (y * idle.width + x) * 4;
                if (idle.data[at] !== shot.data[at] || idle.data[at + 1] !== shot.data[at + 1] || idle.data[at + 2] !== shot.data[at + 2]) changed += 1;
              }
            }
            assert.equal(changed, 0, `${where}: hovering ${hit.kind} repainted ${changed} glyph pixels of "${label.label}"`);
          }
        }
        await page.move(1, 1);
        await new Promise((resolve) => setTimeout(resolve, 400));
        assert.deepEqual((await page.evaluate(PROBE) as Probe).labels, before.labels, `${where}: the labels are as drawn once the pointer has left`);
      }
    });
  } finally {
    work.cleanup();
  }
});
