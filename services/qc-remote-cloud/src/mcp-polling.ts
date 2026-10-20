/** Best-effort edge backpressure for repeated observational reads, before
 * OAuth/KV/DO work. This is not an authentication cache or a global quota:
 * isolates may be evicted. No tokens, responses or request objects are kept.
 * Every admitted request still passes the ordinary OAuth and account fences. */
export const IDLE_POLL_INTERVAL_MS = 15_000;
export const ACTIVE_POLL_INTERVAL_MS = 1_000;
export const CONTROL_ACTIVITY_MS = 30_000;
const ENTRY_TTL_MS = 120_000;
const MAX_CLIENTS = 2_048;
const MAX_ENVELOPE_BYTES = 16_384;
type Poll = "status" | "preset";
type Client = { touched: number; activeUntil: number; next: Partial<Record<Poll, number>> };

export class McpPollingBudget {
  private clients = new Map<string, Client>();
  get size(): number { return this.clients.size; }

  private client(key: string, now: number): Client {
    const previous = this.clients.get(key);
    this.clients.delete(key);
    const value = previous && now - previous.touched < ENTRY_TTL_MS
      ? previous : { touched: now, activeUntil: 0, next: {} };
    value.touched = now;
    this.clients.set(key, value);
    while (this.clients.size > MAX_CLIENTS) this.clients.delete(this.clients.keys().next().value!);
    return value;
  }

  /** Returns the remaining delay; refused attempts never extend it. */
  reserve(key: string, poll: Poll, now = Date.now()): number {
    const client = this.client(key, now);
    const remaining = (client.next[poll] ?? 0) - now;
    if (remaining > 0) return remaining;
    client.next[poll] = now + (client.activeUntil > now ? ACTIVE_POLL_INTERVAL_MS : IDLE_POLL_INTERVAL_MS);
    return 0;
  }

  /** An authorized control attempt needs immediate fresh readback, including
   * after an unknown outcome. Never serve an old snapshot in its place. */
  control(key: string, now = Date.now()): void {
    const client = this.client(key, now);
    client.activeUntil = now + CONTROL_ACTIVITY_MS;
    client.next = {};
  }

  forget(key: string): void { this.clients.delete(key); }
}

/** Only these frequently polled observations are slowed. Writes, OAuth,
 * initialization, catalogs, subscriptions and other reads are unchanged. */
export function observationalPoll(value: unknown): { poll: Poll; id: string | number } | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const envelope = value as { jsonrpc?: unknown; id?: unknown; method?: unknown; params?: unknown };
  if (envelope.jsonrpc !== "2.0" || (typeof envelope.id !== "string" && typeof envelope.id !== "number")) return null;
  if (!envelope.params || typeof envelope.params !== "object" || Array.isArray(envelope.params)) return null;
  const params = envelope.params as Record<string, unknown>;
  let poll: Poll | undefined;
  if (envelope.method === "resources/read") {
    if (params.uri === "qc://status") poll = "status";
    if (params.uri === "qc://current-preset") poll = "preset";
  } else if (envelope.method === "tools/call") {
    if (params.name === "get_qc_status") poll = "status";
    if (params.name === "get_current_preset") poll = "preset";
    if (params.name === "execute_qc_action" && params.arguments && typeof params.arguments === "object") {
      const action = (params.arguments as Record<string, unknown>).action;
      if (action === "get_status") poll = "status";
      if (action === "get_current_preset") poll = "preset";
    }
  }
  return poll ? { poll, id: envelope.id } : null;
}

async function envelope(request: Request): Promise<unknown> {
  const reader = request.clone().body?.getReader();
  if (!reader) return null;
  const chunks: Uint8Array[] = [];
  let size = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const expired = new Promise<null>((resolve) => { timer = setTimeout(() => resolve(null), 1_000); });
  try {
    while (true) {
      const chunk = await Promise.race([reader.read(), expired]);
      if (chunk === null) return null;
      if (chunk.done) break;
      size += chunk.value.byteLength;
      if (size > MAX_ENVELOPE_BYTES) return null;
      chunks.push(chunk.value);
    }
    const bytes = new Uint8Array(size);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    return JSON.parse(new TextDecoder().decode(bytes));
  } catch { return null; }
  finally {
    if (timer !== undefined) clearTimeout(timer);
    // A tee cancellation waits for its sibling: do not wait on it here.
    void reader.cancel().catch(() => {});
  }
}

export async function mcpPollingKey(request: Request): Promise<string | null> {
  const match = /^Bearer ([^\s]{1,8192})$/i.exec(request.headers.get("authorization") ?? "");
  if (!match) return null;
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(match[1]));
  return Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, "0")).join("");
}

/** A standard 429 lets a polling client obey Retry-After without performing
 * any OAuth KV read or waking a Durable Object for the rejected repetition. */
export async function limitMcpPolling(request: Request, budget: McpPollingBudget, now = Date.now()): Promise<Response | null> {
  if (request.method !== "POST" || new URL(request.url).pathname !== "/mcp") return null;
  const key = await mcpPollingKey(request);
  if (!key) return null;
  const read = observationalPoll(await envelope(request));
  if (!read) return null;
  const delay = budget.reserve(key, read.poll, now);
  if (!delay) return null;
  const origin = request.headers.get("origin");
  return new Response(JSON.stringify({ jsonrpc: "2.0", id: read.id, error: {
    code: -32029,
    message: "Status polling is too frequent. Wait before requesting another fresh read.",
    data: { retryAfterMs: delay },
  } }), { status: 429, headers: {
    "content-type": "application/json; charset=utf-8", "cache-control": "no-store",
    "retry-after": String(Math.ceil(delay / 1_000)),
    // The OAuth provider normally supplies these; this early response must
    // also let browser clients read Retry-After rather than fail CORS.
    ...(origin ? { "access-control-allow-origin": origin, "access-control-expose-headers": "Retry-After", vary: "Origin" } : {}),
  } });
}
