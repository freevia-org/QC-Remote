import assert from "node:assert/strict";
import test from "node:test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { bundleModule, findBrowser, makeWorkDirectory, uiSource, withBrowser } from "./support/display-page.ts";

/*
 * Lays the live Preset Directory out at the unit's own 800x480 and pins the
 * geometry measured from the unit's screen (firmware 4.1.0): the row text, the
 * kebab marks and the header tools. Generic preset names only; the numbers are
 * the unit's, the strings are not.
 */

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the directory row check" : false;

type Rect = [number, number, number, number];
interface Measured {
  rows: Array<{ label: string; run: string; strong: { color: string; weight: string; size: string; right: number }; name: { color: string; weight: string; size: string; left: number }; textLeft: number; textTop: number; rowTop: number; kebab: Rect; kebabColor: string }>;
  folderKebabs: Array<{ rect: Rect; color: string; rowTop: number }>;
  tools: Array<{ name: string; button: Rect; icon: Rect | null }>;
  divider: Rect | null;
  dividerColor: string;
}

const MEASURE = `
(async () => {
  await document.fonts.ready;
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const box = (element) => { const r = element.getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; };
  const rows = [...document.querySelectorAll(".directory-preset-row")].map((row) => {
    const label = row.querySelector(".preset-label");
    const strong = label.querySelector("strong");
    const name = label.querySelector("span");
    const range = document.createRange();
    range.selectNodeContents(label);
    const text = range.getBoundingClientRect();
    const more = row.querySelector(".preset-row-more svg");
    const style = (element) => getComputedStyle(element);
    return {
      label: label.getAttribute("class"), run: label.textContent,
      strong: { color: style(strong).color, weight: style(strong).fontWeight, size: style(strong).fontSize, right: strong.getBoundingClientRect().right },
      name: { color: style(name).color, weight: style(name).fontWeight, size: style(name).fontSize, left: name.getBoundingClientRect().left },
      textLeft: text.left, textTop: text.top, rowTop: row.getBoundingClientRect().top,
      kebab: box(more), kebabColor: style(more).color
    };
  });
  const folderKebabs = [...document.querySelectorAll(".directory-folders .directory-folder-more svg")].map((svg) => ({ rect: box(svg), color: getComputedStyle(svg).color, rowTop: svg.closest("button").getBoundingClientRect().top }));
  const tools = [...document.querySelectorAll(".coros-directory-header .directory-tools > button")].map((button) => ({ name: button.getAttribute("aria-label"), button: box(button), icon: button.querySelector("svg") ? box(button.querySelector("svg")) : null }));
  const divider = document.querySelector(".directory-tools .directory-tool-divider");
  return { rows, folderKebabs, tools, divider: divider ? box(divider) : null, dividerColor: divider ? getComputedStyle(divider).backgroundColor : "" };
})()
`;

let measured: Measured;

test("lay the live Directory out at 800x480", { skip, timeout: 120_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { module: bundle, css } = await bundleModule<{ render(): Promise<string> }>(work.path, "directory-entry", `
import { createElement } from "react";
import { prerenderToNodeStream } from "react-dom/static";
import { QuadCortexSurface } from ${JSON.stringify(join(uiSource, "quad-cortex-surface.tsx"))};
import { IDLE_PLACEHOLDER_SNAPSHOT } from ${JSON.stringify(join(uiSource, "index.ts"))};
import { formFactors, skins } from "@qc-remote/form-factors";
import "@qc-remote/theme/theme.css";
const formFactor = formFactors[0];
const skin = skins.find((entry) => entry.id === formFactor.defaultSkinId) ?? skins[0];
const names = ["Preset A", "Preset B", "Preset C Long Name Re", "Unsaved", "Preset E_1", "Preset F", "Preset G", "Unsaved"];
const presets = names.map((name, index) => ({ position: 48 + index, location: "7" + String.fromCharCode(65 + index), name, instrument: 0, occupied: name !== "Unsaved" }));
const folders = [{ key: "f", name: "Factory Presets", isFactory: true }, ...["My Presets", "Folder B", "Folder C", "Folder D"].map((name, index) => ({ key: "k" + index, name, isFactory: false }))];
const directory = { open: true, loading: false, disabled: false, interactionSessionKey: "a",
  list: { setlistKey: "k0", setlistName: "My Presets", presets, folders },
  onClose() {}, onRefresh() {}, onRecall() {}, onSelectSetlist() {}, onNewSetlist() {}, onPresetAction() {}, onPresetBatchAction() {}, onArrangePreset() {} };
export async function render() {
  const { prelude } = await prerenderToNodeStream(createElement(QuadCortexSurface, { formFactor, skin, snapshot: { ...IDLE_PLACEHOLDER_SNAPSHOT, presetPosition: 53, setlistKey: "k0" }, selectedBlockId: "", presentation: "display", presetDirectory: directory, onAction() {} }));
  const chunks = [];
  for await (const chunk of prelude) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}
`);
    const markup = await bundle.render();
    const pages = join(work.path, "pages");
    mkdirSync(pages, { recursive: true });
    writeFileSync(join(pages, "display.css"), css);
    // The display box is the unit's 800x480 pixels, one for one.
    writeFileSync(join(pages, "directory.html"), `<!doctype html><html lang="en"><head><meta charset="utf-8"><link rel="stylesheet" href="display.css"><style>html,body{margin:0;background:#000;overflow:hidden}.qc-screen-bezel{position:fixed!important;left:0;top:0;width:800px!important;height:480px!important;aspect-ratio:auto!important;max-width:none!important}</style></head><body>${markup}</body></html>`);
    measured = await withBrowser(browser, work.path, [], async (page) => await page([800, 480], pathToFileURL(join(pages, "directory.html")).href, MEASURE) as Measured);
  } finally {
    work.cleanup();
  }
  assert.equal(measured.rows.length, 8);
});

const near = (actual: number, expected: number, tolerance: number, what: string) => assert.ok(Math.abs(actual - expected) <= tolerance, `${what}: ${actual.toFixed(2)}, the unit has ${expected}`);

test("a preset row is one run of text in one ink, from the row's 16px inset", { skip }, () => {
  const expected = ["7A Preset A", "7B Preset B", "7C Preset C Long Name Re", "7D Unsaved", "7E Preset E_1", "7F Preset F", "7G Preset G", "7H Unsaved"];
  measured.rows.forEach((row, index) => {
    assert.equal(row.run, expected[index]);
    assert.equal(row.strong.weight, row.name.weight, `${row.run}: the location and the name share a weight`);
    assert.equal(row.strong.size, row.name.size, `${row.run}: the location and the name share a size`);
    assert.equal(row.strong.color, row.name.color, `${row.run}: the location and the name share an ink`);
    assert.equal(row.strong.size, "16px");
    assert.equal(row.strong.weight, "400");
    // The unit's first glyph is on x 409 of 800 (list panel 393, 16px inset); the name follows the location after one space, not on a column of its own.
    near(row.textLeft, 409, 1, `${row.run}: the run begins`);
    assert.ok(row.name.left - row.strong.right < 6, `${row.run}: the name follows the location at one space, not in a column (${(row.name.left - row.strong.right).toFixed(1)}px)`);
  });
});

test("rows are white, an empty slot #adaaad and the loaded preset the accent", { skip }, () => {
  const colours = measured.rows.map((row) => row.strong.color);
  assert.deepEqual(colours, ["rgb(255, 255, 255)", "rgb(255, 255, 255)", "rgb(255, 255, 255)", "rgb(173, 170, 173)", "rgb(255, 255, 255)", "rgb(66, 251, 99)", "rgb(255, 255, 255)", "rgb(173, 170, 173)"]);
});

test("the row kebab is the 24px mark, its dots on x 763-765, in the muted toolbar ink", { skip }, () => {
  for (const row of measured.rows) {
    near(row.kebab[2], 24, 0.01, "kebab width");
    near(row.kebab[3], 24, 0.01, "kebab height");
    // The mark's dots sit 12.5 from its left edge; the unit's centre column is 764.
    near(row.kebab[0] + 12.5, 764.5, 0.6, "kebab dot column");
    // Dots at 4, 12 and 20 in the box: the middle one on y 85 of the first row (60..111).
    near(row.kebab[1] + 12 - row.rowTop, 25, 0.6, "kebab middle dot below the row top");
    assert.equal(row.kebabColor, "rgb(99, 97, 99)");
  }
});

test("a folder kebab is the same mark, its dots on x 242-244", { skip }, () => {
  assert.ok(measured.folderKebabs.length >= 4);
  for (const kebab of measured.folderKebabs) {
    near(kebab.rect[2], 24, 0.01, "folder kebab width");
    near(kebab.rect[0] + 12.5, 243.5, 0.6, "folder kebab dot column");
    near(kebab.rect[1] + 12 - kebab.rowTop, 31, 0.6, "folder kebab middle dot below the row top");
    assert.equal(kebab.color, "rgb(99, 97, 99)");
  }
});

test("the header tools are the unit's 24px marks in 66px buttons, with the divider before the confirm", { skip }, () => {
  assert.deepEqual(measured.tools.map((tool) => Math.round(tool.button[0])), [462, 536, 610, 694]);
  for (const tool of measured.tools) {
    assert.ok(tool.icon, tool.name ?? "tool");
    near(tool.icon![3], 24, 0.01, `${tool.name} icon height`);
    assert.ok(tool.icon![2] <= 24.01, `${tool.name} icon is no wider than its 24px cell (${tool.icon![2]})`);
  }
  assert.deepEqual(measured.tools.map((tool) => Math.round(tool.button[2])), [66, 66, 66, 98]);
  assert.ok(measured.divider);
  near(measured.divider![0], 684, 1, "divider x");
  near(measured.divider![2], 2, 0.01, "divider width");
  near(measured.divider![3], 22, 0.01, "divider height");
  assert.equal(measured.dividerColor, "rgb(41, 44, 41)");
});
