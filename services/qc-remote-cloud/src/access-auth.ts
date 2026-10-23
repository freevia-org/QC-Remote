import { appleAuthorizationUrl, appleSignInButton, exchangeAppleCode, requestedIdentityProvider, type AppleOAuthEnv, type IdentityProvider } from "./apple-auth.ts";
import { observedClientName, requestClientMetadata } from "./mcp-presence.ts";
import type { AuthRequest, OAuthHelpers } from "@cloudflare/workers-oauth-provider";
import { QC_COLORS, QC_TYPOGRAPHY } from "@qc-remote/theme/server";
import { QC_SCOPES, SCOPE_DESCRIPTIONS, type QcScope } from "./scopes.ts";
import { connectorGrantUser } from "./connector-grants.ts";
import { grantIdFromRedirect } from "./account-deletion.ts";
import { clientIdMetadataOrigin, recogniseRedirect, type RedirectRecognition } from "./client-recognition.ts";
import { clientFailureText, PublicError, readBrowserForm, fetchIdentityJson } from "./http-security.ts";

/** Existing Google bindings plus optional Apple sign-in. Cloudflare hosts the relay. */
export interface GoogleOAuthEnv extends AppleOAuthEnv {
  QCRC_OAUTH: KVNamespace;
  OAUTH_PROVIDER: OAuthHelpers;
  GOOGLE_CLIENT_ID: string;
  GOOGLE_CLIENT_SECRET: string;
  COOKIE_ENCRYPTION_KEY: string;
}

type PendingApproval = { request: AuthRequest };
type PendingAuthorization = { request: AuthRequest; browserHash: string; identityProvider?: IdentityProvider };
type PendingDirectPairing = { hostSession?: string; browserHash: string; identityProvider?: IdentityProvider };
type GoogleIdentity = { sub: string; email: string; name: string; authTime?: number; issuedAt?: number };
type PendingAccountDeletion = { browserHash: string; startedAt: number; identityProvider?: IdentityProvider };

const STATE_TTL_SECONDS = 10 * 60;
const ACCOUNT_BEING_DELETED = "This QC Remote account is being deleted, or was deleted while you were signing in. Wait for the deletion to finish, then connect again.";
// The consent screen used to render one fixed sentence whatever was requested,
// so a client asking for full control and a client asking to read the tempo
// produced identical pages. This is the only point in the whole system where a
// person decides anything, and it has to say what it is deciding.
const ALLOWED_SCOPES = new Set<string>(QC_SCOPES);
const GOOGLE_AUTHORIZATION_URL = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL = "https://oauth2.googleapis.com/token";
const GOOGLE_JWKS_URL = "https://www.googleapis.com/oauth2/v3/certs";

function base64url(bytes: Uint8Array): string {
  let text = "";
  for (const byte of bytes) text += String.fromCharCode(byte);
  return btoa(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

function randomToken(bytes = 32): string { return base64url(crypto.getRandomValues(new Uint8Array(bytes))); }

function cookie(request: Request, name: string): string | null {
  const value = request.headers.get("cookie")?.split(";").map((item) => item.trim()).find((item) => item.startsWith(`${name}=`));
  return value ? value.slice(name.length + 1) : null;
}

function same(left: string | null, right: string): boolean {
  if (!left || left.length !== right.length) return false;
  let difference = 0;
  for (let index = 0; index < right.length; index += 1) difference |= left.charCodeAt(index) ^ right.charCodeAt(index);
  return difference === 0;
}

async function hmac(value: string, secret: string): Promise<string> {
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return base64url(new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(value))));
}

async function verifyState(state: string | null, secret: string): Promise<string | null> {
  if (!state) return null;
  const separator = state.lastIndexOf(".");
  if (separator < 1) return null;
  const nonce = state.slice(0, separator);
  return same(state.slice(separator + 1), await hmac(nonce, secret)) ? nonce : null;
}

function response(html: string, csrf?: string): Response {
  // Chromium checks form-action against the POST's redirect as well as its
  // same-origin target. Allow only our configured Google authorization origin.
  const headers = new Headers({ "content-type": "text/html; charset=utf-8", "content-security-policy": `default-src 'none'; style-src 'unsafe-inline'; form-action 'self' ${new URL(GOOGLE_AUTHORIZATION_URL).origin} https://appleid.apple.com; frame-ancestors 'none'`, "x-frame-options": "DENY", "cache-control": "no-store" });
  if (csrf) headers.append("set-cookie", `__Host-qcrc-csrf-${csrf}=${csrf}; HttpOnly; Secure; Path=/; SameSite=Lax; Max-Age=${STATE_TTL_SECONDS}`);
  return new Response(html, { headers });
}

function escape(value: unknown): string { return String(value ?? "").replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character] ?? character); }

function authorizedDeviceLabel(request: Request): string {
  const agent = request.headers.get("user-agent") ?? "";
  const platform = /Android/i.test(agent) ? "Android" : /iPhone|iPad|iPod/i.test(agent) ? "iPhone" : /Windows/i.test(agent) ? "Windows" : /Macintosh|Mac OS X/i.test(agent) ? "Mac" : "Web";
  const browser = /EdgA?|EdgiOS/i.test(agent) ? "Edge" : /Firefox|FxiOS/i.test(agent) ? "Firefox" : /Chrome|CriOS/i.test(agent) ? "Chrome" : /Safari/i.test(agent) ? "Safari" : "browser";
  return `${platform} ${browser}`;
}

/** The consent an MCP client needs before it may reach this account.
 *
 * This runs before the Google round trip, so the account is not known here and
 * the page cannot name it. That is why it reads as a fresh grant and why people
 * ask why they are signing in twice: the answer is that they are not - pairing
 * attached a computer, and this attaches an app to the same account. The copy
 * says so, because the page cannot show it. */
/** Who receives this grant, stated beside the name the client chose for itself.
 *
 * The name is self-declared; the redirect host is where the authorization code
 * is actually sent. A known AI-app host or a loopback address is marked as
 * recognised; anything else is shown with a warning, not refused, because any
 * OAuth-capable MCP app may legitimately connect. */
function destinationPanel(clientName: string, redirect: RedirectRecognition, metadataOrigin: string | undefined): string {
  const verdict = redirect.kind === "first-party"
    ? `<p class="ok">Recognised: ${escape(redirect.vendor)}.</p>`
    : redirect.kind === "loopback"
      ? `<p class="ok">Recognised: an app running on this computer. The approval never leaves this device.</p>`
      : `<p class="warn" role="alert">QC Remote does not recognise ${escape(redirect.host)}. The name &ldquo;${escape(clientName)}&rdquo; was chosen by the app itself and is not verified. Allow only if you started this connection yourself, just now, in an app you trust.</p>`;
  const published = metadataOrigin ? `<div class="row"><span>App details published by</span><strong>${escape(metadataOrigin)}</strong></div>` : "";
  return `<div class="scope destination"><div class="row"><span>Access will be sent to</span><strong>${escape(redirect.host)}</strong></div>${published}${verdict}</div>`;
}

function approvalPage(clientName: string, nonce: string, scopes: readonly string[], redirect: RedirectRecognition, metadataOrigin?: string, env?: GoogleOAuthEnv): string {
  const granted = scopes.map((scope) => `<li><code>${escape(scope)}</code> &mdash; ${escape(SCOPE_DESCRIPTIONS[scope as QcScope] ?? "")}</li>`).join("");
  const control = scopes.includes("qc:control");
  // Only deletions ask, exactly as the Quad Cortex
  // itself asks before deleting a preset or a setlist. This sentence must say
  // that and nothing more; the paired hosts are what keep it true.
  const approvals = control ? `<p class="warn">The connected app must confirm a preset or setlist deletion before sending it. Your paired device checks access and the exact target; it does not ask a second time. A deletion cannot be undone.</p>` : "";
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Connect ${escape(clientName)}</title><style>:root{color-scheme:dark}*{box-sizing:border-box}body{margin:0;background:${QC_COLORS.app.canvas};color:${QC_COLORS.app.text};font:16px ${QC_TYPOGRAPHY.app};display:grid;min-height:100vh;place-items:center}main{width:min(38rem,100%);padding:2rem}h1{margin:.4rem 0 .75rem;font-size:2rem;line-height:1.15}p{color:${QC_COLORS.app.textMuted};line-height:1.55}.badge{color:${QC_COLORS.app.success};font-weight:700;font-size:.78rem;letter-spacing:.1em}.scope{margin:1.25rem 0;padding:1rem;border:1px solid ${QC_COLORS.app.border};border-radius:.65rem;background:${QC_COLORS.app.panelRaised}}.scope strong{display:block;margin-bottom:.35rem}.scope ul{margin:.5rem 0 0;padding-left:1.1rem}.scope li{margin:.35rem 0;color:${QC_COLORS.app.text};line-height:1.45}.scope code{color:${QC_COLORS.app.info};font:.85rem ${QC_TYPOGRAPHY.mono}}.row{margin:0 0 .6rem}.row span{display:block;color:${QC_COLORS.app.textMuted};font-size:.82rem}.row strong{font:700 1.05rem ${QC_TYPOGRAPHY.mono};color:${QC_COLORS.app.text};overflow-wrap:anywhere}.ok{margin:.6rem 0 0;color:${QC_COLORS.app.success};line-height:1.45}.warn{margin:.9rem 0 0;color:${QC_COLORS.app.warning};line-height:1.45}.button{display:block;width:100%;padding:.78rem 1rem;border:1px solid ${QC_COLORS.app.success};border-radius:.55rem;background:${QC_COLORS.app.success};color:${QC_COLORS.app.canvas};font:700 .95rem ${QC_TYPOGRAPHY.app};text-align:center;cursor:pointer}</style></head><body><main><div class="badge">QC Remote</div><h1>Connect ${escape(clientName)}</h1>${destinationPanel(clientName, redirect, metadataOrigin)}<p>Sign in with the same account and sign-in method you used to pair your host in QC Remote. ${escape(clientName)} will reach the Quad Cortex already paired to it.</p><div class="scope"><strong>What this allows</strong><ul>${granted}</ul>${approvals}</div><p class="help">This does not pair a new device and does not create a second account. You can remove this connector later in ${escape(clientName)}.</p><form method="post" action="/authorize"><input type="hidden" name="nonce" value="${escape(nonce)}"><button class="button" type="submit">Continue with Google</button>${env ? appleSignInButton(env) : ""}</form></main></body></html>`;
}

function bytes(encoded: string): Uint8Array {
  const normalized = encoded.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(encoded.length / 4) * 4, "=");
  const text = atob(normalized);
  return Uint8Array.from(text, (character) => character.charCodeAt(0));
}

async function identity(env: GoogleOAuthEnv, idToken: string): Promise<GoogleIdentity> {
  const [headerText, payloadText, signatureText] = idToken.split(".");
  if (!headerText || !payloadText || !signatureText) throw new Error("Invalid identity token");
  const header = JSON.parse(new TextDecoder().decode(bytes(headerText))) as { alg?: string; kid?: string };
  const payload = JSON.parse(new TextDecoder().decode(bytes(payloadText))) as Record<string, unknown>;
  if (header.alg !== "RS256" || !header.kid) throw new Error("Unsupported identity-token signing key");
  const keys = await fetchIdentityJson(GOOGLE_JWKS_URL, { cf: { cacheTtl: 300, cacheEverything: true } }) as { keys?: Array<JsonWebKey & { kid?: string }> };
  const jwk = keys.keys?.find((key) => key.kid === header.kid);
  if (!jwk) throw new Error("Unknown identity-token signing key");
  const key = await crypto.subtle.importKey("jwk", jwk, { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, ["verify"]);
  if (!await crypto.subtle.verify("RSASSA-PKCS1-v1_5", key, bytes(signatureText), new TextEncoder().encode(`${headerText}.${payloadText}`))) throw new Error("Invalid identity-token signature");
  const audience = payload.aud;
  const issuer = payload.iss;
  const audienceMatches = typeof audience === "string" ? audience === env.GOOGLE_CLIENT_ID : Array.isArray(audience) && audience.includes(env.GOOGLE_CLIENT_ID);
  if ((issuer !== "https://accounts.google.com" && issuer !== "accounts.google.com") || !audienceMatches || payload.email_verified !== true || typeof payload.exp !== "number" || payload.exp <= Math.floor(Date.now() / 1000) || typeof payload.sub !== "string" || typeof payload.email !== "string") throw new Error("Invalid Google identity-token claims");
  return {
    sub: payload.sub,
    email: payload.email,
    name: typeof payload.name === "string" ? payload.name : payload.email,
    // Freshness evidence for the account-deletion sign-in (see
    // ACCOUNT_DELETION_REAUTH_SECONDS); unused by the other flows.
    ...(typeof payload.auth_time === "number" && Number.isSafeInteger(payload.auth_time) ? { authTime: payload.auth_time } : {}),
    ...(typeof payload.iat === "number" && Number.isSafeInteger(payload.iat) ? { issuedAt: payload.iat } : {})
  };
}

/** The cookie that binds a Google round trip to the browser that started it.
 *
 * `state` alone only proves this worker minted the nonce. It does not prove the
 * browser presenting it is the one that began the flow, so a nonce handed to
 * someone else completes against their identity. Every sign-in therefore also
 * sets an opaque flow token whose keyed hash is stored beside the pending
 * request, and both callbacks require the two to agree. */
// Each flow has its own short-lived cookie so concurrent tabs cannot overwrite it.
const FLOW_COOKIE = "__Host-qcrc-flow";

function flowRedirect(location: string, token: string, nonce: string, identityProvider: IdentityProvider = "google"): Response {
  const headers = new Headers({ location, "cache-control": "no-store" });
  headers.append("set-cookie", `${FLOW_COOKIE}-${nonce}=${token}; HttpOnly; Secure; Path=/; SameSite=${identityProvider === "apple" ? "None" : "Lax"}; Max-Age=${STATE_TTL_SECONDS}`);
  return new Response(null, { status: 302, headers });
}

async function browserMatchesFlow(request: Request, env: GoogleOAuthEnv, browserHash: string, nonce: string): Promise<boolean> {
  const presented = cookie(request, `${FLOW_COOKIE}-${nonce}`);
  if (!presented) return false;
  return same(browserHash, await hmac(presented, env.COOKIE_ENCRYPTION_KEY));
}

function googleAuthorizationUrl(env: GoogleOAuthEnv, redirectUri: string, state: string, extra: Record<string, string> = {}): string {
  const upstream = new URL(GOOGLE_AUTHORIZATION_URL);
  upstream.search = new URLSearchParams({ client_id: env.GOOGLE_CLIENT_ID, redirect_uri: redirectUri, response_type: "code", scope: "openid email profile", prompt: "select_account", state, ...extra }).toString();
  return upstream.toString();
}

function callbackProvider(request: Request): IdentityProvider {
  return new URL(request.url).pathname.startsWith("/auth/apple/") ? "apple" : "google";
}
function callbackUri(request: Request, provider: IdentityProvider, flow: string): string {
  return new URL(`/auth/${provider}/${flow}/callback`, request.url).toString();
}
function upstreamAuthorization(env: GoogleOAuthEnv, request: Request, provider: IdentityProvider, flow: string, state: string, nonce: string): string {
  return provider === "apple" ? appleAuthorizationUrl(env, callbackUri(request, provider, flow), state, nonce)
    : googleAuthorizationUrl(env, callbackUri(request, provider, flow), state, flow === "delete" ? accountDeletionAuthorizationParams() : {});
}
async function exchangeIdentity(env: GoogleOAuthEnv, request: Request, provider: IdentityProvider, flow: string, code: string, nonce: string) {
  if (provider === "apple") {
    const user = await exchangeAppleCode(env, code, callbackUri(request, provider, flow), nonce);
    return { sub: user.sub, email: user.email, name: user.name, issuedAt: user.issuedAt, authTime: undefined, appleAccessToken: user.accessToken };
  }
  return { ...await exchangeGoogleCode(env, code, callbackUri(request, provider, flow)), appleAccessToken: undefined };
}

/** What the Google sign-in before a remote account deletion proves.
 *
 * By default it proves a RECENT SIGN-IN CONFIRMATION, not a fresh
 * authentication: the relay minted the nonce and stored when it did, the
 * browser that started the flow presented its flow cookie, the HMAC state
 * matched, and Google issued the ID token (`iat`) within the window below.
 * Google may satisfy that from an existing session without asking for a
 * password, so nothing here, in the pages or in the docs calls it
 * reauthentication.
 *
 * Google reports when the person last authenticated (`auth_time`) only when
 * the request asks for it through the `claims` parameter AND the OAuth client
 * has the claim enabled in Google's settings (developers.google.com/identity/
 * openid-connect/openid-connect#auth-time-param); Google documents no
 * `prompt=login`. Whether that claim is enabled is a deployment decision
 * outside this code. The isolated hook is
 * `ACCOUNT_DELETION_REQUEST_AUTH_TIME`: when true, the request asks for the
 * claim and `max_age`, and an `auth_time` older than the window is refused
 * (the person signs out of Google and starts again). Only then is the result
 * labelled `fresh-authentication`. */
export const ACCOUNT_DELETION_REQUEST_AUTH_TIME = false;
export const ACCOUNT_DELETION_SIGN_IN_WINDOW_SECONDS = 5 * 60;
/** Clock skew tolerated between Google and the relay. */
const AUTH_TIME_SKEW_SECONDS = 60;
const AUTH_TIME_CLAIMS = JSON.stringify({ id_token: { auth_time: { essential: true } } });

export type DeletionFreshness = "recent-sign-in" | "fresh-authentication";
export type DeletionSignIn = { sub: string; email: string; freshness: DeletionFreshness; verifiedAt: number; appleAccessToken?: string };

/** The extra authorization parameters for the deletion sign-in. */
export function accountDeletionAuthorizationParams(requestAuthTime = ACCOUNT_DELETION_REQUEST_AUTH_TIME): Record<string, string> {
  return requestAuthTime ? { max_age: String(ACCOUNT_DELETION_SIGN_IN_WINDOW_SECONDS), claims: AUTH_TIME_CLAIMS } : {};
}

/** Start the Google sign-in that must precede a remote account deletion. The
 * caller has checked the page's CSRF token. The flow cookie is SameSite=Lax
 * (flowRedirect) because Google's callback is a cross-site top-level GET. */
export async function beginAccountDeletionSignIn(request: Request, env: GoogleOAuthEnv, now = Date.now(), identityProvider: IdentityProvider = "google"): Promise<Response> {
  const nonce = randomToken(24);
  const browser = randomToken(24);
  await env.QCRC_OAUTH.put(`qcrc:delete-sign-in:${nonce}`, JSON.stringify({ browserHash: await hmac(browser, env.COOKIE_ENCRYPTION_KEY), startedAt: now, identityProvider } satisfies PendingAccountDeletion), { expirationTtl: STATE_TTL_SECONDS });
  const state = `${nonce}.${await hmac(nonce, env.COOKIE_ENCRYPTION_KEY)}`;
  return flowRedirect(upstreamAuthorization(env, request, identityProvider, "delete", state, nonce), browser, nonce, identityProvider);
}

/** Complete it: the verified identity and what its freshness rests on. */
export async function completeAccountDeletionSignIn(request: Request, env: GoogleOAuthEnv, now = Date.now()): Promise<DeletionSignIn> {
  const url = new URL(request.url);
  const nonce = await verifyState(url.searchParams.get("state"), env.COOKIE_ENCRYPTION_KEY);
  const code = url.searchParams.get("code");
  if (!nonce || !code) throw new PublicError("This sign-in link is invalid or expired. Start the deletion again.");
  const stored = await env.QCRC_OAUTH.get<PendingAccountDeletion>(`qcrc:delete-sign-in:${nonce}`, "json");
  await env.QCRC_OAUTH.delete(`qcrc:delete-sign-in:${nonce}`);
  if (!stored) throw new PublicError("This sign-in expired. Start the deletion again.");
  if (!await browserMatchesFlow(request, env, stored.browserHash, nonce)) throw new PublicError("This sign-in was started in a different browser. Start the deletion again in this browser.");
  const provider = callbackProvider(request);
  if ((stored.identityProvider ?? "google") !== provider) throw new PublicError("Sign-in provider mismatch.");
  const user = await exchangeIdentity(env, request, provider, "delete", code, nonce);
  const freshness = deletionFreshness(user, stored.startedAt, now);
  if (!freshness) {
    throw new PublicError(user.authTime !== undefined && ACCOUNT_DELETION_REQUEST_AUTH_TIME
      ? "Google reused an earlier sign-in. Sign out of Google in this browser, then start the deletion again."
      : "This sign-in took too long. Start the deletion again.");
  }
  return { sub: user.sub, email: user.email, ...freshness, appleAccessToken: user.appleAccessToken };
}

/** The freshness a completed deletion sign-in rests on, or null. Times in
 * seconds except `startedAt` and `now` (milliseconds). `auth_time` counts
 * only when the hook asked for it. */
export function deletionFreshness(user: { authTime?: number; issuedAt?: number }, startedAt: number | undefined, now = Date.now(), requestAuthTime = ACCOUNT_DELETION_REQUEST_AUTH_TIME): { freshness: DeletionFreshness; verifiedAt: number } | null {
  const nowSeconds = Math.floor(now / 1000);
  const recent = (seconds: number | undefined) => typeof seconds === "number" && Number.isFinite(seconds)
    && nowSeconds - seconds <= ACCOUNT_DELETION_SIGN_IN_WINDOW_SECONDS && seconds - nowSeconds <= AUTH_TIME_SKEW_SECONDS;
  if (typeof startedAt !== "number" || now - startedAt > STATE_TTL_SECONDS * 1000 || now < startedAt) return null;
  if (requestAuthTime) return recent(user.authTime) ? { freshness: "fresh-authentication", verifiedAt: user.authTime! } : null;
  return recent(user.issuedAt) ? { freshness: "recent-sign-in", verifiedAt: user.issuedAt! } : null;
}

async function exchangeGoogleCode(env: GoogleOAuthEnv, code: string, redirectUri: string): Promise<GoogleIdentity> {
  const tokens = await fetchIdentityJson(GOOGLE_TOKEN_URL, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded", accept: "application/json" }, body: new URLSearchParams({ client_id: env.GOOGLE_CLIENT_ID, client_secret: env.GOOGLE_CLIENT_SECRET, code, grant_type: "authorization_code", redirect_uri: redirectUri }) }) as { id_token?: string };
  if (!tokens.id_token) throw new Error("Google sign-in did not return an identity token");
  return identity(env, tokens.id_token);
}

/** Start the real Google sign-in used by the pairing page. */
export async function beginPairingSignIn(request: Request, env: GoogleOAuthEnv, verifiedHostSession?: string, identityProvider: IdentityProvider = "google"): Promise<Response> {
  // The caller has already checked the session is live and unused and that
  // this browser confirmed it on the pairing page (host-pairing.ts). Nothing
  // here re-reads the query string, so a session id cannot arrive unverified.
  const hostSession = verifiedHostSession?.trim();
  if (hostSession && !/^[A-Za-z0-9_-]{20,128}$/.test(hostSession)) {
    return new Response("Invalid pairing session", { status: 400 });
  }
  const nonce = randomToken(24);
  const browser = randomToken(24);
  await env.QCRC_OAUTH.put(`qcrc:direct-pair:${nonce}`, JSON.stringify({ hostSession: hostSession || undefined, browserHash: await hmac(browser, env.COOKIE_ENCRYPTION_KEY), identityProvider } satisfies PendingDirectPairing), { expirationTtl: STATE_TTL_SECONDS });
  const state = `${nonce}.${await hmac(nonce, env.COOKIE_ENCRYPTION_KEY)}`;
  return flowRedirect(upstreamAuthorization(env, request, identityProvider, "pair", state, nonce), browser, nonce, identityProvider);
}

/** Complete the pairing-page Google sign-in and return the verified owner. */
export async function completePairingSignIn(request: Request, env: GoogleOAuthEnv): Promise<GoogleIdentity & { hostSession?: string }> {
  const url = new URL(request.url);
  const nonce = await verifyState(url.searchParams.get("state"), env.COOKIE_ENCRYPTION_KEY);
  const code = url.searchParams.get("code");
  if (!nonce || !code) throw new PublicError("Invalid or expired sign-in request");
  const stored = await env.QCRC_OAUTH.get<PendingDirectPairing>(`qcrc:direct-pair:${nonce}`, "json");
  await env.QCRC_OAUTH.delete(`qcrc:direct-pair:${nonce}`);
  if (!stored) throw new PublicError("Sign-in request expired");
  if (!await browserMatchesFlow(request, env, stored.browserHash, nonce)) {
    throw new PublicError("This sign-in was started in a different browser. Open REMOTE in QC Remote and start a new pairing code.");
  }
  const provider = callbackProvider(request);
  if ((stored.identityProvider ?? "google") !== provider) throw new PublicError("Sign-in provider mismatch.");
  const user = await exchangeIdentity(env, request, provider, "pair", code, nonce);
  return { sub: user.sub, email: user.email, name: user.name, issuedAt: user.issuedAt, hostSession: stored.hostSession };
}

async function continueAuthorization(request: Request, env: GoogleOAuthEnv, nonce: string, identityProvider: IdentityProvider = "google"): Promise<Response> {
  const stored = await env.QCRC_OAUTH.get<PendingApproval>(`qcrc:approval:${nonce}`, "json");
  await env.QCRC_OAUTH.delete(`qcrc:approval:${nonce}`);
  if (!stored) return new Response("Authorization approval expired. Please return to Claude and try again.", { status: 400 });
  const stateNonce = randomToken(24);
  const browser = randomToken(24);
  await env.QCRC_OAUTH.put(`qcrc:access:${stateNonce}`, JSON.stringify({ request: stored.request, browserHash: await hmac(browser, env.COOKIE_ENCRYPTION_KEY), identityProvider } satisfies PendingAuthorization), { expirationTtl: STATE_TTL_SECONDS });
  const state = `${stateNonce}.${await hmac(stateNonce, env.COOKIE_ENCRYPTION_KEY)}`;
  return flowRedirect(upstreamAuthorization(env, request, identityProvider, "mcp", state, stateNonce), browser, stateNonce, identityProvider);
}

/** The account fence an AI-app grant passes through (account-deletion.ts):
 * admitted before the provider writes the grant, settled after. */
export type GrantFence = {
  rememberAppleSignIn?(subject: string, issuedAt: number): Promise<void>;
  admit(subject: string, admissionId: string): Promise<{ admitted: true; generation: number } | { admitted: false }>;
  settle(subject: string, admissionId: string, generation: number): Promise<boolean>;
};

export async function handleGoogleAuthorization(request: Request, env: GoogleOAuthEnv, fence?: GrantFence): Promise<Response | null> {
  const url = new URL(request.url);
  if (url.pathname === "/authorize" && request.method === "GET") {
    let authorization: AuthRequest;
    try { authorization = await env.OAUTH_PROVIDER.parseAuthRequest(request); } catch { return new Response("Invalid OAuth authorization request", { status: 400 }); }
    const client = await env.OAUTH_PROVIDER.lookupClient(authorization.clientId);
    if (!client) return new Response("Unknown OAuth client", { status: 400 });
    const nonce = randomToken(24);
    await env.QCRC_OAUTH.put(`qcrc:approval:${nonce}`, JSON.stringify({ request: authorization } satisfies PendingApproval), { expirationTtl: STATE_TTL_SECONDS });
    const requested = (authorization.scope ?? []).filter((scope) => ALLOWED_SCOPES.has(scope));
    return response(approvalPage(client.clientName || "this MCP client", nonce, requested.length > 0 ? requested : ["qc:read"], recogniseRedirect(authorization.redirectUri), clientIdMetadataOrigin(authorization.clientId), env), nonce);
  }
  // There is deliberately no GET twin of the route below. The consent step
  // used to be a link, and the link survived the conversion to a CSRF-protected
  // form: anyone could hand a victim `/authorize/continue?nonce=...` and have
  // the victim's Google identity complete the attacker's own authorization
  // request. Consent is a POST, checked against the double-submit cookie.
  if (url.pathname === "/authorize" && request.method === "POST") {
    const form = await readBrowserForm(request);
    if (!form) return new Response("Invalid authorization form", { status: 400 });
    const nonce = typeof form.get("nonce") === "string" ? form.get("nonce") as string : "";
    if (!same(cookie(request, `__Host-qcrc-csrf-${nonce}`), nonce)) return new Response("Authorization approval expired. Please try again.", { status: 400 });
    let provider: IdentityProvider;
    try { provider = requestedIdentityProvider(form.get("identity_provider"), env); } catch { return new Response("Sign-in method unavailable", { status: 400 }); }
    return continueAuthorization(request, env, nonce, provider);
  }
  if (/^\/auth\/(google|apple)\/mcp\/callback$/.test(url.pathname) && request.method === "GET") {
    const nonce = await verifyState(url.searchParams.get("state"), env.COOKIE_ENCRYPTION_KEY);
    const code = url.searchParams.get("code");
    if (!nonce || !code) return new Response("Invalid or expired sign-in request", { status: 400 });
    const stored = await env.QCRC_OAUTH.get<PendingAuthorization>(`qcrc:access:${nonce}`, "json");
    await env.QCRC_OAUTH.delete(`qcrc:access:${nonce}`);
    if (!stored) return new Response("Sign-in request expired", { status: 400 });
    if (!await browserMatchesFlow(request, env, stored.browserHash, nonce)) {
      return new Response("This sign-in was started in a different browser. Return to your MCP client and connect again.", { status: 400 });
    }
    try {
      const provider = callbackProvider(request);
      if ((stored.identityProvider ?? "google") !== provider) throw new PublicError("Sign-in provider mismatch.");
      const user = await exchangeIdentity(env, request, provider, "mcp", code, nonce);
      if (provider === "apple") {
        if (!fence?.rememberAppleSignIn || user.issuedAt === undefined) throw new PublicError("Apple account protection is unavailable.");
        await fence.rememberAppleSignIn(user.sub, user.issuedAt);
      }
      const requestedScopes = stored.request.scope.filter((scope) => ALLOWED_SCOPES.has(scope));
      // Some remote-MCP clients discover the resource first but omit `scope`
      // on their initial authorization request. Give those clients the least
      // privileged useful grant rather than an unusable zero-scope token.
      const scopes = requestedScopes.length > 0 ? requestedScopes : ["qc:read"];
      const client = await env.OAUTH_PROVIDER.lookupClient(stored.request.clientId);
      const clientName = observedClientName(client?.clientName) ?? undefined;
      const caller = requestClientMetadata(request);
      const userId = connectorGrantUser(user.sub);
      // The grant is bracketed by the account fence so a deletion that starts
      // while this callback runs can never be followed by this grant: either
      // admission is refused, or settling reports the deletion and the grant
      // just written is revoked here. Without a fence (single-owner tests)
      // the generation is 0.
      const admissionId = randomToken(16);
      const admission = fence ? await fence.admit(user.sub, admissionId) : { admitted: true as const, generation: 0 };
      if (!admission.admitted) return new Response(ACCOUNT_BEING_DELETED, { status: 409, headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" } });
      const result = await env.OAUTH_PROVIDER.completeAuthorization({ request: stored.request, userId, scope: scopes, metadata: { email: user.email, name: user.name, clientName, authorizedFrom: authorizedDeviceLabel(request), ...caller, accountGeneration: admission.generation }, props: { subject: user.sub, email: user.email, name: user.name, scopes, clientName, clientId: stored.request.clientId, accountGeneration: admission.generation } });
      if (fence) {
        let keep = false;
        try { keep = await fence.settle(user.sub, admissionId, admission.generation); } catch { keep = false; }
        if (!keep) {
          const grantId = grantIdFromRedirect(result.redirectTo, userId);
          if (grantId) await env.OAUTH_PROVIDER.revokeGrant(grantId, userId);
          return new Response(ACCOUNT_BEING_DELETED, { status: 409, headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" } });
        }
      }
      return Response.redirect(result.redirectTo, 302);
    } catch (cause) {
      // The cause may be a JWKS fetch failure, a KV error or a claims check; none
      // of that is the browser's business. It is logged against the reference.
      return clientFailureText(cause, { status: 403, fallback: "Sign-in could not be verified. Return to your MCP client and connect again.", context: "mcp-oauth-callback" });
    }
  }
  return null;
}
