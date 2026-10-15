import { QC_COLORS } from "@qc-remote/theme/server";

/** The web path for deleting a remote account: the one place a person can
 * delete everything the relay keeps for their Google account, from a browser
 * or from the QC Remote apps, which open this page (Google Play allows an
 * in-app link to the web deletion resource).
 *
 * Kept out of `index.ts` so the whole sequence can be driven in tests against
 * an in-memory KV and a fake account, without the Workers runtime.
 *
 * 1. `GET /account/delete` says what is deleted and what is not, and asks the
 *    person to confirm with Google which account it is. Its form carries a
 *    double-submit CSRF token (SameSite=Strict cookie; the form posts from
 *    this same site, so the cookie is sent).
 *
 * The Google step is a RECENT SIGN-IN CONFIRMATION, not reauthentication:
 * Google may answer from an existing session (access-auth.ts,
 * ACCOUNT_DELETION_REQUEST_AUTH_TIME). The pages say so. Its browser binding
 * is the SameSite=Lax flow cookie plus the HMAC state, because Google's
 * callback is a cross-site top-level GET that a Strict cookie would miss.
 * Cookies set ON the callback response are Strict: they are read only by the
 * later same-site POST. Every cookie here is `__Host-` with Path=/.
 * 2. `POST /account/delete/sign-in` starts that sign-in (access-auth.ts).
 * 3. The callback shows the destructive confirmation for the signed-in
 *    account. Its form carries a signed intent bound to a SameSite=Strict
 *    cookie; nothing about the person is stored for this step.
 * 4. `POST /account/delete/confirm` requires the cookie, the signature, a
 *    sign-in no older than the window and the typed word DELETE, then runs
 *    the deletion and hands the browser a signed status token for that
 *    account and operation.
 * 5. `GET /account/delete/status` reads the operation's receipt from the
 *    account itself, never from a device directory; `POST
 *    /account/delete/continue` resumes exactly that operation. "Deleted" is
 *    shown only when the account reports a completed receipt. */
import { beginAccountDeletionSignIn, completeAccountDeletionSignIn, type DeletionFreshness, type GoogleOAuthEnv } from "./access-auth.ts";
import { isOperationId, type AccountDeletionOutcome, type PublicDeletionStatus } from "./account-deletion.ts";
import { describeClientFailure, htmlPageHeaders, readBrowserForm } from "./http-security.ts";

/** The account's Durable Object, as the pages see it. Everything runs there
 * (account-deletion.ts); the pages only start, continue and read it. */
export type AccountDeletionWebAuthority = {
  requestAccountDeletion(): Promise<AccountDeletionOutcome>;
  continueAccountDeletion(operationId: string): Promise<AccountDeletionOutcome | null>;
  accountDeletionStatus(): Promise<PublicDeletionStatus>;
};
export type AccountDeletionWebDeps = {
  /** The account's Durable Object, addressed by the authenticated subject only. */
  authorityFor(subject: string): AccountDeletionWebAuthority;
  /** False in single-owner compatibility mode, which has no per-account namespace. */
  enabled: boolean;
  styles: string;
  now?: () => number;
  log?: (line: string) => void;
};

const START_COOKIE = "__Host-qcrc-delete-start";
const INTENT_COOKIE = "__Host-qcrc-delete";
const STATUS_COOKIE = "__Host-qcrc-delete-status";
/** The confirmation must follow the sign-in within this window. */
export const DELETION_CONFIRM_WINDOW_MS = 10 * 60_000;
/** How long a browser can read the status of the operation it started. */
export const DELETION_STATUS_TOKEN_MS = 7 * 24 * 60 * 60_000;
const START_TTL_SECONDS = 10 * 60;
export const DELETE_CONFIRMATION_WORD = "DELETE";

type Intent = { s: string; e: string; f: DeletionFreshness; v: number; n: string; x: number };
type StatusToken = { s: string; o: string; x: number };

function base64url(bytes: Uint8Array): string {
  let text = "";
  for (const byte of bytes) text += String.fromCharCode(byte);
  return btoa(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}
function fromBase64url(value: string): string {
  const normalized = value.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
  return new TextDecoder().decode(Uint8Array.from(atob(normalized), (character) => character.charCodeAt(0)));
}
function randomToken(bytes = 24): string { return base64url(crypto.getRandomValues(new Uint8Array(bytes))); }
function same(left: string | null | undefined, right: string): boolean {
  if (!left || left.length !== right.length) return false;
  let difference = 0;
  for (let index = 0; index < right.length; index += 1) difference |= left.charCodeAt(index) ^ right.charCodeAt(index);
  return difference === 0;
}
function cookie(request: Request, name: string): string | null {
  const value = request.headers.get("cookie")?.split(";").map((item) => item.trim()).find((item) => item.startsWith(`${name}=`));
  return value ? value.slice(name.length + 1) : null;
}
async function hmac(value: string, secret: string): Promise<string> {
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(`qcrc-account-deletion:${secret}`), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return base64url(new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(value))));
}
async function sign(kind: string, value: object, secret: string): Promise<string> {
  const body = base64url(new TextEncoder().encode(JSON.stringify(value)));
  return `${kind}.${body}.${await hmac(`${kind}.${body}`, secret)}`;
}
async function verify<T>(kind: string, token: string | null | undefined, secret: string): Promise<T | null> {
  if (!token) return null;
  const parts = token.split(".");
  if (parts.length !== 3 || parts[0] !== kind) return null;
  if (!same(parts[2], await hmac(`${kind}.${parts[1]}`, secret))) return null;
  try { return JSON.parse(fromBase64url(parts[1])) as T; } catch { return null; }
}
function escapeHtml(value: unknown): string {
  return String(value ?? "").replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character] ?? character);
}
function page(title: string, body: string, styles: string, status = 200, cookies: string[] = []): Response {
  const headers = new Headers(htmlPageHeaders());
  for (const value of cookies) headers.append("set-cookie", value);
  return new Response(`<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${escapeHtml(title)}</title><style>${styles}.danger{border-color:${QC_COLORS.app.danger}!important;background:${QC_COLORS.app.danger}!important;color:${QC_COLORS.app.canvas}!important}.list li{margin:.35rem 0;line-height:1.45}</style></head><body><main><div class="badge">QC Remote by Freevia</div>${body}</main></body></html>`, { status, headers });
}

const WHAT_IS_DELETED = `<ul class="list">
<li>Every computer and phone paired to this account, and their access credentials</li>
<li>Every AI app linked to this account (its access is revoked)</li>
<li>Recent AI-app activity, including connecting IP address and approximate city and country</li>
<li>The daily remote command count and any pending pairing code</li>
</ul>`;
const WHAT_IS_NOT = `<p class="help">Not affected: your Google account itself, and anything on your Quad Cortex or in the QC Remote apps on your devices. Cloudflare, which hosts the relay, keeps a sample of about 1% of request logs under its own terms, for a period Freevia has not yet confirmed; the relay cannot delete those. After deletion the relay keeps a deletion receipt (an operation number and its times) for 31 days so the deletion can be confirmed, and a counter with no personal details so that nothing issued before the deletion can ever work again. While cleanup is still in progress it also keeps the Google account identifier it needs to finish. A pairing you started but did not finish expires on its own within 10 minutes.</p>`;

function unavailable(styles: string): Response {
  return page("Not available", `<h1>Not available</h1><p>Remote account deletion is not available on this relay.</p>`, styles, 404);
}

function failurePage(cause: unknown, deps: AccountDeletionWebDeps, context: string): Response {
  const { message, correlationId } = describeClientFailure(cause, { status: 400, fallback: "The request could not be completed. Start the deletion again.", context, log: deps.log });
  return page("Deletion not started", `<h1>Deletion not started</h1><p>${escapeHtml(message)}</p><p class="help">Reference ${escapeHtml(correlationId)}.</p><form method="get" action="/account/delete"><button class="button" type="submit">Start again</button></form>`, deps.styles, 400);
}

/** `GET /account/delete`. */
export function showAccountDeletionStart(deps: AccountDeletionWebDeps): Response {
  if (!deps.enabled) return unavailable(deps.styles);
  const nonce = randomToken();
  return page("Delete remote account", `<h1>Delete your QC Remote remote account</h1>
<p>This deletes everything the QC Remote relay (operated by Freevia) keeps for the Google account you choose next:</p>${WHAT_IS_DELETED}${WHAT_IS_NOT}
<p>Next, Google confirms which account you are signed in to (it may not ask for your password if you are already signed in). Then you confirm on the next page. Nothing is deleted until you confirm.</p>
<form method="post" action="/account/delete/sign-in"><input type="hidden" name="nonce" value="${escapeHtml(nonce)}"><button class="button primary" type="submit">Continue with Google</button></form>
<p class="help">You can also ask by email: privacy@freevia.org.</p>`, deps.styles, 200,
  [`${START_COOKIE}-${nonce}=${nonce}; HttpOnly; Secure; Path=/; SameSite=Strict; Max-Age=${START_TTL_SECONDS}`]);
}

/** `POST /account/delete/sign-in`: the page's CSRF token, then Google. */
export async function startAccountDeletionSignIn(request: Request, env: GoogleOAuthEnv, deps: AccountDeletionWebDeps): Promise<Response> {
  if (!deps.enabled) return unavailable(deps.styles);
  const form = await readBrowserForm(request);
  const nonce = typeof form?.get("nonce") === "string" ? form.get("nonce") as string : "";
  if (!nonce || !same(cookie(request, `${START_COOKIE}-${nonce}`), nonce)) {
    return page("Start again", `<h1>Start again</h1><p>This request did not come from the deletion page in this browser.</p><form method="get" action="/account/delete"><button class="button" type="submit">Open the deletion page</button></form>`, deps.styles, 400);
  }
  return beginAccountDeletionSignIn(request, env, (deps.now ?? Date.now)());
}

/** `GET /auth/google/delete/callback`: the destructive confirmation. */
export async function showAccountDeletionConfirmation(request: Request, env: GoogleOAuthEnv, deps: AccountDeletionWebDeps): Promise<Response> {
  if (!deps.enabled) return unavailable(deps.styles);
  const now = (deps.now ?? Date.now)();
  let signedIn: Awaited<ReturnType<typeof completeAccountDeletionSignIn>>;
  try { signedIn = await completeAccountDeletionSignIn(request, env, now); }
  catch (cause) { return failurePage(cause, deps, "account-deletion-callback"); }
  return confirmationPage(signedIn.sub, signedIn.email, signedIn.freshness, signedIn.verifiedAt, env, deps, now);
}

async function confirmationPage(sub: string, email: string, freshness: Intent["f"], verifiedAt: number, env: GoogleOAuthEnv, deps: AccountDeletionWebDeps, now: number, notice = ""): Promise<Response> {
  const nonce = randomToken();
  const intent: Intent = { s: sub, e: email, f: freshness, v: verifiedAt, n: nonce, x: verifiedAt * 1000 + DELETION_CONFIRM_WINDOW_MS };
  const token = await sign("qcrc-delete-intent-v1", intent, env.COOKIE_ENCRYPTION_KEY);
  const maxAge = Math.max(1, Math.floor((intent.x - now) / 1000));
  return page("Confirm deletion", `<h1>Delete this remote account?</h1>${notice}
<div class="account"><strong>${escapeHtml(email)}</strong><br><small>${freshness === "fresh-authentication" ? "Google confirmed you signed in again just now." : "Google confirmed this account is signed in, in this browser, just now."}</small></div>
<p>This permanently deletes, for this Google account:</p>${WHAT_IS_DELETED}
<p class="warn" role="alert">This cannot be undone. Every paired device stops working with remote access immediately and has to be paired again to use it.</p>
<form method="post" action="/account/delete/confirm"><input type="hidden" name="intent" value="${escapeHtml(token)}">
<label for="confirm">Type ${DELETE_CONFIRMATION_WORD} to confirm</label><input class="server-address" id="confirm" name="confirm" autocomplete="off" required>
<div class="actions"><button class="button danger" type="submit">Delete remote account</button></div></form>
<form method="get" action="/"><button class="button" type="submit">Cancel</button></form>`, deps.styles, 200,
  [`${INTENT_COOKIE}=${nonce}; HttpOnly; Secure; Path=/; SameSite=Strict; Max-Age=${maxAge}`]);
}

async function statusCookie(sub: string, operationId: string, env: GoogleOAuthEnv, now: number): Promise<string> {
  const token = await sign("qcrc-delete-status-v1", { s: sub, o: operationId, x: now + DELETION_STATUS_TOKEN_MS } satisfies StatusToken, env.COOKIE_ENCRYPTION_KEY);
  return `${STATUS_COOKIE}=${token}; HttpOnly; Secure; Path=/; SameSite=Strict; Max-Age=${DELETION_STATUS_TOKEN_MS / 1000}`;
}

function statusRedirect(cookies: string[]): Response {
  const headers = new Headers({ location: "/account/delete/status", "cache-control": "no-store" });
  for (const value of cookies) headers.append("set-cookie", value);
  return new Response(null, { status: 303, headers });
}

/** `POST /account/delete/confirm`. */
export async function confirmAccountDeletion(request: Request, env: GoogleOAuthEnv, deps: AccountDeletionWebDeps): Promise<Response> {
  if (!deps.enabled) return unavailable(deps.styles);
  const now = (deps.now ?? Date.now)();
  const form = await readBrowserForm(request);
  const intent = await verify<Intent>("qcrc-delete-intent-v1", typeof form?.get("intent") === "string" ? form.get("intent") as string : null, env.COOKIE_ENCRYPTION_KEY);
  const refuse = (message: string) => page("Deletion not started", `<h1>Nothing was deleted</h1><p>${escapeHtml(message)}</p><form method="get" action="/account/delete"><button class="button" type="submit">Start again</button></form>`, deps.styles, 400);
  if (!intent || typeof intent.s !== "string" || !intent.s || typeof intent.n !== "string") return refuse("This confirmation is not valid. Start again.");
  if (!same(cookie(request, INTENT_COOKIE), intent.n)) return refuse("This confirmation was not made on the deletion page in this browser.");
  if (!Number.isFinite(intent.x) || now > intent.x || now - intent.v * 1000 > DELETION_CONFIRM_WINDOW_MS) return refuse(`The Google sign-in is older than ${DELETION_CONFIRM_WINDOW_MS / 60_000} minutes. Sign in again to confirm.`);
  if (form?.get("confirm") !== DELETE_CONFIRMATION_WORD) {
    return confirmationPage(intent.s, intent.e, intent.f, intent.v, env, deps, now, `<p class="warn" role="alert">Type ${DELETE_CONFIRMATION_WORD} in capitals to confirm. Nothing was deleted.</p>`);
  }
  let outcome: AccountDeletionOutcome | null = null;
  try { outcome = await deps.authorityFor(intent.s).requestAccountDeletion(); }
  catch (cause) {
    describeClientFailure(cause, { status: 503, fallback: "", context: "account-deletion-start", log: deps.log });
    // The mutation may already have revoked access before the RPC failed.
    // Recover the account's receipt without submitting a second mutation.
    try {
      const current = await deps.authorityFor(intent.s).accountDeletionStatus();
      // Only these pending steps still fence account reuse. Completed and
      // legacy-reconciliation receipts can predate this failed request while
      // new-generation access already exists. Without the RPC result we
      // cannot claim those receipts belong to this confirmation.
      if (current.status === "pending" && ["provider-writes", "grants", "directory"].includes(current.step) && isOperationId(current.operationId)) {
        return statusRedirect([
          `${INTENT_COOKIE}=; HttpOnly; Secure; Path=/; SameSite=Strict; Max-Age=0`,
          await statusCookie(intent.s, current.operationId, env, now),
        ]);
      }
    } catch (statusCause) {
      describeClientFailure(statusCause, { status: 503, fallback: "", context: "account-deletion-start-status", log: deps.log });
    }
  }
  return outcomeResponse(outcome, intent.s, env, deps, now);
}

async function outcomeResponse(outcome: AccountDeletionOutcome | null, sub: string, env: GoogleOAuthEnv, deps: AccountDeletionWebDeps, now: number): Promise<Response> {
  const expire = `${INTENT_COOKIE}=; HttpOnly; Secure; Path=/; SameSite=Strict; Max-Age=0`;
  if (!outcome) {
    return page("Deletion status unavailable", `<h1>Deletion status unavailable</h1><p>The relay could not confirm the deletion result. Remote access may already be revoked and cleanup may be in progress. Return to the deletion page to try again.</p><form method="get" action="/account/delete"><button class="button" type="submit">Return to deletion page</button></form>`, deps.styles, 503, [expire]);
  }
  return statusRedirect([expire, await statusCookie(sub, outcome.operationId, env, now)]);
}

async function readStatusToken(request: Request, env: GoogleOAuthEnv, now: number): Promise<StatusToken | null> {
  const token = await verify<StatusToken>("qcrc-delete-status-v1", cookie(request, STATUS_COOKIE), env.COOKIE_ENCRYPTION_KEY);
  if (!token || typeof token.s !== "string" || !token.s || !isOperationId(token.o) || !Number.isFinite(token.x) || now > token.x) return null;
  return token;
}

/** `accessRevoked` describes credentials issued before this deletion, not
 * the current-generation fence: legacy reconciliation permits account reuse.
 * `status:"deleted"` is the second level (cleanup complete). */
export type OperationStatus = { operationId: string; status: "pending" | "deleted" | "superseded"; deletedAt?: number; startedAt?: number; step?: string; accessRevoked?: true; until?: number | null };

/** What the account reports about exactly this operation. */
export function operationStatus(operationId: string, reported: PublicDeletionStatus): OperationStatus {
  if (reported.status !== "none" && reported.operationId === operationId) {
    return reported.status === "deleted"
      ? { operationId, status: "deleted", startedAt: reported.startedAt, deletedAt: reported.deletedAt }
      : { operationId, status: "pending", startedAt: reported.startedAt, step: reported.step, accessRevoked: true, ...(reported.until !== undefined ? { until: reported.until } : {}) };
  }
  // No receipt for this operation: it was replaced by a later one, or its
  // receipt outlived its retention. Never reported as deleted.
  return { operationId, status: "superseded" };
}

/** `GET /account/delete/status`. JSON when asked for it. */
export async function showAccountDeletionStatus(request: Request, env: GoogleOAuthEnv, deps: AccountDeletionWebDeps): Promise<Response> {
  if (!deps.enabled) return unavailable(deps.styles);
  const now = (deps.now ?? Date.now)();
  const token = await readStatusToken(request, env, now);
  const wantsJson = (request.headers.get("accept") ?? "").includes("application/json");
  if (!token) {
    if (wantsJson) return new Response(JSON.stringify({ error: "No deletion started in this browser" }), { status: 401, headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" } });
    return page("No deletion in progress", `<h1>No deletion in this browser</h1><p>This browser has no remote account deletion to show. Start one from the deletion page.</p><form method="get" action="/account/delete"><button class="button" type="submit">Open the deletion page</button></form>`, deps.styles, 401);
  }
  let status: OperationStatus;
  try { status = operationStatus(token.o, await deps.authorityFor(token.s).accountDeletionStatus()); }
  catch (cause) {
    describeClientFailure(cause, { status: 503, fallback: "", context: "account-deletion-status", log: deps.log });
    if (wantsJson) return new Response(JSON.stringify({ operationId: token.o, status: "unknown", retryable: true }), { status: 503, headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store", "retry-after": "30" } });
    return page("Status unavailable", `<h1>Status unavailable</h1><p>The relay could not read the deletion status. Reload this page in a minute.</p>`, deps.styles, 503);
  }
  if (wantsJson) return new Response(JSON.stringify(status), { status: status.status === "pending" ? 202 : 200, headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" } });
  const reference = `<p class="help">Operation ${escapeHtml(token.o)}</p>`;
  if (status.status === "deleted") {
    return page("Remote account deleted", `<h1>Remote account deleted</h1><p>Cleanup is complete: everything the relay kept for this Google account has been deleted (${escapeHtml(new Date(status.deletedAt!).toISOString())}). Paired devices no longer have remote access. In each QC Remote app, open REMOTE and press Unpair to remove the stored pairing from that device.</p>${reference}`, deps.styles);
  }
  if (status.status === "pending") {
    const why = status.step === "provider-writes"
      ? "An AI-app sign-in for this account was still being saved when the deletion started. The relay waits until it can confirm what that sign-in stored, deletes it, and keeps trying on its own; if this stays here for more than a day, write to privacy@freevia.org."
      : status.step === "grants"
        ? "The relay has not yet confirmed that every AI-app authorization is gone. It keeps trying on its own."
        : status.step === "directory"
          ? "Some relay directory entries are still being removed. The relay keeps trying on its own."
          : `Everything this relay version recorded is deleted, and you can use remote access again with a fresh account. AI-app authorizations that an older relay version may have stored outside that record are being removed and checked again${status.until ? `; this can finish no earlier than ${escapeHtml(new Date(status.until).toISOString().slice(0, 10))}` : "; this cannot finish until Freevia can establish that no older relay version can still write one and that every one it may have written has expired, which is not yet established"}.`;
    return page("Cleanup in progress", `<h1>Remote access removed. Cleanup in progress</h1><p>Devices and AI apps linked before this deletion have lost access. ${why} Nothing is reported as deleted until the relay has confirmed all of it.</p>
<form method="post" action="/account/delete/continue"><input type="hidden" name="operation" value="${escapeHtml(token.o)}"><button class="button danger" type="submit">Continue deletion</button></form>${reference}`, deps.styles, 202);
  }
  return page("Deletion replaced", `<h1>No record of this deletion</h1><p>The relay has no receipt for this operation; a later deletion may have replaced it. Start a new deletion to be sure.</p><form method="get" action="/account/delete"><button class="button" type="submit">Open the deletion page</button></form>${reference}`, deps.styles, 409);
}

/** `POST /account/delete/continue`: resumes exactly the operation in the
 * browser's status token, for exactly that account. It cannot start a new
 * deletion: if the account reports no pending operation with that id, it
 * only shows the status. */
export async function continueAccountDeletion(request: Request, env: GoogleOAuthEnv, deps: AccountDeletionWebDeps): Promise<Response> {
  if (!deps.enabled) return unavailable(deps.styles);
  const now = (deps.now ?? Date.now)();
  const token = await readStatusToken(request, env, now);
  const form = await readBrowserForm(request);
  if (!token || !same(typeof form?.get("operation") === "string" ? form.get("operation") as string : null, token.o)) {
    return page("Start again", `<h1>Start again</h1><p>This request did not come from the status page in this browser.</p><form method="get" action="/account/delete"><button class="button" type="submit">Open the deletion page</button></form>`, deps.styles, 400);
  }
  const authority = deps.authorityFor(token.s);
  try {
    const current = operationStatus(token.o, await authority.accountDeletionStatus());
    if (current.status === "pending") await authority.continueAccountDeletion(token.o);
  } catch (cause) { describeClientFailure(cause, { status: 503, fallback: "", context: "account-deletion-continue", log: deps.log }); }
  return statusRedirect([]);
}
