/** A bounded registry of authenticated MCP clients.  This records each client
 * once; it deliberately is not a heartbeat or a claim of a live connection.
 * No OAuth identity leaves the Durable Object.
 */
import relayProfile from "../../../contracts/public/qc-relay-profile.v1.json" with { type: "json" };
export const MCP_PRESENCE_CAPACITY = 128;
/** This is a client registry, not a request log. Rows are bounded by capacity
 * and retained until the associated OAuth grant is explicitly revoked. */
// Kept for the storage-inventory compatibility report. It does not schedule
// expiry or cause registry writes.
export const MCP_NAME_RETENTION_MS = 24 * 60 * 60 * 1_000;
export type McpClientPresence = {
  id: string;
  name: string;
  clientId?: string;
  /** The OAuth grant this session's access token was issued under, so a
   * paired host can revoke exactly this authorization. Taken from the token
   * the OAuth provider has just validated, never inferred from metadata. */
  grantId?: string;
  version?: string;
  device?: string;
  browser?: string;
  ipAddress?: string;
  country?: string;
  city?: string;
  connectedAtUnixMs: number;
  lastSeenAtUnixMs: number;
  expiresAtUnixMs: number;
};
export type StoredMcpPresence = McpClientPresence & { key: string; rememberedUntilUnixMs: number };

/** Keep every identity/grant in a bounded frame; descriptive metadata is optional. */
export function mcpPresenceFrame(clients: readonly McpClientPresence[]): string | null {
  const encoder = new TextEncoder();
  const encode = (rows: readonly unknown[]) => JSON.stringify({ type: "mcp_presence", clients: rows });
  // clientId is OAuth registration metadata, not the host's per-grant Unlink key.
  const full = encode(clients.map(({ clientId: _clientId, ...row }) => row));
  if (encoder.encode(full).byteLength <= relayProfile.maxRequestFrameBytes) return full;
  const compact = encode(clients.map(row => {
    let name = "";
    for (const character of row.name) {
      if (encoder.encode(name + character).byteLength > 128) break;
      name += character;
    }
    return { id: row.id, name, ...(row.grantId ? { grantId: row.grantId } : {}),
      connectedAtUnixMs: row.connectedAtUnixMs, lastSeenAtUnixMs: row.lastSeenAtUnixMs, expiresAtUnixMs: row.expiresAtUnixMs };
  }));
  // An unexpected oversized identity must not disconnect the host with an invalid frame.
  return encoder.encode(compact).byteLength <= relayProfile.maxRequestFrameBytes ? compact : null;
}
export type McpObservation = {
  key: string;
  clientId?: string | null;
  grantId?: string | null;
  name?: string | null;
  version?: string | null;
  fallbackName?: string | null;
  device?: string | null;
  browser?: string | null;
  ipAddress?: string | null;
  country?: string | null;
  city?: string | null;
  remove?: boolean;
};
export type InitializedClient = { name: string | null; version: string | null };

export type McpClientIdentityContext = {
  device?: string | null;
  browser?: string | null;
  clientUri?: string | null;
};

/** The OAuth provider's grant identifiers, and exactly what the connector
 * revoke route accepts. */
export const GRANT_ID_PATTERN = /^[A-Za-z0-9_-]{8,128}$/;

export function observedGrantId(value: unknown): string | undefined {
  return typeof value === "string" && GRANT_ID_PATTERN.test(value) ? value : undefined;
}

export function observedClientName(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const name = value.trim();
  return name.length > 0 && name.length <= 128 && !/[\x00-\x1f\x7f]/.test(name) ? name : null;
}

/** Turn protocol-level client labels into the product/surface name a person
 * recognises. OAuth registrations commonly identify only the vendor product
 * ("Claude", "ChatGPT"), while the authorization request supplies the surface
 * that actually linked it. Unknown clients keep their self-declared name; this
 * is deliberately a small allow-list rather than a vendor-guessing heuristic. */
export function displayMcpClientName(value: unknown, context: McpClientIdentityContext = {}): string | null {
  const observed = observedClientName(value);
  if (!observed) return null;
  const normalized = observed.toLocaleLowerCase();
  const device = observedClientName(context.device)?.toLocaleLowerCase() ?? "";
  const uri = observedClientName(context.clientUri)?.toLocaleLowerCase() ?? "";
  const mobile = device === "android" || device === "iphone";

  if (normalized.includes("antigravity")) {
    return /\b(mobile|android|ios|web|desktop)\b/.test(normalized) ? observed : "Antigravity desktop";
  }
  if (normalized.includes("chatgpt") || normalized === "openai") {
    if (/\b(mobile|android|ios)\b/.test(normalized)) return "ChatGPT Mobile";
    if (/\bdesktop\b/.test(normalized)) return "ChatGPT Desktop";
    // ChatGPT currently exposes custom remote MCP through its web surface.
    return "ChatGPT web";
  }
  if (normalized.includes("claude") || uri.includes("claude.ai")) {
    if (/\bdesktop\b/.test(normalized)) return "Claude Desktop";
    if (/\b(mobile|android|ios)\b/.test(normalized) || mobile) return "Claude Mobile";
    if (/\bweb\b/.test(normalized)) return "Claude web";
    return "Claude";
  }
  if (normalized.includes("gemini")) {
    if (/\b(mobile|android|ios)\b/.test(normalized)) return "Gemini Mobile";
    if (/\bdesktop\b/.test(normalized)) return "Gemini Desktop";
    if (/\bweb\b/.test(normalized)) return "Gemini web";
  }
  return observed;
}

export function initializedClientName(value: unknown): string | null {
  return initializedClient(value).name;
}

export function initializedClient(value: unknown): InitializedClient {
  if (typeof value !== "object" || value === null) return { name: null, version: null };
  const request = value as Record<string, unknown>;
  if (request.method !== "initialize") return { name: null, version: null };
  const params = request.params as Record<string, unknown> | undefined;
  const client = params?.clientInfo as Record<string, unknown> | undefined;
  return {
    name: observedClientName(client?.name),
    version: observedClientName(client?.version),
  };
}

function safeMetadata(value: unknown, max = 128): string | undefined {
  if (typeof value !== "string") return undefined;
  const text = value.trim();
  return text && text.length <= max && !/[\x00-\x1f\x7f]/.test(text) ? text : undefined;
}

/** A normalized, bounded description of the caller. Raw user agents are never stored. */
export function requestClientMetadata(request: Request): Pick<McpObservation, "device" | "browser" | "ipAddress" | "country" | "city"> {
  const agent = request.headers.get("user-agent") ?? "";
  const device = /Android/i.test(agent) ? "Android" : /iPhone|iPad|iPod/i.test(agent) ? "iPhone" : /Windows/i.test(agent) ? "Windows" : /Macintosh|Mac OS X/i.test(agent) ? "Mac" : /Linux/i.test(agent) ? "Linux" : "Web";
  const browserMatch = /EdgA?|EdgiOS/i.test(agent) ? ["Edge", agent.match(/(?:EdgA?|EdgiOS)\/([\d.]+)/i)?.[1]] : /Firefox|FxiOS/i.test(agent) ? ["Firefox", agent.match(/(?:Firefox|FxiOS)\/([\d.]+)/i)?.[1]] : /Chrome|CriOS/i.test(agent) ? ["Chrome", agent.match(/(?:Chrome|CriOS)\/([\d.]+)/i)?.[1]] : /Safari/i.test(agent) ? ["Safari", agent.match(/Version\/([\d.]+)/i)?.[1]] : ["browser", undefined];
  const cf = (request as Request & { cf?: Record<string, unknown> }).cf;
  return {
    device,
    browser: `${browserMatch[0]}${browserMatch[1] ? ` ${browserMatch[1]}` : ""}`,
    ipAddress: safeMetadata(request.headers.get("CF-Connecting-IP"), 64),
    country: safeMetadata(cf?.country, 2),
    city: safeMetadata(cf?.city, 64),
  };
}

export function updateMcpPresence(
  previous: readonly StoredMcpPresence[], observation: McpObservation | undefined,
  now: number, newId: () => string,
): StoredMcpPresence[] {
  const clients = [...previous];
  if (observation) {
    if (!/^[a-f0-9]{64}$/.test(observation.key)) throw new Error("Invalid private MCP activity key");
    const existing = clients.find(row => row.key === observation.key);
    const remaining = clients.filter(row => row.key !== observation.key);
    if (!observation.remove) {
      // A known client is intentionally immutable: a request must never become
      // a storage write. This avoids charging a Durable Object heartbeat.
      if (existing) return clients;
      const clientId = safeMetadata(observation.clientId);
      const grantId = observedGrantId(observation.grantId);
      const device = safeMetadata(observation.device);
      const browser = safeMetadata(observation.browser);
      const version = observedClientName(observation.version);
      const observedName = displayMcpClientName(observation.name, { device, browser })
        ?? displayMcpClientName(observation.fallbackName, { device, browser })
        ?? "Remote MCP client";
      remaining.push({
        key: observation.key,
        id: newId(),
        name: observedName,
        ...(clientId ? { clientId } : {}),
        ...(grantId ? { grantId } : {}),
        ...(version ? { version } : {}),
        ...(device ? { device } : {}),
        ...(browser ? { browser } : {}),
        ...(safeMetadata(observation.ipAddress, 64) ? { ipAddress: safeMetadata(observation.ipAddress, 64) } : {}),
        ...(safeMetadata(observation.country, 2) ? { country: safeMetadata(observation.country, 2) } : {}),
        ...(safeMetadata(observation.city, 64) ? { city: safeMetadata(observation.city, 64) } : {}),
        connectedAtUnixMs: now,
        lastSeenAtUnixMs: now,
        // These timestamps describe the single observation, not a lease.
        expiresAtUnixMs: now,
        rememberedUntilUnixMs: Number.MAX_SAFE_INTEGER,
      });
    }
    return remaining.sort((a, b) => b.expiresAtUnixMs - a.expiresAtUnixMs || a.id.localeCompare(b.id))
      .slice(0, MCP_PRESENCE_CAPACITY);
  }
  return clients;
}

/** Drop every row, live or remembered, of a grant that was just revoked, so
 * paired hosts stop listing it at once instead of when its lease lapses. */
export function forgetMcpGrant(rows: readonly StoredMcpPresence[], grantId: string): StoredMcpPresence[] {
  return rows.filter(row => row.grantId !== grantId);
}

export function publicMcpPresence(rows: readonly StoredMcpPresence[], now: number): McpClientPresence[] {
  void now;
  return rows
    .map(({ key: _key, rememberedUntilUnixMs: _remembered, ...row }) => ({
      ...row,
      connectedAtUnixMs: row.connectedAtUnixMs ?? row.expiresAtUnixMs,
      lastSeenAtUnixMs: row.lastSeenAtUnixMs ?? row.expiresAtUnixMs,
    }))
    .sort((a, b) => a.id.localeCompare(b.id));
}

/** Sanitized recent activity, including an expired live lease. The durable
 * connector roster uses this to say "last active" without pretending that an
 * OAuth authorization is a permanently connected session. */
export function rememberedMcpPresence(rows: readonly StoredMcpPresence[], now: number): McpClientPresence[] {
  void now;
  return rows
    .map(({ key: _key, rememberedUntilUnixMs: _remembered, ...row }) => ({ ...row }))
    .sort((a, b) => b.lastSeenAtUnixMs - a.lastSeenAtUnixMs || a.id.localeCompare(b.id));
}

export function nextPresenceExpiry(rows: readonly StoredMcpPresence[], now: number): number | null {
  void rows;
  void now;
  return null;
}

/** Read a bounded clone without consuming the handler's request. Cancelling a
 * tee branch waits for its sibling, so deliberately do not await cancellation. */
export async function requestInitializeClient(request: Request): Promise<InitializedClient> {
  const reader = request.clone().body?.getReader();
  if (!reader) return { name: null, version: null };
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const chunk = await reader.read();
      if (chunk.done) break;
      size += chunk.value.byteLength;
      if (size > 65_536) return { name: null, version: null };
      chunks.push(chunk.value);
    }
    const bytes = new Uint8Array(size);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    return initializedClient(JSON.parse(new TextDecoder().decode(bytes)));
  } catch { return { name: null, version: null }; }
  finally { void reader.cancel().catch(() => {}); }
}

export async function requestInitializeName(request: Request): Promise<string | null> {
  return (await requestInitializeClient(request)).name;
}
