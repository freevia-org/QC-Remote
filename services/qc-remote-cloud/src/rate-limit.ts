/** Per-IP rate limits for the routes anyone can call and that write to KV.
 *
 * Creating a pairing session, starting a pairing sign-in, opening the OAuth
 * consent screen and registering an OAuth client all need no credential, and
 * each one stores a record. Without a limit, one client can fill the namespace
 * and spend the account's KV write allowance for everybody.
 *
 * The limits use Cloudflare's Workers rate-limiting binding, declared under
 * `ratelimits` in wrangler.jsonc. Its counters are per Cloudflare location and
 * eventually consistent, which is the right trade for abuse protection: it
 * costs no KV write of its own and adds no latency worth measuring.
 *
 * A missing binding fails open only in explicit private mode, so local
 * compatibility tests run without one; a production deployment that lost its
 * binding refuses these routes rather than quietly running unprotected. */

/** The part of Cloudflare's `RateLimit` binding this file uses. */
import { relayEnvironment } from "./owner-key.ts";
import { DEVICE_PAIR_REPLACE_PATH } from "./device-retention.ts";
import relayProfile from "../../../contracts/public/qc-relay-profile.v1.json" with { type: "json" };

export type RateLimitBinding = { limit(options: { key: string }): Promise<{ success: boolean }> };

export type RateLimitEnv = {
  QCRC_ENVIRONMENT?: string;
  QCRC_PAIRING_RATE_LIMIT?: RateLimitBinding;
  QCRC_OAUTH_RATE_LIMIT?: RateLimitBinding;
  QCRC_MCP_RATE_LIMIT?: RateLimitBinding;
};

export type RateLimitedRoute = "mcp" | "pairing-session" | "pairing-sign-in" | "pairing-cancel" | "pairing-device" | "pairing-replace" | "account-delete-sign-in" | "account-delete-confirm" | "oauth-authorize" | "oauth-register";

/** Which limit, if any, applies to this request.
 *
 * The binding is chosen per family so each carries its own budget in
 * wrangler.jsonc; the route is part of the key so one family's routes do not
 * spend each other's allowance. */
export function rateLimitedRoute(request: Request, url = new URL(request.url)): { route: RateLimitedRoute; binding: "QCRC_PAIRING_RATE_LIMIT" | "QCRC_OAUTH_RATE_LIMIT" | "QCRC_MCP_RATE_LIMIT" } | null {
  const { pathname } = url;
  // Stop polling storms before OAuth KV reads and account DO admission.
  // Include the origin alias used by Claude Mobile and all MCP HTTP methods.
  if ((pathname === "/mcp" || pathname === "/") && ["POST", "GET", "DELETE"].includes(request.method) && (pathname === "/mcp" || request.method === "POST")) return { route: "mcp", binding: "QCRC_MCP_RATE_LIMIT" };
  if (request.method === "POST" && pathname === "/v1/host/pairing-sessions") return { route: "pairing-session", binding: "QCRC_PAIRING_RATE_LIMIT" };
  if (request.method === "POST" && pathname === "/pair/sign-in") return { route: "pairing-sign-in", binding: "QCRC_PAIRING_RATE_LIMIT" };
  if (request.method === "POST" && pathname === "/pair/cancel") return { route: "pairing-cancel", binding: "QCRC_PAIRING_RATE_LIMIT" };
  // The ordinary pair route. Each request that reaches admission can reserve
  // capacity in the owner's Durable Object (device-retention.ts
  // admitPairingAttempt), so it carries the pairing family's limit too.
  if (request.method === "POST" && pathname === relayProfile.devicePairPath) return { route: "pairing-device", binding: "QCRC_PAIRING_RATE_LIMIT" };
  // The two-phase re-pair route (device-retention.ts DEVICE_PAIR_REPLACE_PATH).
  if (request.method === "POST" && pathname === DEVICE_PAIR_REPLACE_PATH) return { route: "pairing-replace", binding: "QCRC_PAIRING_RATE_LIMIT" };
  // Remote account deletion: the sign-in start writes KV, and confirm and
  // continue run the deletion (account-deletion-web.ts).
  if (request.method === "POST" && pathname === "/account/delete/sign-in") return { route: "account-delete-sign-in", binding: "QCRC_PAIRING_RATE_LIMIT" };
  if (request.method === "POST" && (pathname === "/account/delete/confirm" || pathname === "/account/delete/continue")) return { route: "account-delete-confirm", binding: "QCRC_PAIRING_RATE_LIMIT" };
  if (request.method === "GET" && pathname === "/authorize") return { route: "oauth-authorize", binding: "QCRC_OAUTH_RATE_LIMIT" };
  if (request.method === "POST" && pathname === "/register") return { route: "oauth-register", binding: "QCRC_OAUTH_RATE_LIMIT" };
  return null;
}

/** The caller's address as Cloudflare reports it. Absent only off the edge. */
export function clientAddress(request: Request): string {
  const address = request.headers.get("cf-connecting-ip")?.trim();
  return address && address.length <= 64 ? address : "unknown";
}

function tooManyRequests(): Response {
  return new Response(JSON.stringify({ error: "Too many requests. Wait a minute and try again." }), {
    status: 429,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store", "retry-after": "60", "access-control-allow-origin": "*", "access-control-expose-headers": "Retry-After" }
  });
}

/** A response refusing the request, or null when it may proceed. */
export async function enforceRateLimit(request: Request, env: RateLimitEnv, log: (line: string) => void = (line) => console.error(line)): Promise<Response | null> {
  const limited = rateLimitedRoute(request);
  if (!limited) return null;
  const binding = env[limited.binding];
  if (!binding) {
    if (relayEnvironment(env.QCRC_ENVIRONMENT) === "private") return null;
    log(JSON.stringify({ event: "rate-limit-binding-missing", binding: limited.binding, route: limited.route }));
    return new Response(JSON.stringify({ error: "This service is temporarily unavailable." }), {
      status: 503,
      headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" }
    });
  }
  try {
    const { success } = await binding.limit({ key: `${limited.route}:${clientAddress(request)}` });
    return success ? null : tooManyRequests();
  } catch (cause) {
    // A configured limiter outage must not silently remove the abuse barrier.
    // Keep the diagnostic in the logs, but fail closed with a generic response.
    log(JSON.stringify({ event: "rate-limit-error", binding: limited.binding, route: limited.route, detail: cause instanceof Error ? cause.name : "unknown" }));
    return new Response(JSON.stringify({ error: "This service is temporarily unavailable." }), {
      status: 503,
      headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" }
    });
  }
}
