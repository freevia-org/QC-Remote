import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { classesOf, customPropertiesIn, parseCss, type CssRule } from "./support/css-rules.ts";

/*
 * The on-device display (the 800x480 CorOS screen drawn by QuadCortexSurface)
 * is one piece of shared source. These tests make a second source impossible
 * to ship: a host may offer the display a container, and nothing else.
 */

const root = fileURLToPath(new URL("../../../../", import.meta.url));
const uiSource = join(root, "packages/typescript/qc-ui/src");
const hostSourceDirectories = [join(root, "apps/windows/src"), join(root, "apps/android/src")];

const read = (path: string) => readFileSync(path, "utf8");
const filesIn = (directory: string, extensions: string[]): string[] => readdirSync(directory, { withFileTypes: true })
  .flatMap((entry) => entry.isDirectory() ? filesIn(join(directory, entry.name), extensions) : [join(directory, entry.name)])
  .filter((path) => extensions.some((extension) => path.endsWith(extension)) && !/\.test\.tsx?$/.test(path));

const hostStylesheets = hostSourceDirectories.flatMap((directory) => filesIn(directory, [".css"]));
const hostScripts = hostSourceDirectories.flatMap((directory) => filesIn(directory, [".ts", ".tsx"]));
const uiStylesheets = filesIn(uiSource, [".css"]);
const uiRules = new Map(uiStylesheets.map((path) => [path, parseCss(read(path))]));

/** Class names that belong to the display: its frame, its screens and their parts. */
const DISPLAY_CLASS = /^(?:qc-screen|qc-chassis|qc-display|coros-|directory-|preset-title|vector-|qc-library-icon|scene-label-dialog|chassis-vector|is-(?:canonical-canvas|live-grid|scene-editor|display-only))|-hit$/;
/** Custom properties that belong to the display. Hosts may set exactly the two box knobs. */
const DISPLAY_PROPERTY = /^--qc-(?:screen|grid|nav-bar|display)/;
const HOST_BOX_KNOBS = new Set(["--qc-surface-width", "--qc-display-max-height"]);

/** Hardware-control and state classes are shared vocabulary, not display classes: the phone's quick controls are the unit's own switch component. */
const SHARED_VOCABULARY = /^(?:is-|has-|hardware-|switch-)/;
/** Classes that qc-ui styles inside the display's own scope are display classes whatever they are called. */
const scopedDisplayClasses = new Set<string>();
for (const rules of uiRules.values()) {
  for (const rule of rules) {
    if (rule.context.some((prelude) => /^@scope\s*\(\s*\.qc-screen-fixture-root/.test(prelude))) classesOf(rule.selector).filter((name) => !SHARED_VOCABULARY.test(name)).forEach((name) => scopedDisplayClasses.add(name));
  }
}

test("no host stylesheet selects a display class", () => {
  const offences: string[] = [];
  for (const path of hostStylesheets) {
    for (const rule of parseCss(read(path))) {
      for (const name of classesOf(rule.selector)) {
        if (DISPLAY_CLASS.test(name) || scopedDisplayClasses.has(name)) offences.push(`${path.slice(root.length)}: .${name} in "${rule.selector.slice(0, 80)}"`);
      }
    }
  }
  assert.deepEqual(offences, [], "the display's look is decided in packages/typescript/qc-ui (display-frame.css and the screen sheets) for every host");
});

test("no host stylesheet declares or reads a display custom property beyond the two box knobs", () => {
  const offences: string[] = [];
  for (const path of hostStylesheets) {
    for (const property of customPropertiesIn(read(path))) {
      if (DISPLAY_PROPERTY.test(property) && !HOST_BOX_KNOBS.has(property)) offences.push(`${path.slice(root.length)}: ${property}`);
    }
  }
  assert.deepEqual(offences, []);
});

test("no host script styles or queries the display by class", () => {
  const offences: string[] = [];
  const classText = [
    /className=(?:"([^"]*)"|'([^']*)'|\{`([^`]*)`\}|\{"([^"]*)"\})/g,
    /(?:querySelector(?:All)?|closest|matches)\(\s*(?:"([^"]*)"|'([^']*)'|`([^`]*)`)/g,
    /classList\.\w+\(\s*(?:"([^"]*)"|'([^']*)')/g
  ];
  for (const path of hostScripts) {
    const source = read(path).replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");
    for (const pattern of classText) {
      for (const match of source.matchAll(pattern)) {
        const text = match.slice(1).find((part) => part !== undefined) ?? "";
        for (const token of text.split(/[\s.#>+~,\[\]="'()]+/).filter(Boolean)) {
          if (DISPLAY_CLASS.test(token) || scopedDisplayClasses.has(token)) offences.push(`${path.slice(root.length)}: ${token}`);
        }
      }
    }
    for (const match of source.matchAll(/style=\{\{[^}]*["'](--qc-[\w-]+)["']/g)) {
      if (DISPLAY_PROPERTY.test(match[1]) && !HOST_BOX_KNOBS.has(match[1])) offences.push(`${path.slice(root.length)}: ${match[1]}`);
    }
  }
  assert.deepEqual(offences, []);
});

const frameRules = parseCss(read(join(uiSource, "display-frame.css")));
const rule = (rules: CssRule[], selector: string) => {
  const found = rules.find((candidate) => candidate.selector.replace(/\s+/g, " ") === selector);
  assert.ok(found, `display-frame.css has a rule for ${selector}`);
  return found;
};

/** The one physical aspect of the display: the inner opening of the screen recess in qc-chassis-neutral.svg. */
const DISPLAY_ASPECT = 585.353 / 327.749;
/** How much wider than tall each of the unit's 800x480 pixels is: the physical aspect over 5:3. */
const PIXEL_ASPECT = DISPLAY_ASPECT / (800 / 480);

test("the display box has one fixed physical aspect in every presentation", () => {
  const variables = rule(frameRules, ".qc-chassis, .qc-display-standalone").declarations;
  assert.equal(variables.get("--qc-display-aspect"), "calc(585.353 / 327.749)", "the constant is the vector's inner opening, stated once");
  assert.equal(rule(frameRules, ":is(.qc-chassis, .qc-display-standalone) > .qc-screen-bezel").declarations.get("aspect-ratio"), "var(--qc-display-aspect)");
  assert.equal(rule(frameRules, ".qc-chassis.is-display-only").declarations.get("aspect-ratio"), "var(--qc-display-aspect)");
  assert.equal(rule(frameRules, ".qc-display-standalone").declarations.get("aspect-ratio"), "var(--qc-display-aspect)");
  assert.equal(rule(frameRules, ".qc-chassis.is-display-only").declarations.get("width"), "min(100%, calc(var(--qc-display-max-height, 100vmax) * var(--qc-display-aspect)))");
  assert.ok(Math.abs(PIXEL_ASPECT - 1.0716) < 0.0001, `the pixel aspect is ${PIXEL_ASPECT}`);
});

test("on the hardware the display is exactly the inner opening of the art's recess", () => {
  const svg = read(join(root, "packages/typescript/qc-theme/assets/qc-chassis-neutral.svg"));
  // The recess is drawn as two outlines; the inner one is these four straight edges.
  for (const edge of ["M255.796 84.1888V405.892", "M841.149 405.891V84.1887", "M838.125 81.1649H258.82", "M258.82 408.914H838.125"]) assert.ok(svg.includes(edge), `the art still draws the recess edge ${edge}`);
  const placed = rule(frameRules, ".qc-chassis > .qc-screen-bezel").declarations;
  assert.equal(placed.get("left"), "calc(255.796 / 1096.94 * 100%)");
  assert.equal(placed.get("top"), "calc((81.1649 - 14) / 719.079 * 100%)");
  assert.equal(placed.get("width"), "calc(585.353 / 1096.94 * 100cqi)");
  assert.equal(placed.get("height"), "calc(327.749 / 1096.94 * 100cqi)");
  assert.ok(Math.abs((841.149 - 255.796) - 585.353) < 0.001 && Math.abs((408.914 - 81.1649) - 327.749) < 0.001, "the constants are the opening's own size");
});

test("the canvas is a fixed 800x480 box scaled by the one documented transform", () => {
  const canvas = rule(frameRules, ".qc-screen-bezel > .qc-screen-fixture-root").declarations;
  assert.equal(canvas.get("width"), "800px");
  assert.equal(canvas.get("height"), "480px");
  assert.equal(canvas.get("transform"), "scale(calc(100cqw / 800px), calc(100cqh / 480px))", "x by box width / 800, y by box height / 480; the box's fixed aspect makes their ratio the pixel aspect");
  assert.equal(canvas.get("transform-origin"), "top left");
  assert.equal(canvas.get("container-type"), "size", "inside the canvas 1cqw is 8 canvas pixels on every host");
});

test("nothing else in qc-ui sizes, positions or scales the display box or its canvas", () => {
  const geometry = ["width", "height", "min-width", "min-height", "max-width", "max-height", "inset", "left", "right", "top", "bottom", "transform", "scale", "zoom", "aspect-ratio", "container-type", "padding", "border", "border-radius", "box-shadow", "background", "position", "translate"];
  const offences: string[] = [];
  for (const [path, rules] of uiRules) {
    if (path.endsWith("display-frame.css")) continue;
    for (const candidate of rules) {
      const scopedToRoot = candidate.context.some((prelude) => /^@scope\s*\(\s*\.qc-screen-fixture-root\s*\)/.test(prelude));
      for (const selector of candidate.selector.split(",").map((part) => part.trim())) {
        const last = selector.split(/[\s>+~]+/).filter(Boolean).pop() ?? "";
        if (/::?(?:before|after)/.test(last)) continue;
        const isFrame = classesOf(last).some((name) => name === "qc-screen-bezel" || name === "qc-screen-fixture-root") || (scopedToRoot && last === ":scope");
        if (!isFrame) continue;
        for (const property of geometry) if (candidate.declarations.has(property)) offences.push(`${path.slice(root.length)}: ${selector} sets ${property}`);
      }
    }
  }
  assert.deepEqual(offences, [], "the display frame is owned by display-frame.css");
});

test("the canvas rule is the only scale in qc-ui that treats the two axes differently", () => {
  const offences: string[] = [];
  for (const [path, rules] of uiRules) {
    for (const candidate of rules) {
      if (path.endsWith("display-frame.css") && candidate.selector === ".qc-screen-bezel > .qc-screen-fixture-root") continue;
      if (!/qc-screen-bezel|qc-screen-fixture-root|qc-chassis|qc-display/.test(candidate.selector)) continue;
      const transform = candidate.declarations.get("transform") ?? candidate.declarations.get("scale") ?? "";
      for (const match of transform.matchAll(/scale\(((?:[^()]|\([^()]*\))*)\)/g)) {
        let depth = 0;
        for (const char of match[1]) { depth += char === "(" ? 1 : char === ")" ? -1 : 0; if (char === "," && depth === 0) offences.push(`${path.slice(root.length)}: ${candidate.selector} ${transform}`); }
      }
      if (/^\s*\S+\s+\S+\s*$/.test(candidate.declarations.get("scale") ?? "")) offences.push(`${path.slice(root.length)}: ${candidate.selector} scale: ${candidate.declarations.get("scale")}`);
    }
  }
  assert.deepEqual(offences, []);
});

test("the live Grid canvas is never stretched to fit", () => {
  for (const file of ["coros-official-grid.tsx", "quad-cortex-surface.tsx"]) {
    const source = read(join(uiSource, file));
    for (const element of source.match(/<svg[^>]*className="coros-vector-canvas"[^>]*>/g) ?? []) {
      assert.doesNotMatch(element, /preserveAspectRatio="none"/, `${file}: the Grid's SVG keeps its 800x480 aspect`);
      assert.match(element, /viewBox="0 0 800 480"/);
    }
  }
});

test("every root the surface draws inside the display box is the shared canvas", () => {
  const surface = read(join(uiSource, "quad-cortex-surface.tsx"));
  assert.equal(surface.match(/className="qc-screen-bezel|className=\{`qc-screen-bezel/g)?.length, 1, "one display box");
  const roots = surface.match(/className="qc-screen-fixture-root[^"]*"/g) ?? [];
  assert.equal(roots.length, 5, "scene editor, screen content, fixture screen, live Grid and the dialog layer (Copy/Swap Scene) are the only display roots");
  assert.doesNotMatch(surface, /className=\{[^}]*qc-screen-fixture-root/, "a root never takes a conditional class that could skip the canvas rules");
  assert.match(surface, /presentation === "display" \? " is-display-only" : ""/);
});

test("the standalone display uses the same frame and canvas as the surface", () => {
  const standalone = read(join(uiSource, "device-information-display.tsx"));
  assert.match(standalone, /<div className="qc-display-standalone">\s*<div className="qc-screen-bezel">\s*<div className="qc-screen-fixture-root">/);
  assert.match(standalone, /import "\.\/display-frame\.css";/);
});
