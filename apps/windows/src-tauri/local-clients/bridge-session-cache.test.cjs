"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const directory = fs.mkdtempSync(path.join(os.tmpdir(), "qc-bridge-cache-"));
process.env.QC_REMOTE_LOCAL_MCP_STATE = path.join(directory, "local-mcp.json");
const { rememberSession, rememberedSession, readLocalTextSync } = require("./bridge.cjs");
test.after(() => fs.rmSync(directory, { recursive: true, force: true }));

const cacheFile = () => fs.readdirSync(directory).map((name) => path.join(directory, name))
  .find((file) => !file.endsWith("local-mcp.json") && !file.endsWith(".tmp"));

test("an unchanged catalog is not rewritten", () => {
  rememberSession({ tools: [{ name: "a" }] });
  const file = cacheFile();
  assert.ok(file, "the first catalog is written");
  const past = new Date(Date.now() - 60_000);
  fs.utimesSync(file, past, past);
  rememberSession({ tools: [{ name: "a" }] });
  assert.equal(fs.statSync(file).mtimeMs, past.getTime(), "an identical catalog skips the write");
  rememberSession({ tools: [{ name: "b" }] });
  assert.notEqual(fs.statSync(file).mtimeMs, past.getTime());
  assert.deepEqual(rememberedSession(), { tools: [{ name: "b" }] });
});

test("a read is sized from the file and still enforces its maximum", () => {
  const file = path.join(directory, "document.json");
  fs.writeFileSync(file, "x".repeat(100));
  assert.equal(readLocalTextSync(file, 100).length, 100);
  assert.throws(() => readLocalTextSync(file, 99), /exceeds its software envelope/);
});
