import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { parseCss, type CssRule } from "./support/css-rules.ts";

/*
 * Every control the user can operate shows the same affordance: the pointer
 * cursor, a highlight under the pointer (on a device that can hover) and one on
 * keyboard focus. Inside the on-device display that is one rule in
 * qc-screen-elements.css, scoped to the display root and not to any screen
 * class; for the app's own chrome it is one rule in @qc-remote/theme. These
 * tests fail when a control kind is missing from the lists, when a rule is tied
 * to a screen again (the parameter editor lost both because it did not carry
 * the .qc-screen class), when hover is not guarded for touch screens, and when
 * a source file introduces a clickable element that is not a control.
 */

const root = fileURLToPath(new URL("../../../../", import.meta.url));
const read = (relative: string) => readFileSync(join(root, relative), "utf8").replaceAll("\r\n", "\n");
const displayRules = parseCss(read("packages/typescript/qc-ui/src/qc-screen-elements.css"));
const themeRules = parseCss(read("packages/typescript/qc-theme/src/theme.css"));

/** The control kinds both lists must cover: every native control and every interactive ARIA role the sources use. */
const NATIVE = ["button", "select", "a[href]", "summary", 'input[type="checkbox"]', 'input[type="radio"]', 'input[type="range"]'];
const INTERACTIVE_ROLES = ["button", "link", "menuitem", "menuitemcheckbox", "menuitemradio", "option", "tab", "switch", "radio", "checkbox", "slider"];

const controlList = (selector: string) => {
  const start = selector.indexOf(":is(") >= 0 ? selector.indexOf(":is(") + 4 : selector.indexOf(":where(") + 7;
  let depth = 1;
  let end = start;
  for (; end < selector.length && depth > 0; end += 1) {
    if (selector[end] === "(") depth += 1;
    else if (selector[end] === ")") depth -= 1;
  }
  return selector.slice(start, end - 1).split(/,\s*(?![^\[]*\])/).map((item) => item.trim());
};

const interactionRules = (rules: CssRule[], marker: string) => rules.filter((rule) => rule.selector.includes(marker));

test("the display's shared rule covers every control kind, in one list, for the cursor, the focus outline and the hover outline", () => {
  const rules = interactionRules(displayRules, '[role="slider"]');
  const cursor = rules.find((rule) => rule.declarations.get("cursor") === "pointer");
  const focus = rules.find((rule) => rule.selector.endsWith(":focus-visible"));
  const hover = rules.find((rule) => rule.selector.endsWith(":hover"));
  assert.ok(cursor && focus && hover, "cursor, focus-visible and hover rules exist");
  const list = controlList(cursor.selector);
  for (const kind of NATIVE) assert.ok(list.includes(kind), `${kind} is in the display's control list`);
  for (const role of INTERACTIVE_ROLES) assert.ok(list.includes(`[role="${role}"]`), `role ${role} is in the display's control list`);
  assert.deepEqual(controlList(focus.selector), list, "focus uses the same list");
  assert.deepEqual(controlList(hover.selector), list, "hover uses the same list");
  for (const rule of [cursor, focus, hover]) {
    assert.ok(rule.context.some((prelude) => /^@scope\s*\(\s*\.qc-screen-fixture-root\s*\)/.test(prelude)), "scoped to the display root");
    assert.ok(!/\.qc-screen(?![\w-])/.test(rule.selector), "never tied to a screen class: a panel without the class would lose the affordance");
    assert.ok(!/\.coros-|\.qc-[a-z]/.test(rule.selector.replace(/\[class\*="[^"]*"\]/g, "")), "names no component");
  }
  assert.ok(focus.declarations.get("outline")?.startsWith("1px solid"), "a visible focus outline");
  assert.ok(hover.declarations.get("outline")?.startsWith("1px solid"), "a visible hover outline");
});

test("hover is only ever applied where the device can hover: a touch screen keeps no sticky outline", () => {
  for (const [name, rules] of [["display", displayRules], ["theme", themeRules]] as const) {
    const hovers = interactionRules(rules, '[role="slider"]').filter((rule) => rule.selector.endsWith(":hover"));
    assert.ok(hovers.length > 0, `${name} has a hover rule`);
    for (const rule of hovers) assert.ok(rule.context.includes("@media (hover: hover)"), `${name}: ${rule.selector.slice(-30)} is inside @media (hover: hover)`);
  }
});

test("the app chrome has the same control list, as zero-specificity defaults, outside the display", () => {
  const displayList = controlList(interactionRules(displayRules, '[role="slider"]').find((rule) => rule.declarations.get("cursor") === "pointer")!.selector);
  const chrome = interactionRules(themeRules, '[role="slider"]');
  const cursor = chrome.find((rule) => rule.declarations.get("cursor") === "pointer");
  const focus = chrome.find((rule) => rule.selector.includes(":focus-visible"));
  assert.ok(cursor && focus, "chrome cursor and focus rules exist");
  const list = controlList(cursor.selector);
  for (const item of displayList) assert.ok(list.includes(item), `${item} is in the chrome list too`);
  for (const rule of chrome) {
    assert.ok(rule.selector.startsWith(":where("), "zero specificity: a component's own rule always wins");
    assert.ok(rule.context.some((prelude) => /^@scope\s*\(\s*:root\s*\)\s*to\s*\(\s*\.qc-screen-fixture-root\s*\)/.test(prelude)), "the display is excluded, so the two rules never both apply");
  }
});

// ---------------------------------------------------------------------------------- the sources

const sourceDirectories = ["packages/typescript/qc-ui/src", "apps/windows/src", "apps/android/src"];
const sources = sourceDirectories.flatMap((directory) => readdirSync(join(root, directory))
  .filter((name) => name.endsWith(".tsx") && !/\.test\./.test(name))
  .map((name) => ({ path: `${directory}/${name}`, text: read(`${directory}/${name}`) })));

test("every interactive ARIA role the sources use is in the shared control list", () => {
  const interactive = new Set(["button", "link", "menuitem", "menuitemcheckbox", "menuitemradio", "option", "tab", "switch", "radio", "checkbox", "slider", "spinbutton", "combobox", "textbox", "searchbox"]);
  const covered = new Set(INTERACTIVE_ROLES);
  const missing: string[] = [];
  for (const { path, text } of sources) {
    for (const match of text.matchAll(/\brole="([a-z]+)"/g)) {
      if (interactive.has(match[1]) && !covered.has(match[1]) && !["spinbutton", "combobox", "textbox", "searchbox"].includes(match[1])) missing.push(`${path}: role="${match[1]}"`);
    }
  }
  assert.deepEqual(missing, [], "add the role to the control lists in qc-screen-elements.css and theme.css");
});

/** Elements that carry pointer handlers to read gestures over a whole region: they are not controls and are not offered as ones. */
const GESTURE_SURFACES = [/qc-screen-bezel/, /qc-screen-fixture-root/, /coros-parameter\$\{meter/, /android-app/,
  // A wash that dismisses what is over it is not a visible control (the shared rule skips it too), and a dialog panel only stops the click from reaching its wash.
  /className="[^"]*(?:scrim|wash|backdrop|dimmer|dismiss)[^"]*"/, /mobile-workflow-panel/];

test("a clickable element is a real control: no div, span or svg with a click or pointer handler stands in for a button", () => {
  const offenders: string[] = [];
  for (const { path, text } of sources) {
    for (const match of text.matchAll(/<(div|span|section|li|ul|g|rect|path|svg|img|p|label|article|header|footer|nav|main|aside|small|strong|b|i|h1|h2|h3|circle|ellipse|text)\b([^<>]*?)>/gs)) {
      const attributes = match[2];
      if (!/\bon(Click|DoubleClick|PointerDown|MouseDown|PointerUp|TouchStart)\w*=/.test(attributes)) continue;
      const role = attributes.match(/role="([a-z]+)"/)?.[1];
      if (role && INTERACTIVE_ROLES.includes(role)) continue;
      if (GESTURE_SURFACES.some((surface) => surface.test(attributes))) continue;
      offenders.push(`${path}:${text.slice(0, match.index).split("\n").length} <${match[1]} ${(attributes.match(/className=\S+/)?.[0] ?? "").slice(0, 50)}>`);
    }
  }
  assert.deepEqual(offenders, [], "use a <button> (it gets the pointer and the highlight from the shared rule) or give the element an interactive role");
});

test("the display's control rule does not depend on any screen having the .qc-screen class", () => {
  // The panels that lost the affordance: they are display content with no .qc-screen ancestor.
  const classless = ["coros-parameter-editor", "lane-control-panel", "coros-device-workflow", "scene-command-panel", "scene-label-dialog-panel"];
  const inUi = sources.map((source) => source.text).join("\n");
  for (const name of classless) assert.ok(inUi.includes(name), `${name} still exists`);
  const sheets = read("packages/typescript/qc-ui/src/qc-screen-elements.css");
  assert.ok(!/^\.qc-screen\s+:is\(button/m.test(sheets), "the control rule must not start from .qc-screen");
});
