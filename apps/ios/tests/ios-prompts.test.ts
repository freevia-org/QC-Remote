import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("../src/use-ios-prompts.tsx", import.meta.url), "utf8");

test("iOS prompts delegate to the shared Android hook that settles replaced and unmounted dialogs", () => {
  assert.match(source, /from "@qc-remote\/android-src\/use-android-prompts\.tsx"/);
  assert.doesNotMatch(source, /new Promise|setConfirmation|setTextPrompt/,
    "the iOS copy must not re-implement the prompt state that leaked unsettled promises");
  assert.match(source, /registerWorkflowConfirmation\(\(message\) => Promise\.resolve\(confirm\(message\)\)\)/);
});
