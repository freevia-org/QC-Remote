import assert from "node:assert/strict";
import test from "node:test";
import { withTimeout } from "../src/tauri-invoke.ts";

test("withTimeout passes a result and a rejection through (WF-8)", async () => {
  assert.equal(await withTimeout(Promise.resolve(7), 1000), 7);
  await assert.rejects(withTimeout(Promise.reject(new Error("native failure")), 1000), /native failure/);
});

test("withTimeout rejects a read that never answers", async () => {
  await assert.rejects(withTimeout(new Promise(() => undefined), 20, "A status read"), /A status read did not answer/);
});

test("withTimeout clears its timer once the read settles", async () => {
  const before = process.getActiveResourcesInfo().filter((name) => name === "Timeout").length;
  await withTimeout(Promise.resolve(1), 60_000);
  assert.equal(process.getActiveResourcesInfo().filter((name) => name === "Timeout").length, before);
});
