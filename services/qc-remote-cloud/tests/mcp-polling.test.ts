import assert from "node:assert/strict";
import test from "node:test";
import {
  McpPollingBudget, observationalPoll, limitMcpPolling, mcpPollingKey,
  IDLE_POLL_INTERVAL_MS, ACTIVE_POLL_INTERVAL_MS, CONTROL_ACTIVITY_MS,
} from "../src/mcp-polling.ts";

const call = (name: string, id: number | string = 1, args: object = {}) => ({
  jsonrpc: "2.0", id, method: "tools/call", params: { name, arguments: args },
});
const request = (value: unknown, token = "test-token-a", path = "/mcp") => new Request(`https://relay.example.test${path}`, {
  method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json" }, body: JSON.stringify(value),
});

test("idle poll flood is bounded without moving the deadline on refusals", () => {
  const budget = new McpPollingBudget();
  let accepted = 0;
  for (let now = 0; now < 60_000; now += 100) if (budget.reserve("client-a", "status", now) === 0) accepted++;
  assert.equal(accepted, 4); // 600 requests -> 4 OAuth/DO admissions, no result cache.
  assert.equal(budget.reserve("client-a", "status", 60_000), 0);
  assert.equal(budget.reserve("client-a", "status", 60_001), IDLE_POLL_INTERVAL_MS - 1);
});

test("control readback is immediate and idle polling resumes after activity ends", () => {
  const budget = new McpPollingBudget();
  assert.equal(budget.reserve("a", "preset", 1), 0);
  assert.ok(budget.reserve("a", "preset", 2) > 0);
  budget.control("a", 3);
  assert.equal(budget.reserve("a", "preset", 3), 0);
  assert.equal(budget.reserve("a", "preset", 4), ACTIVE_POLL_INTERVAL_MS - 1);
  assert.equal(budget.reserve("a", "preset", 3 + CONTROL_ACTIVITY_MS), 0);
  assert.equal(budget.reserve("a", "preset", 4 + CONTROL_ACTIVITY_MS), IDLE_POLL_INTERVAL_MS - 1);
});

test("different callers and read families do not spend each other's slots", () => {
  const budget = new McpPollingBudget();
  assert.equal(budget.reserve("a", "status", 1), 0);
  assert.equal(budget.reserve("a", "preset", 1), 0);
  assert.equal(budget.reserve("b", "status", 1), 0);
  assert.ok(budget.reserve("a", "status", 2) > 0);
  budget.forget("a");
  assert.equal(budget.reserve("a", "status", 2), 0);
});

test("counter storage is bounded and an expired caller starts fresh", () => {
  const budget = new McpPollingBudget();
  for (let id = 0; id < 4_000; id++) budget.reserve(String(id), "status", 1);
  assert.equal(budget.size, 2_048);
  assert.equal(budget.reserve("3999", "status", 120_001), 0);
});

test("only status/preset polls are classified; mutations and protocol traffic pass", () => {
  assert.deepEqual(observationalPoll(call("get_qc_status")), { poll: "status", id: 1 });
  assert.deepEqual(observationalPoll(call("get_current_preset", "r")), { poll: "preset", id: "r" });
  assert.deepEqual(observationalPoll(call("execute_qc_action", 2, { action: "get_status" })), { poll: "status", id: 2 });
  assert.deepEqual(observationalPoll({ jsonrpc: "2.0", id: 3, method: "resources/read", params: { uri: "qc://current-preset" } }), { poll: "preset", id: 3 });
  for (const value of [call("set_tempo"), call("capture_screen"), call("execute_qc_action", 1, { action: "set_tempo" }), call("list_qc_actions"),
    { jsonrpc: "2.0", id: 1, method: "initialize", params: {} },
    { jsonrpc: "2.0", id: 1, method: "tools/list", params: {} },
    { jsonrpc: "2.0", method: "notifications/initialized", params: {} },
    { jsonrpc: "2.0", id: 1, method: "resources/subscribe", params: { uri: "qc://status" } },
    [call("get_qc_status")], null]) assert.equal(observationalPoll(value), null);
});

test("429 preserves RPC id, Retry-After and the request body; it skips provider work", async () => {
  const budget = new McpPollingBudget();
  let providerReads = 0;
  async function route(now: number) {
    const req = request(call("get_qc_status", "read-1"));
    const denied = await limitMcpPolling(req, budget, now);
    assert.deepEqual(await req.json(), call("get_qc_status", "read-1"));
    if (denied) return denied;
    providerReads++;
    return Response.json({ result: "fresh, fully authenticated" });
  }
  assert.equal((await route(1)).status, 200);
  const denied = await route(1_001);
  assert.equal(denied.status, 429);
  assert.equal(denied.headers.get("retry-after"), "14");
  assert.equal(denied.headers.get("cache-control"), "no-store");
  assert.deepEqual(await denied.json(), { jsonrpc: "2.0", id: "read-1", error: {
    code: -32029, message: "Status polling is too frequent. Wait before requesting another fresh read.", data: { retryAfterMs: 14_000 },
  } });
  assert.equal(providerReads, 1);
});

test("control, login, refresh and revocation are not poll-limited", async () => {
  const budget = new McpPollingBudget();
  await limitMcpPolling(request(call("get_qc_status")), budget, 1);
  for (let now = 2; now < 12; now++) {
    assert.equal(await limitMcpPolling(request(call("set_tempo", now, { tempo: 120 })), budget, now), null);
    assert.equal(await limitMcpPolling(request({ grant_type: "refresh_token" }, "a", "/token"), budget, now), null);
    assert.equal(await limitMcpPolling(request({}, "a", "/authorize"), budget, now), null);
  }
});

test("browser clients can read Retry-After on the early response", async () => {
  const budget = new McpPollingBudget();
  await limitMcpPolling(request(call("get_qc_status")), budget, 1);
  const browserRequest = request(call("get_qc_status"));
  browserRequest.headers.set("origin", "https://client.example.test");
  const denied = await limitMcpPolling(browserRequest, budget, 2);
  assert.equal(denied?.status, 429);
  assert.equal(denied.headers.get("access-control-allow-origin"), "https://client.example.test");
  assert.equal(denied.headers.get("access-control-expose-headers"), "Retry-After");
});

test("fingerprints are opaque and separate credentials; no response is cached", async () => {
  const a = await mcpPollingKey(request(call("get_qc_status")));
  const b = await mcpPollingKey(request(call("get_qc_status"), "different-token"));
  assert.match(a!, /^[a-f0-9]{64}$/);
  assert.notEqual(a, b);
  const budget = new McpPollingBudget();
  assert.equal(await limitMcpPolling(request(call("get_qc_status")), budget, 0), null);
  assert.equal(await limitMcpPolling(request(call("get_qc_status"), "different-token"), budget, 1), null);
  assert.equal(await limitMcpPolling(request(call("get_qc_status")), budget, 15_000), null);
});

test("invalid and oversized envelopes leave the original stream available", async () => {
  const budget = new McpPollingBudget();
  const huge = request(call("get_qc_status", 1, { extra: "x".repeat(20_000) }));
  assert.equal(await limitMcpPolling(huge, budget), null);
  assert.ok((await huge.text()).length > 20_000);
  const broken = new Request("https://relay.example.test/mcp", { method: "POST", headers: { authorization: "Bearer test" }, body: "{" });
  assert.equal(await limitMcpPolling(broken, budget), null);
  assert.equal(await broken.text(), "{");
});

test("an unfinished inspection stream times out and cancels its clone", { timeout: 3_000 }, async () => {
  let cancelled = false;
  const body = new ReadableStream({
    start(controller) { controller.enqueue(new TextEncoder().encode("{")); },
    cancel() { cancelled = true; },
  });
  const incoming = new Request("https://relay.example.test/mcp", {
    method: "POST", headers: { authorization: "Bearer test" }, body, duplex: "half",
  } as RequestInit);
  const budget = new McpPollingBudget();
  try {
    assert.equal(await limitMcpPolling(incoming, budget), null);
    assert.equal(budget.size, 0);
  } finally { await incoming.body!.cancel(); }
  assert.equal(cancelled, true);
});
