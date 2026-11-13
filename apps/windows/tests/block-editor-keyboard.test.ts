import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { isPlainGridKey, shouldCloseBlockEditorOnEscape } from "../src/block-editor-keyboard.ts";

const escape = (scope: string | null, defaultPrevented = false) => ({
  key: "Escape",
  defaultPrevented,
  target: scope === null ? null : {
    closest: (selector: string) => selector.includes(`[role="${scope}"]`) || (scope === "modal" && selector.includes('[aria-modal="true"]')) ? {} : null
  }
}) as unknown as KeyboardEvent;

test("Escape closes an open block editor regardless of ordinary focused control", () => {
  assert.equal(shouldCloseBlockEditorOnEscape(escape(null), true), true);
  assert.equal(shouldCloseBlockEditorOnEscape(escape("button"), true), true);
  assert.equal(shouldCloseBlockEditorOnEscape(escape("input"), true), true);
});

test("a nested menu or dialog keeps Escape for itself", () => {
  for (const scope of ["menu", "listbox", "dialog", "alertdialog", "modal"]) {
    assert.equal(shouldCloseBlockEditorOnEscape(escape(scope), true), false, scope);
  }
  assert.equal(shouldCloseBlockEditorOnEscape(escape("button", true), true), false);
  assert.equal(shouldCloseBlockEditorOnEscape(escape("button"), false), false);
});

test("the app routes Escape to the block editor before focused-control filters", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const route = app.indexOf("if (shouldCloseBlockEditorOnEscape(event, Boolean(blockDetails)))");
  const textFilter = app.indexOf('const textEntry = target?.matches("input, textarea, [contenteditable=true]")');
  assert.notEqual(route, -1);
  assert.ok(app.indexOf("if (event.defaultPrevented) return") < route, "nested handlers retain first refusal");
  assert.ok(app.indexOf("if (appMenuOpen || sceneWorkflow.command") < route, "application-level surfaces retain first refusal");
  assert.ok(route < textFilter, "Escape is handled before input/button focus exclusions");
  assert.match(app.slice(route, textFilter), /parameterWorkflow\.cancel\(\)[\s\S]*gridWorkflow\.closeOnDevice\(\)/);
});

test("Grid single keys ignore Ctrl, Meta and Alt chords but accept Shift (WF-17)", () => {
  assert.equal(isPlainGridKey({ ctrlKey: false, metaKey: false, altKey: false }), true);
  for (const chord of [{ ctrlKey: true }, { metaKey: true }, { altKey: true }]) {
    assert.equal(isPlainGridKey({ ctrlKey: false, metaKey: false, altKey: false, ...chord }), false);
  }
  assert.equal(isPlainGridKey({ ctrlKey: false, metaKey: false, altKey: false, shiftKey: true } as never), true, "Shift+T still opens the Tuner");
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const gate = app.indexOf("if (!isPlainGridKey(event)) return;");
  assert.ok(gate > app.indexOf("event.ctrlKey ? void chooseScene") && gate < app.indexOf('if (event.key === "[")'), "after the Ctrl+digit scene shortcut, before the plain keys");
});

test("single-flight LOCAL actions guard on a synchronously set ref, and exit/fullscreen report refusals (WF-17)", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(app, /if \(localIntegrationAction\) return/);
  assert.equal((app.match(/if \(localActionRef\.current\) return/g) ?? []).length, 10);
  assert.match(app, /void exitApp\(\)\.catch\(actionFailed\)/);
  assert.match(app, /requestFullscreen\?\.\(\)\)\.catch\(actionFailed\)/);
});
