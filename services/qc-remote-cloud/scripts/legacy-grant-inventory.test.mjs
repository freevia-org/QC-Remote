import assert from "node:assert/strict";
import { mkdtempSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import { inventory, isEntryPoint, run } from "./legacy-grant-inventory.mjs";

const subject = "sub-1";
const grant = (id) => `grant:access-${subject}:${id}`;
const token = (id) => `token:access-${subject}:g:${id}`;

function fakeApi({ values = {}, failGet = null, failDeleteAt = null } = {}) {
  const deleted = [];
  return {
    deleted,
    async list(prefix) { return { keys: Object.keys(values).filter((name) => name.startsWith(prefix)).map((name) => ({ name })) }; },
    async get(name) { if (failGet === name) throw new Error("Cloudflare KV read failed (503)"); return values[name] ?? null; },
    async delete(name) { if (deleted.length === failDeleteAt) throw new Error("Cloudflare KV delete failed"); deleted.push(name); },
  };
}

test("a failed read fails the report instead of being counted as a grant without expiry", async () => {
  const values = { [grant("a")]: JSON.stringify({ expiresAt: 5, metadata: { accountGeneration: 0 } }), [grant("b")]: JSON.stringify({ expiresAt: 6 }) };
  await assert.rejects(inventory(fakeApi({ values, failGet: grant("b") }), subject), /read failed/);
  const report = await inventory(fakeApi({ values }), subject);
  assert.equal(report.grantsWithoutExpiry, 0);
  assert.equal(report.latestExpiresAt, 6);
});

test("an unparseable or missing value still reads as no expiry", async () => {
  const report = await inventory(fakeApi({ values: { [grant("a")]: "not json" } }), subject);
  assert.equal(report.grantsWithoutExpiry, 1);
});

test("a delete that fails on the third key still reports how many were deleted", async () => {
  const values = { [token("1")]: "{}", [token("2")]: "{}", [token("3")]: "{}", [grant("a")]: "{}" };
  const api = fakeApi({ values, failDeleteAt: 2 });
  const lines = [];
  await assert.rejects(run(["--subject", subject, "--execute", "--authorization", "ref-1"], api, (line) => lines.push(line)), /delete failed/);
  const final = JSON.parse(lines.at(-1));
  assert.equal(final.deleted, 2);
  assert.equal(typeof final.failedAt, "string");
  assert.equal(final.failedAt.length, 12);
  assert.deepEqual(api.deleted, [token("1"), token("2")]);
});

test("a complete execute reports every deletion and no failure", async () => {
  const api = fakeApi({ values: { [token("1")]: "{}", [grant("a")]: "{}" } });
  const lines = [];
  assert.deepEqual(await run(["--subject", subject, "--execute", "--authorization", "ref-1"], api, (line) => lines.push(line)), { deleted: 2 });
  assert.deepEqual(JSON.parse(lines.at(-1)), { deleted: 2, failedAt: null, authorization: "ref-1" });
});

test("the entry-point check follows symlinks", (t) => {
  const directory = mkdtempSync(join(tmpdir(), "qcrc-entry-"));
  const real = join(directory, "real.mjs");
  writeFileSync(real, "export {};\n");
  const link = join(directory, "link.mjs");
  try { symlinkSync(real, link); } catch (error) { t.skip(`symlinks unavailable: ${error.code}`); return; }
  assert.equal(isEntryPoint(link, pathToFileURL(real).href), true);
  assert.equal(isEntryPoint(real, pathToFileURL(real).href), true);
  assert.equal(isEntryPoint(join(directory, "other.mjs"), pathToFileURL(real).href), false);
  assert.equal(isEntryPoint(undefined, pathToFileURL(real).href), false);
});
