/** The gate in front of the `QCRC_ADMIN_TOKEN` routes.
 *
 * Those routes - creating a single-owner pairing offer, listing every paired
 * device with its owner's email, reading the compatibility namespace and
 * sending a device command - exist for the single-owner compatibility
 * deployment and for bootstrapping one. In production every device belongs to
 * a Google account and is reached through that account's own namespace, so the
 * routes there are nothing but a static bearer secret standing between the
 * internet and every account's device directory. They do not exist in
 * production: the gate answers 404 whether or not a token is configured or
 * presented, so the deployment does not even confirm they are there. */

export type AdminEnv = { QCRC_ENVIRONMENT?: string; QCRC_ADMIN_TOKEN?: string };
import { relayEnvironment } from "./owner-key.ts";

function json(value: Record<string, unknown>, status: number): Response {
  return new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" } });
}

function bearer(request: Request): string | null {
  const match = request.headers.get("authorization")?.match(/^Bearer\s+(.+)$/i);
  return match?.[1]?.trim() || null;
}

/** Compare two secrets by their digests, in time independent of where they differ. */
async function digestsEqual(left: string, right: string): Promise<boolean> {
  const encoder = new TextEncoder();
  const [a, b] = await Promise.all([
    crypto.subtle.digest("SHA-256", encoder.encode(left)),
    crypto.subtle.digest("SHA-256", encoder.encode(right))
  ]);
  const x = new Uint8Array(a);
  const y = new Uint8Array(b);
  let difference = 0;
  for (let index = 0; index < x.length; index += 1) difference |= x[index] ^ y[index];
  return difference === 0;
}

export function adminRoutesEnabled(env: AdminEnv): boolean {
  return relayEnvironment(env.QCRC_ENVIRONMENT) === "private";
}

/** A response refusing the request, or null when it carries the admin token
 * in a deployment that has admin routes at all. */
export async function adminGate(request: Request, env: AdminEnv): Promise<Response | null> {
  if (!adminRoutesEnabled(env)) return json({ error: "Not found" }, 404);
  const token = bearer(request);
  if (!env.QCRC_ADMIN_TOKEN || !token) return json({ error: "Unauthorized" }, 401);
  if (!await digestsEqual(token, env.QCRC_ADMIN_TOKEN)) return json({ error: "Unauthorized" }, 401);
  return null;
}
