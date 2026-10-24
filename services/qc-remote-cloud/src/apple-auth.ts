import { PublicError, fetchIdentityJson } from "./http-security.ts";
import { QC_COLORS } from "@qc-remote/theme/server";

export type IdentityProvider = "google" | "apple";
export interface AppleOAuthEnv {
  APPLE_CLIENT_ID?: string;
  APPLE_APP_ID?: string;
  APPLE_TEAM_ID?: string;
  APPLE_KEY_ID?: string;
  APPLE_PRIVATE_KEY?: string;
}
export const appleSignInEnabled = (env: AppleOAuthEnv): boolean =>
  Boolean(env.APPLE_APP_ID && env.APPLE_CLIENT_ID && env.APPLE_TEAM_ID && env.APPLE_KEY_ID && env.APPLE_PRIVATE_KEY);
export function requestedIdentityProvider(value: unknown, env: AppleOAuthEnv): IdentityProvider {
  if (value == null || value === "" || value === "google") return "google";
  if (value === "apple" && appleSignInEnabled(env)) return "apple";
  throw new PublicError("This sign-in method is not available. Choose another method.");
}
export function appleSignInButton(env: AppleOAuthEnv): string {
  return appleSignInEnabled(env) ? `<button class="button" style="background:${QC_COLORS.screenUi.textPrimary};color:${QC_COLORS.screenUi.background};border-color:${QC_COLORS.screenUi.background};margin-top:.75rem" type="submit" name="identity_provider" value="apple">Sign in with Apple</button>` : "";
}
function encoded(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function decoded(text: string): Uint8Array {
  return Uint8Array.from(atob(text.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0));
}
const jsonPart = (value: unknown) => encoded(new TextEncoder().encode(JSON.stringify(value)));
export async function appleClientSecret(env: AppleOAuthEnv, now = Date.now()): Promise<string> {
  if (!appleSignInEnabled(env)) throw new PublicError("Apple sign-in is not configured.");
  const issuedAt = Math.floor(now / 1000);
  const input = `${jsonPart({ alg: "ES256", kid: env.APPLE_KEY_ID })}.${jsonPart({ iss: env.APPLE_TEAM_ID, iat: issuedAt, exp: issuedAt + 300, aud: "https://appleid.apple.com", sub: env.APPLE_CLIENT_ID })}`;
  const pem = env.APPLE_PRIVATE_KEY!.replace(/-----[^-]+-----/g, "").replace(/\s/g, "");
  const key = await crypto.subtle.importKey("pkcs8", decoded(pem), { name: "ECDSA", namedCurve: "P-256" }, false, ["sign"]);
  const signature = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key, new TextEncoder().encode(input));
  return `${input}.${encoded(new Uint8Array(signature))}`;
}
export function appleAuthorizationUrl(env: AppleOAuthEnv, redirectUri: string, state: string, nonce: string): string {
  if (!appleSignInEnabled(env)) throw new PublicError("Apple sign-in is not configured.");
  const url = new URL("https://appleid.apple.com/auth/authorize");
  url.search = new URLSearchParams({ client_id: env.APPLE_CLIENT_ID!, redirect_uri: redirectUri, response_type: "code", response_mode: "form_post", scope: "email", state, nonce }).toString();
  return url.toString();
}
/** Signature and audience are checked before any claim is used for account routing. */
export async function verifyAppleToken(env: AppleOAuthEnv, token: string, nonce?: string, now = Date.now(), notification = false): Promise<Record<string, unknown>> {
  const audience = notification ? env.APPLE_APP_ID : env.APPLE_CLIENT_ID;
  if (!audience) throw new PublicError("Apple sign-in is not configured.");
  const parts = token.split(".");
  if (parts.length !== 3) throw new PublicError("Invalid Apple identity token.");
  const header = JSON.parse(new TextDecoder().decode(decoded(parts[0]))) as { alg?: string; kid?: string };
  if (header.alg !== "RS256" || !header.kid) throw new PublicError("Invalid Apple identity signature.");
  const keys = await fetchIdentityJson("https://appleid.apple.com/auth/keys", { cf: { cacheTtl: 300, cacheEverything: true } }) as { keys?: Array<JsonWebKey & { kid?: string }> };
  const jwk = keys.keys?.find(key => key.kid === header.kid && key.kty === "RSA");
  if (!jwk) throw new PublicError("Unknown Apple identity signing key.");
  const key = await crypto.subtle.importKey("jwk", jwk, { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, ["verify"]);
  if (!await crypto.subtle.verify("RSASSA-PKCS1-v1_5", key, decoded(parts[2]), new TextEncoder().encode(`${parts[0]}.${parts[1]}`))) throw new PublicError("Invalid Apple identity signature.");
  const payload = JSON.parse(new TextDecoder().decode(decoded(parts[1]))) as Record<string, unknown>;
  const seconds = Math.floor(now / 1000);
  if (payload.iss !== "https://appleid.apple.com" || payload.aud !== audience || (!notification && typeof payload.exp !== "number") || (payload.exp !== undefined && (typeof payload.exp !== "number" || payload.exp <= seconds)) || typeof payload.iat !== "number" || payload.iat > seconds + 60 || (notification && payload.iat < seconds - 86400) || (!notification && (typeof payload.sub !== "string" || !/^[A-Za-z0-9._-]{1,255}$/.test(payload.sub))) || (nonce !== undefined && payload.nonce !== nonce)) throw new PublicError("Invalid Apple identity claims.");
  return payload;
}
export async function exchangeAppleCode(env: AppleOAuthEnv, code: string, redirectUri: string, nonce: string) {
  const result = await fetchIdentityJson("https://appleid.apple.com/auth/token", { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ client_id: env.APPLE_CLIENT_ID!, client_secret: await appleClientSecret(env), code, grant_type: "authorization_code", redirect_uri: redirectUri }) }) as { id_token?: string; access_token?: string };
  if (!result.id_token || !result.access_token) throw new PublicError("Apple did not return a complete sign-in response.");
  const payload = await verifyAppleToken(env, result.id_token, nonce);
  if (typeof payload.email !== "string" || !payload.email || (payload.email_verified !== true && payload.email_verified !== "true")) throw new PublicError("Apple did not confirm an email address.");
  // Never merge accounts by email, including Apple's private relay addresses.
  return { sub: `apple:${payload.sub}`, email: payload.email, name: payload.email, issuedAt: payload.iat as number, accessToken: result.access_token };
}
/** Apple posts across sites; only its callback flow uses a SameSite=None cookie. */
export async function normalizeAppleCallback(request: Request): Promise<Request> {
  const url = new URL(request.url);
  if (!/^\/auth\/apple\/(mcp|pair|delete)\/callback$/.test(url.pathname) || request.method !== "POST") return request;
  const origin = request.headers.get("origin");
  if (origin && origin !== "https://appleid.apple.com") throw new PublicError("Invalid Apple callback origin.");
  if (!(request.headers.get("content-type") ?? "").startsWith("application/x-www-form-urlencoded")) throw new PublicError("Invalid Apple callback format.");
  const body = await request.text();
  if (body.length > 16_384) throw new PublicError("Invalid Apple callback size.");
  const form = new URLSearchParams(body);
  // Never copy a token or untrusted account details into the callback URL.
  url.search = new URLSearchParams({ state: form.get("state") ?? "", code: form.get("code") ?? "" }).toString();
  return new Request(url, { method: "GET", headers: request.headers });
}
/** Obtain a new Apple token during deletion sign-in, then revoke after typed confirmation.
 * No upstream refresh token is retained by the ordinary login flow. */
export async function revokeAppleAccess(env: AppleOAuthEnv, token: string): Promise<void> {
  const response = await fetch("https://appleid.apple.com/auth/revoke", { method: "POST", signal: AbortSignal.timeout(10_000), headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ client_id: env.APPLE_CLIENT_ID!, client_secret: await appleClientSecret(env), token, token_type_hint: "access_token" }) });
  if (!response.ok) throw new PublicError("Apple access could not be revoked. Please try deletion again.");
}

async function deletionKey(secret: string): Promise<CryptoKey> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(`qcrc-apple-delete-v1:${secret}`));
  return crypto.subtle.importKey("raw", digest, "AES-GCM", false, ["encrypt", "decrypt"]);
}
export async function sealAppleDeletionToken(token: string, subject: string, secret: string): Promise<string> {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ciphertext = await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: new TextEncoder().encode(subject) }, await deletionKey(secret), new TextEncoder().encode(token));
  return `${encoded(iv)}.${encoded(new Uint8Array(ciphertext))}`;
}
export async function openAppleDeletionToken(value: string, subject: string, secret: string): Promise<string> {
  const parts = value.split(".");
  if (parts.length !== 2) throw new PublicError("Invalid deletion credential.");
  const plaintext = await crypto.subtle.decrypt({ name: "AES-GCM", iv: decoded(parts[0]), additionalData: new TextEncoder().encode(subject) }, await deletionKey(secret), decoded(parts[1]));
  return new TextDecoder().decode(plaintext);
}


export async function readAppleNotification(request: Request, env: AppleOAuthEnv): Promise<{ subject: string; eventTime: number } | null> {
  const text = await request.text();
  if (text.length > 16_384) throw new PublicError("Invalid Apple notification size.");
  const body = JSON.parse(text) as { payload?: string };
  if (typeof body.payload !== "string") throw new PublicError("Invalid Apple notification.");
  const claims = await verifyAppleToken(env, body.payload, undefined, Date.now(), true);
  const event = (typeof claims.events === "string" ? JSON.parse(claims.events) : claims.events) as { type?: string; sub?: string; event_time?: number } | undefined;
  if (!event || !["consent-revoked", "account-deleted"].includes(event.type ?? "")) return null;
  if (typeof event.sub !== "string" || !/^[A-Za-z0-9._-]{1,255}$/.test(event.sub) || typeof event.event_time !== "number" || !Number.isSafeInteger(event.event_time) || event.event_time <= 0 || event.event_time > Math.floor(Date.now() / 1000) + 60) throw new PublicError("Invalid Apple account event.");
  return { subject: `apple:${event.sub}`, eventTime: event.event_time };
}
