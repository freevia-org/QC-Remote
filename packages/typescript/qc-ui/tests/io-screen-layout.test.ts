import assert from "node:assert/strict";
import test from "node:test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { bundleModule, findBrowser, makeWorkDirectory, uiSource, withBrowser } from "./support/display-page.ts";

/*
 * The live I/O Settings screen (Output 1/2, both strips) laid out at the unit's own 800x480 in headless Chrome and
 * compared with what the unit draws. The numbers are measured from the unit's frame (4.1.0): label and reading ink,
 * the dim and selected inks of the On/Off captions, the caption inks of the port row, the plugged headphones jack and
 * the 3/4 link glyph's row. It needs a Chrome or Chromium (QC_BROWSER_EXECUTABLE); CI always has one.
 */

const browser = findBrowser();
const mustRun = Boolean(process.env.CI) || process.env.QC_REQUIRE_BROWSER === "1";
const skip = !browser && !mustRun ? "no Chrome or Chromium found; set QC_BROWSER_EXECUTABLE to run the I/O layout check" : false;

const MEASURE = `
(async () => {
  await document.fonts.ready;
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const canvas = document.querySelector(".qc-screen-fixture-root").getBoundingClientRect();
  const style = (element) => { const s = getComputedStyle(element); return { color: s.color, size: parseFloat(s.fontSize), weight: s.fontWeight, fill: s.fill }; };
  const textBox = (element) => { const range = document.createRange(); range.selectNodeContents(element); const r = range.getBoundingClientRect(); return [r.left - canvas.left, r.top - canvas.top, r.width, r.height]; };
  const byText = (selector, text) => [...document.querySelectorAll(selector)].filter((element) => element.textContent.trim() === text);
  const strips = [...document.querySelectorAll(".io-editor.is-output > section")];
  const strip = strips[0];
  const switches = [...strip.querySelectorAll(".io-switch")];
  const mute = switches[1];
  const buttons = (root) => [...root.querySelectorAll("button")].map((button) => ({ text: button.textContent, ...style(button), box: textBox(button), pressed: button.getAttribute("aria-pressed") }));
  const box = (element) => { const r = element.getBoundingClientRect(); return [r.left - canvas.left, r.top - canvas.top, r.width, r.height]; };
  const caption = (text) => { const element = byText(".io-ports > button > small", text)[0]; return element ? { ...style(element), active: element.parentElement.classList.contains("is-active") } : null; };
  const jack = (socket) => { const svg = document.querySelector('[data-io-socket="' + socket + '"] svg'); return { state: svg.getAttribute("data-state"), fills: [...svg.querySelectorAll("path")].map((path) => getComputedStyle(path).fill) }; };
  const geq = document.querySelector(".io-global-eq");
  return {
    strips: strips.length,
    heading: { small: style(document.querySelector(".io-heading small")), title: style(document.querySelector(".io-heading strong")), text: document.querySelector(".io-heading strong").textContent },
    label: { ...style(strip.querySelector("em")), text: strip.querySelector("em").textContent },
    reading: { number: style(strip.querySelector(".io-readout strong")), unit: style(strip.querySelector(".io-readout strong small")), unitText: strip.querySelector(".io-readout strong small").textContent },
    dialReading: { box: textBox(strip.querySelector(".io-dial-wrap > strong")), unit: style(strip.querySelector(".io-dial-wrap > strong small")) },
    mute: buttons(mute),
    muteLabel: { ...style(mute.querySelector("em")), text: mute.querySelector("em").textContent },
    groundLift: buttons(switches[0]),
    captions: { idle: caption("USB"), activeOut: caption("OUT 1/L"), midi: caption("MIDI IN") },
    headphonesGlyph: style(document.querySelector(".io-headphone-label svg")),
    phonesJack: jack("phones"), captureJack: jack("capture-out"), out1Jack: jack("out1"),
    link12: box(document.querySelector(".io-port-link-output12")), link34: box(document.querySelector(".io-port-link-output34")),
    geq: { ...style(geq), box: box(geq), led: box(geq.querySelector("i")), ledColor: getComputedStyle(geq.querySelector("i")).backgroundColor },
    strip2Top: box(strips[1])[1] - box(strips[0])[1]
  };
})()
`;

type Style = { color: string; size: number; weight: string };
interface Measured {
  strips: number; heading: { small: Style; title: Style; text: string }; label: Style & { text: string };
  reading: { number: Style; unit: Style; unitText: string }; dialReading: { box: number[]; unit: Style };
  mute: Array<Style & { text: string; box: number[]; pressed: string }>; muteLabel: Style & { text: string };
  groundLift: Array<Style & { text: string; box: number[]; pressed: string }>;
  captions: { idle: Style | null; activeOut: Style | null; midi: Style | null };
  headphonesGlyph: Style; phonesJack: { state: string; fills: string[] }; captureJack: { state: string; fills: string[] }; out1Jack: { state: string; fills: string[] };
  link12: number[]; link34: number[]; geq: Style & { box: number[]; led: number[]; ledColor: string }; strip2Top: number;
}

let measured: Measured | undefined;
const near = (actual: number, expected: number, tolerance: number, what: string) => assert.ok(Math.abs(actual - expected) <= tolerance, `${what}: ${actual} is not within ${tolerance} of ${expected}`);

test("lay out the live I/O Settings screen at the unit's size", { skip, timeout: 240_000 }, async () => {
  assert.ok(browser, "CI must provide a Chrome or Chromium (set QC_BROWSER_EXECUTABLE)");
  const work = makeWorkDirectory();
  try {
    const { module: bundle, css } = await bundleModule<{ html: string }>(work.path, "io-screen", `
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { CorOsIoSettingsScreen } from ${JSON.stringify(join(uiSource, "io-global-eq-screens.tsx"))};
import "@qc-remote/theme/theme.css";
const outputs = [4, 5, 6, 7].map((outputPortId) => ({ outputPortId, levelDb: 0, groundLift: 0, muted: false }));
const io = { inputs: [], outputs, expressionPorts: [], xlr12Linked: false, out34Linked: false };
const workflow = { io, loading: false, pending: false, eq: { parameters: [], bypassed: true }, unitSelection: { view: "output", plugged: ["usb", "phones"] }, setOutputPort() {}, setInputPort() {}, setOutputPairing() {}, setUsbPort() {}, setMidiThru() {} };
export const html = renderToStaticMarkup(createElement("div", { className: "qc-screen-fixture-root is-canonical-canvas" }, createElement(CorOsIoSettingsScreen, { workflow, onGlobalEq() {}, onClose() {} })));
`);
    const pages = join(work.path, "pages");
    mkdirSync(pages, { recursive: true });
    writeFileSync(join(pages, "io.html"), `<!doctype html><html lang="en"><head><meta charset="utf-8"><style>html,body{margin:0;background:#000}${css}
.qc-screen-fixture-root{position:absolute;left:0;top:0;width:800px;height:480px;container-type:size;overflow:hidden}</style></head><body>${bundle.html}</body></html>`);
    measured = await withBrowser(browser, work.path, [], (page) => page([800, 480], pathToFileURL(join(pages, "io.html")).href, MEASURE)) as Measured;
  } finally {
    work.cleanup();
  }
  assert.equal(measured.strips, 2, "an unlinked pair has two strips");
});

test("the heading and the pair's strips read as the unit's do", { skip }, () => {
  const m = measured!;
  assert.equal(m.heading.text, "Output 1/2");
  assert.equal(m.heading.title.color, "rgb(66, 251, 99)", "the title is the screen's accent, as the selected jack's ring");
  assert.equal(m.heading.small.color, "rgb(148, 150, 148)");
  near(m.strip2Top, 109, 1, "the second strip sits 109px below the first");
});

test("labels are white at the unit's size and readings are a white number with a dim, smaller unit", { skip }, () => {
  const m = measured!;
  assert.equal(m.label.color, "rgb(255, 255, 255)");
  near(m.label.size, 15.5, 0.01, "label size");
  assert.equal(m.reading.number.color, "rgb(255, 255, 255)");
  near(m.reading.number.size, 15.5, 0.01, "reading size");
  assert.equal(m.reading.unitText, "dB");
  assert.equal(m.reading.unit.color, "rgb(148, 150, 148)");
  near(m.reading.unit.size, 14, 0.01, "the unit of a reading");
  assert.equal(m.dialReading.unit.color, "rgb(148, 150, 148)");
  // The level under the knob inks x 21..63 and ends on y 346 (strip 1).
  near(m.dialReading.box[0], 21, 1, "knob reading left");
  near(m.dialReading.box[1] + m.dialReading.box[3], 349, 3, "knob reading foot");
});

test("MUTE and GROUND LIFT: the selected caption is white, the other dim, both in normal weight, in the unit's columns", { skip }, () => {
  const m = measured!;
  for (const [name, buttons] of [["MUTE", m.mute], ["GROUND LIFT", m.groundLift]] as const) {
    const [on, off] = buttons;
    assert.deepEqual([on.text, off.text], ["On", "Off"], name);
    assert.deepEqual([on.pressed, off.pressed], ["false", "true"], `${name}: Off is the selected caption`);
    assert.equal(off.color, "rgb(255, 255, 255)", `${name}: Off`);
    assert.equal(on.color, "rgb(123, 125, 123)", `${name}: On`);
    assert.equal(off.weight, "400", `${name}: Off is not bold`);
    assert.equal(on.weight, "400", `${name}: On`);
    near(off.size, 14, 0.01, `${name} caption size`);
    near(on.box[1] + on.box[3] / 2, 315, 3, `${name} On row`);
    near(off.box[1] + off.box[3] / 2, 339, 3, `${name} Off row`);
    near(off.box[0] - on.box[0], 0, 1, `${name}: On and Off share a left edge`);
  }
  assert.equal(m.muteLabel.text, "MUTE");
  assert.equal(m.muteLabel.color, "rgb(255, 255, 255)");
  near(m.muteLabel.size, 15.5, 0.01, "MUTE label size");
  near(m.mute[0].box[0], 691, 1.5, "MUTE captions start at the unit's x 691");
  near(m.groundLift[0].box[0], 220, 1.5, "GROUND LIFT captions start at the unit's x 220");
});

test("the port row inks its captions #848684, the selected pair white, the headphones mark #848684, a plugged jack white", { skip }, () => {
  const m = measured!;
  assert.equal(m.captions.idle!.color, "rgb(132, 134, 132)");
  assert.equal(m.captions.midi!.color, "rgb(132, 134, 132)");
  assert.equal(m.captions.activeOut!.color, "rgb(255, 255, 255)");
  assert.equal(m.headphonesGlyph.color, "rgb(132, 134, 132)");
  assert.equal(m.phonesJack.state, "plugged");
  assert.ok(m.phonesJack.fills.includes("rgb(255, 255, 255)"), "the headphones jack the unit marks taken is the white nut");
  assert.equal(m.captureJack.state, "idle");
  assert.ok(!m.captureJack.fills.includes("rgb(255, 255, 255)"));
  assert.equal(m.out1Jack.state, "active");
  near(m.link12[1] - m.link34[1], 4, 0.5, "the unit draws the 3/4 link four rows above the 1/2 one");
});

test("GLOBAL EQ is 16px with the unit's 6px lamp at its top right", { skip }, () => {
  const { geq } = measured!;
  near(geq.size, 16, 0.01, "GLOBAL EQ size");
  near(geq.led[0] - geq.box[0], 98, 1, "lamp left within the button");
  near(geq.led[1] - geq.box[1], 5, 1, "lamp top within the button");
  near(geq.led[2], 6, 0.5, "lamp width");
  assert.equal(geq.ledColor, "rgb(41, 44, 41)", "the Global EQ is bypassed in this fixture: the dim control ink");
});
