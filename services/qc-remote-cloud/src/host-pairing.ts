/** Browser pairing of a host app: the session it creates, the page a person
 * confirms it on, and the single completion it may have.
 *
 * Kept out of `index.ts` so the whole route sequence can be driven against an
 * in-memory KV in the tests, without the Workers runtime.
 *
 * A pairing session is unauthenticated by design (see `pairing-code.ts`), so
 * the link to it is a capability: whoever opens it and signs in decides which
 * Google account the computer that created it joins. That used to happen
 * without a single screen in between - opening the link went straight to
 * Google - so a link sent by somebody else attached *their* computer to the
 * recipient's account with nothing on the way saying so. The link now opens a
 * confirmation page naming what is being added, where and when the request
 * came from, and that it will join the account chosen next, and only an
 * explicit Continue on that page reaches the sign-in. A session completes at
 * most once, and is gone when it expires. */
import { beginPairingSignIn, type GoogleOAuthEnv } from "./access-auth.ts";
import { readBrowserForm } from "./http-security.ts";
import { checkHostPairingSession, isHostSessionId, legacyVerificationDigits, type PairingSessionCheck } from "./pairing-code.ts";
import { cancelHostPairingState, claimHostPairingState, completeHostPairingState, pollHostPairingState, type HostPairingState } from "./host-pairing-state.ts";
import { htmlPageHeaders } from "./http-security.ts";

export type PairingMetadata = { email: string; device: string; pairedAt: number };

/** What the host said about itself when it created the session. Self-declared
 * and unverified, which the confirmation page says. */
export type DeclaredHost = { label?: string; platform?: string };

/** Where the request that created the session came from, as Cloudflare saw it. */
export type RequestLocation = { country?: string; city?: string };

export type HostPairingSession = {
  receiverSecretHash: string;
  expiresAt: number;
  createdAt?: number;
  host?: DeclaredHost;
  origin?: RequestLocation;
  pairingCode?: string;
  completedAt?: number;
  pairing?: PairingMetadata;
};

export interface HostPairingAuthority {
  initializeHostPairingSession(session: HostPairingSession): Promise<boolean>;
  readHostPairingSession(candidate?: HostPairingSession): Promise<HostPairingState | null>;
  claimHostPairingSession(candidate: HostPairingSession | null, claimId: string, now: number): ReturnType<typeof claimHostPairingState>;
  completeHostPairingSession(claimId: string, pairingCode: string, pairing: PairingMetadata, now: number): Promise<boolean>;
  cancelHostPairingSession(candidate: HostPairingSession | null, now: number): Promise<"cancelled" | "invalid-session" | "expired" | "used">;
  pollHostPairingSession(candidate: HostPairingSession | null, receiverSecretHash: string, now: number): ReturnType<typeof pollHostPairingState>;
}

export type HostPairingEnv = GoogleOAuthEnv & { DEVICE_SESSION: { getByName(name: string): HostPairingAuthority } };

export const HOST_PAIRING_TTL_MS = 10 * 60_000;
/** Workers KV refuses an `expirationTtl` below sixty seconds. A completion in
 * the last minute of a session used to compute a smaller one, so the write
 * threw and the person who had just signed in was shown an error. The record's
 * own `expiresAt` still ends it on time; this only bounds how long KV keeps it. */
const MIN_KV_TTL_SECONDS = 60;
const PROTOCOL_VERSION = "qc-relay.v1";
const CONFIRM_COOKIE = "__Host-qcrc-pair";

export function hostPairingKey(sessionId: string): string { return `qcrc:host-pair:${sessionId}`; }
export function hostPairingObjectName(sessionId: string): string { return `qcrc-host-pairing:${sessionId}`; }
function pairingAuthority(env: HostPairingEnv, sessionId: string): HostPairingAuthority { return env.DEVICE_SESSION.getByName(hostPairingObjectName(sessionId)); }

function hex(bytes: Uint8Array): string { return Array.from(bytes, (part) => part.toString(16).padStart(2, "0")).join(""); }
function randomToken(bytes: number): string { return hex(crypto.getRandomValues(new Uint8Array(bytes))); }
async function sha256(value: string): Promise<string> { return hex(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value)))); }

function same(left: string | null, right: string): boolean {
  if (!left || left.length !== right.length) return false;
  let difference = 0;
  for (let index = 0; index < right.length; index += 1) difference |= left.charCodeAt(index) ^ right.charCodeAt(index);
  return difference === 0;
}

function cookie(request: Request, name: string): string | null {
  const value = request.headers.get("cookie")?.split(";").map((item) => item.trim()).find((item) => item.startsWith(`${name}=`));
  return value ? value.slice(name.length + 1) : null;
}

export function escapeHtml(value: unknown): string {
  return String(value ?? "").replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character] ?? character);
}

function boundedText(value: unknown, max: number): string | undefined {
  if (typeof value !== "string") return undefined;
  const text = value.trim();
  return text && text.length <= max && !/[\x00-\x1f\x7f]/.test(text) ? text : undefined;
}

/** The label and platform a host declares in its session request, bounded.
 * Hosts built before this field send no body (Windows) or `{}` (Android). */
export function declaredHost(payload: Record<string, unknown> | null | undefined): DeclaredHost {
  const label = boundedText(payload?.label, 80);
  const platform = boundedText(payload?.platform, 32);
  return { ...(label ? { label } : {}), ...(platform ? { platform } : {}) };
}

export function requestLocation(request: Request): RequestLocation {
  const cf = (request as Request & { cf?: Record<string, unknown> }).cf;
  const country = boundedText(cf?.country, 2);
  const city = boundedText(cf?.city, 64);
  return { ...(country ? { country } : {}), ...(city ? { city } : {}) };
}

async function readSession(env: HostPairingEnv, sessionId: string): Promise<HostPairingSession | null> {
  return await env.QCRC_OAUTH.get<HostPairingSession>(hostPairingKey(sessionId), "json");
}

async function authoritativeState(env: HostPairingEnv, sessionId: string): Promise<HostPairingState | null> {
  return pairingAuthority(env, sessionId).readHostPairingSession(await readSession(env, sessionId) ?? undefined);
}

/** `POST /v1/host/pairing-sessions`. */
export async function createHostPairingSession(request: Request, env: HostPairingEnv, payload: Record<string, unknown> | null, now = Date.now()): Promise<Response> {
  const sessionId = randomToken(20);
  const receiverSecret = randomToken(32);
  const expiresAt = now + HOST_PAIRING_TTL_MS;
  const host = declaredHost(payload);
  const origin = requestLocation(request);
  const record: HostPairingSession = {
    receiverSecretHash: await sha256(receiverSecret),
    expiresAt,
    createdAt: now,
    ...(Object.keys(host).length > 0 ? { host } : {}),
    ...(Object.keys(origin).length > 0 ? { origin } : {})
  };
  // The namespaced Durable Object is the atomic authority for the one-time
  // claim. KV remains a lookup mirror for old links and is never used to win a
  // claim race.
  if (!await pairingAuthority(env, sessionId).initializeHostPairingSession(record)) throw new Error("Could not initialize pairing session");
  await env.QCRC_OAUTH.put(hostPairingKey(sessionId), JSON.stringify(record), { expirationTtl: HOST_PAIRING_TTL_MS / 1000 });
  // `verificationCode` is answered for apps that predate its retirement and
  // will not open the pairing page without one. It is not stored and not
  // checked; such an app displays digits this service no longer asks for,
  // which is the cost of it continuing to pair at all.
  return new Response(JSON.stringify({
    sessionId,
    receiverSecret,
    verificationCode: legacyVerificationDigits(),
    pairUrl: new URL(`/pair?hostSession=${encodeURIComponent(sessionId)}`, request.url).toString(),
    expiresAt,
    protocol: PROTOCOL_VERSION
  }), { headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" } });
}

const UNAVAILABLE_MESSAGES: Record<Exclude<PairingSessionCheck, { ok: true }>["reason"], { status: number; message: string }> = {
  "invalid-session": { status: 410, message: "This pairing code has expired. Open REMOTE in QC Remote to create a new one." },
  expired: { status: 410, message: "This pairing code has expired. Open REMOTE in QC Remote to create a new one." },
  used: { status: 410, message: "This pairing link has already been used. Each link pairs one device once." }
};

export function pairingUnavailablePage(message: string, styles: string): string {
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Pairing code ended</title><style>${styles}</style></head><body><main><h1>Pairing code ended</h1><p>${escapeHtml(message)}</p><p>Return to QC Remote and open <b>REMOTE</b> to create a new code.</p></main></body></html>`;
}

function unavailable(message: string, status: number, styles: string): Response {
  return new Response(pairingUnavailablePage(message, styles), { status, headers: htmlPageHeaders() });
}

function unavailableFor(check: Exclude<PairingSessionCheck, { ok: true }>, styles: string): Response {
  const { status, message } = UNAVAILABLE_MESSAGES[check.reason];
  return unavailable(message, status, styles);
}

function describeAge(createdAt: number | undefined, now: number): string {
  if (typeof createdAt !== "number") return "Time not recorded";
  const minutes = Math.max(0, Math.floor((now - createdAt) / 60_000));
  const ago = minutes === 0 ? "less than a minute ago" : minutes === 1 ? "1 minute ago" : `${minutes} minutes ago`;
  return `${ago} (${new Date(createdAt).toISOString().slice(11, 16)} UTC)`;
}

function describeLocation(origin: RequestLocation | undefined): string {
  if (!origin?.country && !origin?.city) return "Location not available";
  return [origin.city, origin.country].filter(Boolean).join(", ");
}

/** The page a pairing link opens. Continue reaches Google; Cancel ends the session. */
export function pairingConfirmationPage(sessionId: string, session: HostPairingSession, confirmation: string, styles: string, now = Date.now(), viewer: RequestLocation = {}): string {
  const label = session.host?.label ?? "QC Remote (this app version does not send a name)";
  const platform = session.host?.platform ?? "Not stated by this app version";
  const elsewhere = session.origin?.country && viewer.country && session.origin.country !== viewer.country
    ? `<p class="warn" role="alert">This request came from ${escapeHtml(session.origin.country)}, but you are opening it from ${escapeHtml(viewer.country)}. If you did not start pairing yourself, cancel.</p>`
    : "";
  const hidden = `<input type="hidden" name="hostSession" value="${escapeHtml(sessionId)}"><input type="hidden" name="confirmation" value="${escapeHtml(confirmation)}">`;
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Add device</title><style>${styles}.facts{display:grid;gap:.5rem;margin:1.25rem 0}.facts div{padding:.7rem 1rem;border:1px solid currentColor;border-radius:.55rem;overflow-wrap:anywhere}.facts dt{font-size:.78rem;opacity:.75}.facts dd{margin:.2rem 0 0;font-weight:700}.warn{font-weight:700}</style></head><body><main><div class="badge">QC Remote</div><h1>Add this device to your account?</h1><p>A QC Remote app asked to be paired. Continue only if you started pairing yourself, just now, on your own computer or phone. If someone sent you this link, cancel.</p><dl class="facts"><div><dt>Device name (as the app reported it)</dt><dd>${escapeHtml(label)}</dd></div><div><dt>Platform (as the app reported it)</dt><dd>${escapeHtml(platform)}</dd></div><div><dt>Requested</dt><dd>${escapeHtml(describeAge(session.createdAt, now))}</dd></div><div><dt>Requested from</dt><dd>${escapeHtml(describeLocation(session.origin))}</dd></div></dl>${elsewhere}<p>If you continue, this device will be attached to the Google account you choose on the next screen, and AI apps connected to that account will be able to control it.</p><div class="actions"><form method="post" action="/pair/sign-in">${hidden}<button class="button primary" type="submit">Continue</button></form><form method="post" action="/pair/cancel">${hidden}<button class="button" type="submit">Cancel</button></form></div></main></body></html>`;
}

/** `GET /pair?hostSession=...`: the confirmation page, never a redirect. */
export async function showPairingConfirmation(request: Request, env: HostPairingEnv, sessionId: string, styles: string, now = Date.now()): Promise<Response> {
  if (!isHostSessionId(sessionId)) return unavailable("This pairing link is not valid.", 400, styles);
  const state = await authoritativeState(env, sessionId);
  const session = state?.session ?? null;
  const check = state && (state.status !== "pending")
    ? { ok: false as const, reason: "used" as const }
    : checkHostPairingSession(session, now);
  if (!check.ok) return unavailableFor(check, styles);
  const confirmation = randomToken(24);
  const headers = new Headers(htmlPageHeaders());
  headers.append("set-cookie", `${CONFIRM_COOKIE}-${sessionId}=${confirmation}; HttpOnly; Secure; Path=/; SameSite=Lax; Max-Age=${HOST_PAIRING_TTL_MS / 1000}`);
  return new Response(pairingConfirmationPage(sessionId, session!, confirmation, styles, now, requestLocation(request)), { headers });
}

/** The session named by a confirmation form, provided the form really came
 * from the confirmation page this browser was shown.
 *
 * The double-submit cookie is what makes the page a step rather than a
 * suggestion: without it, any site could post a session id straight to
 * `/pair/sign-in` and the person would land on Google having seen nothing. */
async function confirmedSession(request: Request, env: HostPairingEnv, form: FormData, styles: string, now: number): Promise<{ sessionId: string } | Response> {
  const sessionId = String(form.get("hostSession") ?? "").trim();
  if (!isHostSessionId(sessionId)) return unavailable("This pairing link is not valid.", 400, styles);
  const confirmation = String(form.get("confirmation") ?? "");
  if (!confirmation || !same(cookie(request, `${CONFIRM_COOKIE}-${sessionId}`), confirmation)) {
    return unavailable("This confirmation was not made on this browser's pairing page. Open the pairing link again.", 400, styles);
  }
  const state = await authoritativeState(env, sessionId);
  const check = state && (state.status !== "pending")
    ? { ok: false as const, reason: "used" as const }
    : checkHostPairingSession(state?.session, now);
  if (!check.ok) return unavailableFor(check, styles);
  return { sessionId };
}

/** `POST /pair/sign-in`. Without a session it is the manual pairing path
 * (someone who typed the address); with one, it is Continue on the
 * confirmation page. */
export async function startPairingSignIn(request: Request, env: HostPairingEnv, styles: string, now = Date.now()): Promise<Response> {
  const form = await readBrowserForm(request);
  if (!form) return unavailable("Invalid pairing form.", 400, styles);
  if (!String(form.get("hostSession") ?? "").trim()) return beginPairingSignIn(request, env);
  const confirmed = await confirmedSession(request, env, form, styles, now);
  if (confirmed instanceof Response) return confirmed;
  return beginPairingSignIn(request, env, confirmed.sessionId);
}

/** `POST /pair/cancel`: end the session so the link cannot be used later. */
export async function cancelPairing(request: Request, env: HostPairingEnv, styles: string, now = Date.now()): Promise<Response> {
  const form = await readBrowserForm(request);
  if (!form) return unavailable("Invalid pairing form.", 400, styles);
  const confirmed = await confirmedSession(request, env, form, styles, now);
  if (confirmed instanceof Response) return confirmed;
  const result = await pairingAuthority(env, confirmed.sessionId).cancelHostPairingSession(await readSession(env, confirmed.sessionId), now);
  if (result !== "cancelled") return unavailableFor({ ok: false, reason: result }, styles);
  await env.QCRC_OAUTH.delete(hostPairingKey(confirmed.sessionId));
  return new Response(`<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Pairing cancelled</title><style>${styles}</style></head><body><main><h1>Pairing cancelled</h1><p>Nothing was added to any account, and this pairing link no longer works.</p><p class="help">To pair your own device, open <b>REMOTE</b> in QC Remote and start again.</p></main></body></html>`, { headers: htmlPageHeaders() });
}

/** The session a completed Google sign-in is about to complete, if it still may be.
 *
 * Checked before anything else happens in the callback - in particular before
 * a pairing offer is minted, because minting replaces the account's pending
 * offer and a replayed callback would otherwise clobber a real one. */
export async function openHostPairingSession(env: HostPairingEnv, sessionId: string, now = Date.now()): Promise<{ ok: true; session: HostPairingSession; claimId: string } | { ok: false; reason: "invalid-session" | "expired" | "used" }> {
  if (!isHostSessionId(sessionId)) return { ok: false, reason: "invalid-session" };
  const claimId = randomToken(24);
  return pairingAuthority(env, sessionId).claimHostPairingSession(await readSession(env, sessionId), claimId, now);
}

/** Persist the claimed result in the authority before any caller may return it. */
export async function completeHostPairingSession(env: HostPairingEnv, sessionId: string, claimId: string, pairingCode: string, pairing: PairingMetadata, now = Date.now()): Promise<boolean> {
  if (!isHostSessionId(sessionId)) return false;
  const completed = await pairingAuthority(env, sessionId).completeHostPairingSession(claimId, pairingCode, pairing, now);
  if (!completed) return false;
  const state = await authoritativeState(env, sessionId);
  if (state?.status !== "completed" || state.claimId !== claimId || !state.result) return false;
  // The mirror never carries the pairing code: the Durable Object holds
  // it until the host collects it once. The mirror keeps only what lets a page
  // say the link was used.
  const record: HostPairingSession = { ...state.session, completedAt: state.result.completedAt, pairing: state.result.pairing };
  const remainingSeconds = Math.ceil((record.expiresAt - now) / 1000);
  // The DO result is already durable. A failed mirror write must not cause a
  // retry that can replace or replay the completed result.
  try { await env.QCRC_OAUTH.put(hostPairingKey(sessionId), JSON.stringify(record), { expirationTtl: Math.max(MIN_KV_TTL_SECONDS, remainingSeconds) }); } catch { /* Durable Object is authoritative. */ }
  return true;
}

export async function hostPairingStatus(env: HostPairingEnv, sessionId: string, receiverSecret: string, now = Date.now()): Promise<{ state: "waiting"; expiresAt: number } | { state: "complete"; pairingCode: string; expiresAt: number } | null> {
  if (!isHostSessionId(sessionId) || !receiverSecret) return null;
  const receiverSecretHash = await sha256(receiverSecret);
  const mirrored = await readSession(env, sessionId);
  const status = await pairingAuthority(env, sessionId).pollHostPairingSession(mirrored, receiverSecretHash, now);
  // A mirror written before the code moved into the Durable Object still
  // holds the plaintext code. The Durable
  // Object has just handed it out once and dropped it; drop it here too. A
  // failure only leaves that legacy copy to its own KV expiry (at most the
  // session's ten minutes), and the Durable Object never re-reads the mirror
  // while its own state exists.
  if (status?.state === "complete" && mirrored?.pairingCode !== undefined) {
    const { pairingCode: _collected, ...kept } = mirrored;
    try { await env.QCRC_OAUTH.put(hostPairingKey(sessionId), JSON.stringify(kept), { expirationTtl: Math.max(MIN_KV_TTL_SECONDS, Math.ceil((kept.expiresAt - now) / 1000)) }); }
    catch { /* The Durable Object is authoritative and already delivered. */ }
  }
  return status;
}

/** Where a failed check sends the browser after the Google round trip. */
export function hostPairingUnavailable(reason: "invalid-session" | "expired" | "used", styles: string): Response {
  return unavailableFor({ ok: false, reason }, styles);
}
