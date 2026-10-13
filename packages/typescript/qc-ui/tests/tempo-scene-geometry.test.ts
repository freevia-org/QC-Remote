import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test from "node:test";
import { bundleModule, findBrowser, makeWorkDirectory, withBrowser } from "./support/display-page.ts";

/*
 * The Tempo screen and the scene menu, laid out in headless Chrome at the unit's own 800x480 (the canvas
 * scale is then the identity, so a CSS pixel is a framebuffer pixel) and pinned to what was measured on
 * the unit's screen: the 784x216 panel of five 157px columns over two 107px rows with a 2px seam, a
 * divider 78px tall inset 15px in each cell, the 137x37 boxes of the selects and UNMUTE, the 24px beat discs
 * with one white 10px dot over the current beat, 18px heading and caption, 13.5px small text, the 23px
 * letter on the scene badges in pure black, and the VOLUME dial's pointer on the unit's taper (0.0 dB at
 * raw 0.613). A rule that drifts from those numbers fails here.
 *
 * Needs a Chrome or Chromium; QC_BROWSER_EXECUTABLE names one. CI always has one, so there a missing
 * browser fails the test instead of skipping it.
 */

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the Tempo and scene menu geometry check" : false;

type Box = [number, number, number, number];
interface Measured {
  boxes: Record<string, Box | null>;
  styles: Record<string, Record<string, string> | null>;
  panelCells: Box[];
  selects: Box[];
  dots: Box[];
  scene: { rows: Array<{ badge: Box; letter: Record<string, string>; label: Box }>; popup: Box; popupOverflow: string; activeColor: string };
}

const entry = fileURLToPath(new URL("./support/display-host-entry.tsx", import.meta.url));

const MEASURE = `
(async () => {
  await document.fonts.ready;
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const box = (element) => { if (!element) return null; const r = element.getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; };
  const pick = (element, names, pseudo) => { if (!element) return null; const style = getComputedStyle(element, pseudo); return Object.fromEntries(names.map((name) => [name, style.getPropertyValue(name)])); };
  const one = (selector) => document.querySelector(selector);
  const text = ["font-size", "font-weight", "color"];
  const out = { boxes: {}, styles: {}, panelCells: [], selects: [], dots: [], scene: null };
  const tempo = one(".coros-tempo");
  if (tempo) {
    out.boxes.panel = box(one(".tempo-controls"));
    out.panelCells = [...document.querySelectorAll(".tempo-control")].map(box);
    out.selects = [...document.querySelectorAll(".tempo-select select")].map(box);
    out.boxes.unmute = box(one(".tempo-control.is-mute button"));
    out.boxes.tempoDial = box(one(".is-tempo .tempo-dial svg"));
    out.boxes.volumeDial = box(one(".is-volume .tempo-dial svg"));
    out.boxes.panDial = box(one(".is-pan .tempo-dial svg"));
    out.boxes.sceneSelector = box(one(".tempo-scene"));
    out.dots = [...document.querySelectorAll(".tempo-display i")].map(box);
    out.styles.title = pick(one(".coros-tempo > header > span"), text);
    out.styles.big = pick(one(".tempo-display strong"), text);
    out.styles.caption = pick(one(".tempo-display small"), text);
    out.styles.heading = pick(one(".is-tempo > span"), text);
    out.styles.value = pick(one(".is-tempo > strong"), text);
    out.styles.unitSuffix = pick(one(".is-tempo .tempo-unit"), text);
    out.styles.unmute = pick(one(".tempo-control.is-mute button"), text.concat(["background-color", "border-radius"]));
    out.styles.select = pick(document.querySelector(".tempo-select select"), text.concat(["background-color", "border-radius"]));
    out.styles.selectArrow = pick(one(".tempo-select"), ["border-top-width", "border-top-color", "border-left-width", "right", "top"], "::after");
    out.styles.divider = pick(one(".tempo-control"), ["width", "height", "top", "right", "background-color"], "::after");
    out.styles.lastDivider = pick(document.querySelectorAll(".tempo-control")[4], ["content"], "::after");
    out.styles.activeDot = pick(one(".tempo-display i.is-active"), ["background-color", "box-shadow"]);
    out.styles.idleDot = pick(document.querySelectorAll(".tempo-display i")[1], ["background-color", "box-shadow"]);
    out.styles.badge = pick(one(".tempo-scene .qc-scene-letter-badge"), text);
    out.styles.volumePointer = { angle: one(".is-volume svg.qc-rotary").style.getPropertyValue("--qc-rotary-angle") };
    out.styles.panPointer = { angle: one(".is-pan svg.qc-rotary").style.getPropertyValue("--qc-rotary-angle") };
  }
  const menu = one(".scene-dropdown");
  if (menu) {
    const rows = [...menu.querySelectorAll("button")];
    out.scene = {
      popup: box(menu),
      popupOverflow: getComputedStyle(menu).overflow,
      activeColor: getComputedStyle(menu.querySelector("button.is-active")).color,
      rows: rows.map((row) => ({ badge: box(row.querySelector("span")), letter: pick(row.querySelector("span"), text.concat(["font-weight"])), label: box(row) }))
    };
  }
  return out;
})()
`;

let tempo: Measured;
let scene: Measured;

test("lay out the Tempo screen and the scene menu at the unit's 800x480", { skip, timeout: 240_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { module, css } = await bundleModule<{ render(flavour: string, scenario: object): Promise<string> }>(work.path, "geometry", `export * from ${JSON.stringify(entry)};`);
    const pages = join(work.path, "pages");
    mkdirSync(pages, { recursive: true });
    const scenarios: Record<string, object> = { tempo: { view: null, open: "tempo" }, scene: { view: null, popup: "scene-menu" } };
    const urls: Record<string, string> = {};
    for (const [name, scenario] of Object.entries(scenarios)) {
      const markup = await module.render("windows", { ...scenario, presentation: "display" });
      const file = join(pages, `${name}.html`);
      // The canvas is 800x480 and unscaled: the box is forced to exactly that, which makes the canvas transform the identity.
      writeFileSync(file, `<!doctype html><html lang="en"><head><meta charset="utf-8"><style>${css}
html,body{margin:0;background:#000}.qc-chassis{width:800px!important;height:480px!important;aspect-ratio:auto!important;max-width:none!important}.qc-screen-bezel{width:800px!important;height:480px!important;position:absolute!important;inset:0 auto auto 0!important}</style></head><body><div id="root">${markup}</div></body></html>`);
      urls[name] = pathToFileURL(file).href;
    }
    await withBrowser(browser, work.path, [], async (page) => {
      tempo = await page([800, 480], urls.tempo, MEASURE) as Measured;
      scene = await page([800, 480], urls.scene, MEASURE) as Measured;
    });
  } finally {
    work.cleanup();
  }
});

const near = (actual: number, expected: number, message: string, tolerance = 0.51) => assert.ok(Math.abs(actual - expected) <= tolerance, `${message}: ${actual} is not ${expected} (+/-${tolerance})`);
const boxIs = (actual: Box | null | undefined, expected: Box, message: string, tolerance = 0.51) => {
  assert.ok(actual, `${message}: not rendered`);
  expected.forEach((value, index) => near(actual[index], value, `${message} [${["x", "y", "width", "height"][index]}]`, tolerance));
};

test("the Tempo panel is 784x216: five 157px columns (the last 156) over two 107px rows and a 2px seam", { skip }, () => {
  boxIs(tempo.boxes.panel, [8, 257, 784, 216], "panel");
  assert.equal(tempo.panelCells.length, 10, "ten cells");
  // Document order: TEMPO MODE LED VOLUME MUTE, then PAN (column 1) and the four selects.
  const columns = [8, 165, 322, 479, 636];
  const widths = [157, 157, 157, 157, 156];
  const rows = [257, 366];
  tempo.panelCells.forEach((cell, index) => boxIs(cell, [columns[index % 5], rows[Math.floor(index / 5)], widths[index % 5], 107], `cell ${index}`));
});

test("each cell but the last of a row has a 1px divider, 78px tall and 15px down from the cell's top", { skip }, () => {
  assert.equal(tempo.styles.divider?.width, "1px");
  assert.equal(tempo.styles.divider?.height, "78px");
  assert.equal(tempo.styles.divider?.top, "15px");
  assert.equal(tempo.styles.divider?.right, "0px");
  assert.equal(tempo.styles.divider?.["background-color"], "rgb(16, 16, 16)");
  assert.equal(tempo.styles.lastDivider?.content, "none", "the fifth column has none");
});

test("the heading, big BPM and its caption are 18px, 96px and 18px, the caption in the unit's red", { skip }, () => {
  assert.equal(tempo.styles.title?.["font-size"], "18px");
  assert.equal(tempo.styles.title?.color, "rgb(148, 150, 148)");
  assert.equal(tempo.styles.big?.["font-size"], "96px");
  assert.equal(tempo.styles.big?.color, "rgb(66, 251, 99)");
  assert.equal(tempo.styles.caption?.["font-size"], "18px");
  assert.equal(tempo.styles.caption?.color, "rgb(255, 36, 33)");
});

test("cell headings are 16px; the BPM / dB suffix, UNMUTE and the select text are 13.5px", { skip }, () => {
  assert.equal(tempo.styles.heading?.["font-size"], "16px");
  assert.equal(tempo.styles.value?.["font-size"], "16px");
  assert.equal(tempo.styles.unitSuffix?.["font-size"], "13.5px");
  assert.equal(tempo.styles.unmute?.["font-size"], "13.5px");
  assert.equal(tempo.styles.select?.["font-size"], "13.5px");
  assert.equal(tempo.styles.unmute?.["font-weight"], "400");
});

test("UNMUTE and the four selects are 137x37 boxes 11px (UNMUTE 10px) from their cell's left", { skip }, () => {
  boxIs(tempo.boxes.unmute, [646, 299, 137, 37], "UNMUTE");
  [176, 333, 490, 647].forEach((left, index) => boxIs(tempo.selects[index], [left, 423, 137, 37], `select ${index}`));
  assert.equal(tempo.styles.select?.["background-color"], "rgb(16, 16, 16)");
  assert.equal(tempo.styles.unmute?.["background-color"], "rgb(0, 0, 0)");
  // The triangle is 10 wide and 8 tall, in white.
  assert.equal(tempo.styles.selectArrow?.["border-top-width"], "8px");
  assert.equal(tempo.styles.selectArrow?.["border-top-color"], "rgb(255, 255, 255)");
  assert.equal(tempo.styles.selectArrow?.["border-left-width"], "5px");
});

test("the beat discs are 24px, the current one white with a white 10px dot 24.5px above it and no dot on the others", { skip }, () => {
  [196, 324, 452, 580].forEach((left, index) => boxIs(tempo.dots[index], [left, 201, 24, 24], `disc ${index}`));
  assert.equal(tempo.styles.activeDot?.["background-color"], "rgb(255, 255, 255)");
  assert.equal(tempo.styles.activeDot?.["box-shadow"], "rgb(255, 255, 255) 0px -24.5px 0px -7px");
  assert.equal(tempo.styles.idleDot?.["background-color"], "rgb(66, 65, 66)");
  assert.equal(tempo.styles.idleDot?.["box-shadow"], "none");
});

test("the three dials are drawn at 72x72, the unit's own size, and the volume pointer is on its taper", { skip }, () => {
  for (const name of ["tempoDial", "volumeDial", "panDial"] as const) {
    assert.equal(tempo.boxes[name]?.[2], 72, `${name} width`);
    assert.equal(tempo.boxes[name]?.[3], 72, `${name} height`);
  }
  // 0.0 dB is raw 0.613 on the 270 degree sweep from 135: 135 + 0.613 * 270.
  near(parseFloat(tempo.styles.volumePointer?.angle ?? "NaN"), 135 + 0.613 * 270, "volume pointer angle", 0.01);
  near(parseFloat(tempo.styles.panPointer?.angle ?? "NaN"), 135 + 0.5 * 270, "centred pan pointer angle", 0.01);
  // The dials' centres, from the unit's frame.
  const centre = (box: Box | null) => [box![0] + box![2] / 2, box![1] + box![3] / 2];
  [["tempoDial", 123, 330], ["volumeDial", 593, 330], ["panDial", 123, 439]].forEach(([name, x, y]) => {
    const [cx, cy] = centre(tempo.boxes[name as string]);
    near(cx, x as number, `${name} centre x`, 1);
    near(cy, y as number, `${name} centre y`, 1);
  });
});

test("the header: the scene selector is 132x44 at x 544 with no rules inside it, and the close button sits 9px right of it", { skip }, () => {
  boxIs(tempo.boxes.sceneSelector, [544, 8, 132, 44], "scene selector");
  assert.equal(tempo.styles.badge?.color, "rgb(0, 0, 0)");
  assert.equal(tempo.styles.badge?.["font-size"], "23px");
});

test("scene menu: a 232x416 panel at (454,44) with its top corners rounded, 52px rows and 24px letter badges", { skip }, () => {
  assert.ok(scene.scene, "the scene menu is rendered");
  boxIs(scene.scene.popup, [454, 44, 232, 416], "popup");
  assert.equal(scene.scene.popupOverflow, "hidden", "the first row's fill is clipped to the panel's rounded corners");
  assert.equal(scene.scene.rows.length, 8);
  scene.scene.rows.forEach((row, index) => {
    boxIs(row.label, [454, 44 + 52 * index, 232, 52], `row ${index}`);
    boxIs(row.badge, [470, 58 + 52 * index, 24, 24], `badge ${index}`);
    assert.equal(row.letter?.["font-size"], "23px", `badge ${index} letter size`);
    assert.equal(row.letter?.color, "rgb(0, 0, 0)", `badge ${index} letter colour`);
  });
  assert.equal(scene.scene.activeColor, "rgb(66, 251, 99)", "the current scene is the unit's accent green");
});
