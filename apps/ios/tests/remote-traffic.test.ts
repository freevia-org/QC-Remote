import assert from "node:assert/strict";
import test from "node:test";
import { RemoteTraffic } from "../src/remote-traffic.ts";

test("unsupported idle SSE subscription is rejected locally with no HTTP request", async t => {
  let calls = 0;
  t.mock.method(globalThis, "fetch", async () => { calls++; return new Response(""); });
  const traffic = new RemoteTraffic();
  for (let i = 0; i < 100; i++) assert.equal((await traffic.fetch("https://example.test/mcp")).status, 405);
  assert.equal(calls, 0);
});

test("explicit reads stay fresh and OAuth refresh or revocation responses are preserved", async t => {
  let calls = 0;
  t.mock.method(globalThis, "fetch", async () => new Response(String(++calls), { status: calls === 3 ? 401 : 200 }));
  const traffic = new RemoteTraffic();
  assert.equal(await (await traffic.fetch("https://example.test/mcp", { method: "POST" })).text(), "1");
  assert.equal(await (await traffic.fetch("https://example.test/mcp", { method: "POST" })).text(), "2");
  assert.equal((await traffic.fetch("https://example.test/mcp", { method: "POST" })).status, 401);
  assert.equal((await traffic.fetch("https://example.test/token", { method: "POST" })).status, 200);
  assert.equal(calls, 4);
});

test("429 Retry-After blocks all HTTP requests including another tool and reconnect", async t => {
  let now = 0, calls = 0;
  t.mock.method(globalThis, "fetch", async () => { calls++; return new Response("", { status: 429, headers: { "retry-after": "120" } }); });
  const traffic = new RemoteTraffic(() => now);
  await traffic.fetch("https://example.test/mcp", { method: "POST" });
  now = 60_000;
  await assert.rejects(traffic.fetch("https://example.test/token"), /cooling down/);
  assert.equal(calls, 1);
  now = 120_000;
  await traffic.fetch("https://example.test/mcp", { method: "POST" });
  assert.equal(calls, 2);
});

test("inactive clients send no requests and explicit fresh reads have no polling budget", async t => {
  let active = false, calls = 0;
  t.mock.method(globalThis, "fetch", async () => { calls++; return new Response(""); });
  const traffic = new RemoteTraffic(() => 0, () => active);
  await assert.rejects(traffic.fetch("https://example.test"), /inactive/);
  active = true;
  for (let i = 0; i < 50; i++) await traffic.fetch("https://example.test", { method: "POST" });
  assert.equal(calls, 50);
});
