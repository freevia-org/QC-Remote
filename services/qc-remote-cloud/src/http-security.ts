/** Response hardening shared by every HTML page and every failure this worker
 * reports, kept out of `index.ts` so it can be exercised without the Workers
 * runtime.
 *
 * Two hardening measures live here. The pairing and
 * landing pages could be framed by any site, while the OAuth consent page
 * already refused it; and internal exception text - a KV quota message, a
 * JWKS fetch failure, a Durable Object RPC error - was handed straight to the
 * browser or API client that triggered it. */

/** Headers for every HTML page the worker serves outside the consent screen.
 *
 * `frame-ancestors 'none'` and `X-Frame-Options: DENY` match the consent page:
 * a pairing page decides which account a computer joins, and a page that can
 * be framed can be dressed up as something else. The policy deliberately sets
 * nothing but `frame-ancestors`, because these pages carry small inline
 * handlers (copy-to-clipboard, select-on-focus) a stricter default would
 * silently break. */
export function htmlPageHeaders(): Record<string, string> {
  return {
    "content-type": "text/html; charset=utf-8",
    "cache-control": "no-store",
    "referrer-policy": "no-referrer",
    "content-security-policy": "frame-ancestors 'none'",
    "x-frame-options": "DENY"
  };
}

/** A failure whose message was written for the person who will read it.
 *
 * Everything else that reaches a catch block is treated as internal: its text
 * is logged, never returned. Throwing this is the one way a handler opts a
 * message into a response. Durable Object RPC does not preserve the class, so
 * messages crossing that boundary are matched by text instead (see
 * `clientFailure`'s `publicMessages`). */
export class PublicError extends Error {
  readonly publicMessage = true;
  constructor(message: string) {
    super(message);
    this.name = "PublicError";
  }
}

export function isPublicError(cause: unknown): cause is PublicError {
  return cause instanceof Error && (cause as Partial<PublicError>).publicMessage === true;
}

/** A short id that ties a client-visible failure to its server-side log line. */
export function correlationId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  return Array.from(bytes, (part) => part.toString(16).padStart(2, "0")).join("");
}

/** Strip anything shaped like a credential before a message is logged.
 *
 * Bearer tokens, pairing codes, OAuth codes and state nonces are all long runs
 * of URL-safe characters; nothing a person needs to read in a log is. */
export function redactForLog(value: string): string {
  return value
    .replace(/Bearer\s+\S+/gi, "Bearer [redacted]")
    .replace(/[A-Za-z0-9_\-]{24,}/g, "[redacted]")
    .slice(0, 500);
}

export type FailureLogger = (line: string) => void;

export type ClientFailureOptions = {
  status: number;
  /** What the client is told when the cause is not public. */
  fallback: string;
  /** Where the failure happened, for the log line only. */
  context: string;
  /** Exact messages from across an RPC boundary that are safe to repeat. */
  publicMessages?: readonly string[];
  /** For some of those repeatable messages, a stable machine-readable code and
   * the status (and optional Retry-After, in seconds) that replace the
   * defaults. Applied only when the message is repeated. */
  publicCodes?: Readonly<Record<string, { code: string; status: number; retryAfterSeconds?: number }>>;
  log?: FailureLogger;
};

/** The message and correlation id a client is given for `cause`, after logging
 * the internal detail. */
export function describeClientFailure(cause: unknown, options: ClientFailureOptions): { message: string; correlationId: string; status: number; code?: string; retryAfterSeconds?: number } {
  const id = correlationId();
  const detail = cause instanceof Error ? `${cause.name}: ${cause.message}` : String(cause);
  const log = options.log ?? ((line: string) => console.error(line));
  const spoken = cause instanceof Error && (isPublicError(cause) || options.publicMessages?.includes(cause.message) === true);
  const coded = spoken && Object.hasOwn(options.publicCodes ?? {}, cause.message) ? options.publicCodes![cause.message] : undefined;
  const status = coded?.status ?? options.status;
  log(JSON.stringify({ event: "client-failure", correlationId: id, context: options.context, status, detail: redactForLog(detail) }));
  const message = spoken ? cause.message : options.fallback;
  return { message, correlationId: id, status, ...(coded ? { code: coded.code, ...(coded.retryAfterSeconds !== undefined ? { retryAfterSeconds: coded.retryAfterSeconds } : {}) } : {}) };
}

/** A JSON failure that never carries internal exception text. */
export function clientFailure(cause: unknown, options: ClientFailureOptions): Response {
  const { message, correlationId: id, status, code, retryAfterSeconds } = describeClientFailure(cause, options);
  return new Response(JSON.stringify({ error: message, ...(code ? { code } : {}), correlationId: id }), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store", "x-correlation-id": id, ...(retryAfterSeconds !== undefined ? { "retry-after": String(retryAfterSeconds) } : {}) }
  });
}

/** A plain-text failure for browser routes, with the same guarantees. */
export function clientFailureText(cause: unknown, options: ClientFailureOptions): Response {
  const { message, correlationId: id, status } = describeClientFailure(cause, options);
  return new Response(`${message} (reference ${id})`, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store", "x-correlation-id": id }
  });
}
/** A malformed browser form is an invalid request, not an unhandled Worker error. */
export async function readBrowserForm(request: Request): Promise<FormData | null> {
  try {
    return await request.formData();
  } catch {
    return null;
  }
}

/** Bound both connection and response-body reads for upstream identity calls. */
export async function fetchIdentityJson(
  url: string, init: RequestInit = {}, fetcher: typeof fetch = fetch, timeoutMs = 10_000,
): Promise<unknown> {
  const deadline = AbortSignal.timeout(timeoutMs);
  const signal = init.signal ? AbortSignal.any([init.signal, deadline]) : deadline;
  const response = await fetcher(url, { ...init, signal });
  if (!response.ok) throw new PublicError("QC Remote could not complete sign-in. Please try again later.");
  return response.json();
}
