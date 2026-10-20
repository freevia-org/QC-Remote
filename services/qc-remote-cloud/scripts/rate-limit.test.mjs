import assert from "node:assert/strict";
import test from "node:test";
import { clientAddress, enforceRateLimit, rateLimitedRoute } from "../src/rate-limit.ts";

const request = (path, method = "POST", address = "192.0.2.7") => new Request(`https://relay.example${path}`, {
  method,
  headers: { "cf-connecting-ip": address }
});

test("MCP transports and the root POST alias share one limiter family", () => {
  const cases = [["/mcp", "POST"], ["/mcp", "GET"], ["/mcp", "DELETE"], ["/", "POST"]];
  for (const [path, method] of cases) {
    assert.deepEqual(rateLimitedRoute(request(path, method)), { route: "mcp", binding: "QCRC_MCP_RATE_LIMIT" });
  }
});

test("preflight, root navigation and unrelated paths do not spend the MCP budget", () => {
  const cases = [["/mcp", "OPTIONS"], ["/mcp/", "POST"], ["/", "GET"], ["/", "DELETE"], ["/health", "GET"]];
  for (const [path, method] of cases) assert.equal(rateLimitedRoute(request(path, method)), null);
});

test("each allowed MCP request charges once using the shared address key", async () => {
  const keys = [];
  const env = { QCRC_ENVIRONMENT: "production", QCRC_MCP_RATE_LIMIT: { async limit({ key }) { keys.push(key); return { success: true }; } } };
  assert.equal(await enforceRateLimit(request("/mcp", "POST", " 192.0.2.7 "), env), null);
  assert.equal(await enforceRateLimit(request("/", "POST"), env), null);
  assert.deepEqual(keys, ["mcp:192.0.2.7", "mcp:192.0.2.7"]);
});

test("MCP refusal exposes the existing retry contract without caching", async () => {
  let calls = 0;
  const response = await enforceRateLimit(request("/mcp"), { QCRC_ENVIRONMENT: "production", QCRC_MCP_RATE_LIMIT: {
    async limit() { calls += 1; return { success: false }; }
  } });
  assert.equal(calls, 1);
  assert.equal(response.status, 429);
  assert.equal(response.headers.get("retry-after"), "60");
  assert.equal(response.headers.get("cache-control"), "no-store");
  assert.equal(response.headers.get("access-control-allow-origin"), "*");
  assert.equal(response.headers.get("access-control-expose-headers"), "Retry-After");
  assert.deepEqual(await response.json(), { error: "Too many requests. Wait a minute and try again." });
});

test("missing production MCP binding and binding outages fail closed", async () => {
  const logs = [];
  const missing = await enforceRateLimit(request("/mcp"), { QCRC_ENVIRONMENT: "production" }, line => logs.push(JSON.parse(line)));
  assert.equal(missing.status, 503);
  assert.equal(logs[0].event, "rate-limit-binding-missing");
  assert.equal(logs[0].binding, "QCRC_MCP_RATE_LIMIT");
  const failed = await enforceRateLimit(request("/mcp"), { QCRC_ENVIRONMENT: "production", QCRC_MCP_RATE_LIMIT: {
    async limit() { throw new Error("not exposed"); }
  } }, line => logs.push(JSON.parse(line)));
  assert.equal(failed.status, 503);
  assert.equal(failed.headers.get("retry-after"), null);
  assert.deepEqual(await failed.json(), { error: "This service is temporarily unavailable." });
  assert.equal(logs[1].event, "rate-limit-error");
  assert.equal(await enforceRateLimit(request("/mcp"), { QCRC_ENVIRONMENT: "private" }), null);
});

test("pairing and OAuth retain separate bindings and budgets", async () => {
  const keys = [];
  const env = { QCRC_ENVIRONMENT: "production", QCRC_MCP_RATE_LIMIT: { async limit() { assert.fail("wrong family"); } },
    QCRC_PAIRING_RATE_LIMIT: { async limit({ key }) { keys.push(["pair", key]); return { success: true }; } },
    QCRC_OAUTH_RATE_LIMIT: { async limit({ key }) { keys.push(["oauth", key]); return { success: true }; } }
  };
  assert.equal(await enforceRateLimit(request("/v1/host/pairing-sessions"), env), null);
  assert.equal(await enforceRateLimit(request("/authorize", "GET"), env), null);
  assert.equal(await enforceRateLimit(request("/register"), env), null);
  assert.deepEqual(keys, [["pair", "pairing-session:192.0.2.7"], ["oauth", "oauth-authorize:192.0.2.7"], ["oauth", "oauth-register:192.0.2.7"]]);
});

test("missing or oversized edge addresses use the existing unknown bucket", () => {
  assert.equal(clientAddress(new Request("https://relay.example/mcp")), "unknown");
  assert.equal(clientAddress(request("/mcp", "POST", "x".repeat(65))), "unknown");
});
