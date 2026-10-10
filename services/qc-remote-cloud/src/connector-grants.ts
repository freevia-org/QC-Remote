import { ownerSubject, type RelayEnvironment } from "./owner-key.ts";
import { displayMcpClientName, observedGrantId, type McpClientPresence } from "./mcp-presence.ts";

/** The part of the OAuth provider this file needs, and nothing more.
 *
 * Declared here rather than imported so the rule below can be tested against a
 * fake: the real helpers arrive on `env` only inside a request the provider
 * wraps, which is not reachable from a unit test. */
export type ConnectorGrantRegistry = {
  listUserGrants(userId: string, options?: { cursor?: string }): Promise<{ items: ConnectorGrantSummary[]; cursor?: string }>;
  revokeGrant(grantId: string, userId: string): Promise<void>;
  lookupClient?(clientId: string): Promise<{ clientName?: string; clientUri?: string } | null>;
};

export type ConnectorGrantSummary = {
  id: string;
  clientId: string;
  scope: string[];
  metadata?: {
    clientName?: string;
    authorizedFrom?: string;
    device?: string;
    browser?: string;
    ipAddress?: string;
    country?: string;
    city?: string;
    /** The account generation the grant was admitted under (account-deletion.ts). */
    accountGeneration?: number;
  };
  createdAt: number;
  expiresAt?: number;
  redirectUri?: string;
};

export type LinkedConnector = {
  grantId: string;
  clientId: string;
  name: string;
  scopes: string[];
  authorizedAt: number;
  expiresAt?: number;
  authorizedFrom?: string;
  device?: string;
  browser?: string;
  ipAddress?: string;
  country?: string;
  city?: string;
  clientUri?: string;
  active?: boolean;
  lastActiveAt?: number;
  appVersion?: string;
  activityDevice?: string;
  activityBrowser?: string;
  activityIpAddress?: string;
  activityCountry?: string;
  activityCity?: string;
};

/** How this service files an AI app's grant against the account that made it.
 *
 * One definition, used by the code that creates grants and the code that
 * retires them. They were written apart once and only the creating side
 * encoded the subject, so a subject containing a reserved character produced a
 * user id that nothing could ever look up again. */
export function connectorGrantUser(subject: string): string {
  return `access-${encodeURIComponent(subject)}`;
}

/** The grant an authenticated MCP request was made under, or null.
 *
 * Call only with the bearer token the OAuth provider has just accepted for
 * this request. The provider issues every access token as
 * `<userId>:<grantId>:<secret>` and accepts one only after finding its record
 * under exactly that user and grant (`token:<userId>:<grantId>:<hash>`), so
 * the middle segment of an accepted token *is* its grant: no lookup, no
 * matching by client, device or time. A user id never contains a colon (the
 * provider refuses one), so the split is unambiguous.
 *
 * Belt and braces: the user segment must be the one this service files the
 * signed-in account's grants under, and the grant must have the provider's
 * shape. Anything else yields null, and the session is published without a
 * grant, which the paired host treats as "cannot unlink from here". */
export function accessTokenGrantId(token: string | null | undefined, subject: string): string | null {
  if (!token) return null;
  const parts = token.split(":");
  if (parts.length !== 3) return null;
  const [userId, grantId] = parts;
  return userId === connectorGrantUser(subject) ? observedGrantId(grantId) ?? null : null;
}

/** List the durable AI-app authorizations for one Google account.
 *
 * These are OAuth grants, not transient MCP requests and not other QC Remote
 * installations. The provider deliberately returns no token or secret. */
export async function listConnectorGrants(
  registry: ConnectorGrantRegistry,
  subject: string,
  activity: readonly McpClientPresence[] = [],
  now = Date.now(),
): Promise<LinkedConnector[]> {
  const userId = connectorGrantUser(subject);
  const grants: ConnectorGrantSummary[] = [];
  let cursor: string | undefined;
  do {
    const page = await registry.listUserGrants(userId, cursor ? { cursor } : undefined);
    grants.push(...page.items);
    cursor = page.cursor;
  } while (cursor);
  const clients = await Promise.all(grants.map((grant) => registry.lookupClient?.(grant.clientId).catch(() => null) ?? null));
  return grants.map((grant, index) => {
    const client = clients[index];
    const registeredName = client?.clientName?.trim() || grant.metadata?.clientName?.trim() || "Remote MCP app";
    // A session that names its grant belongs to that grant alone; only a row
    // recorded before grants were observed falls back to the client id.
    const recent = activity
      .filter((row) => row.grantId ? row.grantId === grant.id : row.clientId === grant.clientId)
      .sort((left, right) => right.lastSeenAtUnixMs - left.lastSeenAtUnixMs)[0];
    return {
      grantId: grant.id,
      clientId: grant.clientId,
      name: displayMcpClientName(registeredName, {
        device: grant.metadata?.device,
        browser: grant.metadata?.browser,
        clientUri: client?.clientUri,
      }) ?? "Remote MCP app",
      scopes: grant.scope,
      authorizedAt: grant.createdAt * 1_000,
      ...(grant.expiresAt ? { expiresAt: grant.expiresAt * 1_000 } : {}),
      ...(grant.metadata?.authorizedFrom ? { authorizedFrom: grant.metadata.authorizedFrom } : {}),
      ...(grant.metadata?.device ? { device: grant.metadata.device } : {}),
      ...(grant.metadata?.browser ? { browser: grant.metadata.browser } : {}),
      ...(grant.metadata?.ipAddress ? { ipAddress: grant.metadata.ipAddress } : {}),
      ...(grant.metadata?.country ? { country: grant.metadata.country } : {}),
      ...(grant.metadata?.city ? { city: grant.metadata.city } : {}),
      ...(client?.clientUri ? { clientUri: client.clientUri } : {}),
      ...(recent ? {
        active: recent.expiresAtUnixMs > now,
        lastActiveAt: recent.lastSeenAtUnixMs,
        ...(recent.version ? { appVersion: recent.version } : {}),
        ...(recent.device ? { activityDevice: recent.device } : {}),
        ...(recent.browser ? { activityBrowser: recent.browser } : {}),
        ...(recent.ipAddress ? { activityIpAddress: recent.ipAddress } : {}),
        ...(recent.country ? { activityCountry: recent.country } : {}),
        ...(recent.city ? { activityCity: recent.city } : {}),
      } : {})
    };
  }).sort((left, right) => right.authorizedAt - left.authorizedAt);
}

/** Retire every AI-app grant held by an account whose last device has gone.
 *
 * A connector's grant and a paired computer are separate credentials joined
 * only by the account, so re-pairing a computer to a second account leaves the
 * first account's tokens valid - and pointing at a namespace with nothing in
 * it. The app goes on reporting itself connected, because it is, and the
 * mismatch first appears as a command failing for reasons the person has no
 * way to connect to what they did.
 *
 * Retiring them turns that into the app asking to sign in again, which is the
 * one moment the account chooser appears and the right account can be picked.
 *
 * Deliberately only at zero. An account that still holds another device is an
 * account whose apps still work, and revoking there would sign people out of a
 * connector that had nothing wrong with it.
 *
 * Single-owner compatibility mode collapses every account onto one namespace and keeps
 * no subject to map back to, so it retires nothing rather than guess at whose
 * grants these are.
 *
 * Never rejects. By the time this runs the device is already gone, and a
 * person who asked to unpair must not be told it did not happen because a
 * second system was briefly unreachable. A grant that survives leaves exactly
 * the behaviour that existed before this was written: the app keeps a token for
 * an empty account, and its next command explains that precisely.
 *
 * Returns how many were actually revoked, which is what a partial run achieved
 * rather than what it set out to do. */
export async function retireGrantsForEmptiedAccount(
  registry: ConnectorGrantRegistry,
  environment: RelayEnvironment,
  ownerKey: string,
  remainingDevices: number
): Promise<number> {
  if (remainingDevices > 0) return 0;
  const subject = ownerSubject(environment, ownerKey);
  if (!subject) return 0;
  const userId = connectorGrantUser(subject);

  let revoked = 0;
  try {
    let cursor: string | undefined;
    // Paginated because the list is the provider's, not ours: an account may
    // have authorised more apps than one page returns, and stopping at the
    // first page would leave exactly the stale grants this exists to remove.
    do {
      const page = await registry.listUserGrants(userId, cursor ? { cursor } : undefined);
      for (const grant of page.items) {
        await registry.revokeGrant(grant.id, userId);
        revoked += 1;
      }
      cursor = page.cursor;
    } while (cursor);
  } catch {
    // Deliberately swallowed; see above. `revoked` still reports what landed.
  }
  return revoked;
}
