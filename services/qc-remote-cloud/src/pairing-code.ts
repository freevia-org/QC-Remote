/** What a browser must satisfy before it may begin a pairing sign-in.
 *
 * A pairing session is unauthenticated by design: the host has no credential
 * yet, and requiring one would put an owner secret into the app. It used to
 * carry six digits as well, displayed by the app that created the session and
 * repeated by the browser, so a pairing link sent to someone else was inert.
 *
 * That guarded a host which would pair with whoever signed in. A host now
 * refuses a receipt naming an account other than the one it already holds, so
 * a link forwarded from a paired device cannot take it, and an unpaired one
 * has nothing yet to take. Pairing is one scan and one sign-in, and what is
 * left to establish is that the session exists, has not expired, and has not
 * already been used: a session completes once, and a second sign-in through
 * the same link is refused rather than allowed to replace the first.
 */

export type PairingSessionCheck =
  | { ok: true }
  | { ok: false; reason: "invalid-session" | "expired" | "used" };

export interface HostPairingRecord {
  expiresAt: number;
  completedAt?: number;
}

export function isHostSessionId(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9_-]{20,128}$/.test(value);
}

export function checkHostPairingSession(
  record: HostPairingRecord | null | undefined,
  now = Date.now()
): PairingSessionCheck {
  if (!record) return { ok: false, reason: "invalid-session" };
  if (record.expiresAt < now) return { ok: false, reason: "expired" };
  if (typeof record.completedAt === "number") return { ok: false, reason: "used" };
  return { ok: true };
}

/** Six digits minted only so an app built before they were retired still
 * starts a pairing.
 *
 * Those apps validate the field's shape in the session response and refuse to
 * open the pairing page without it, so dropping it from the response would
 * stop every install in the field from pairing the moment the relay deploys.
 * Nothing checks the value any more - the pairing page has no field to type it
 * into - and this can go once no such app remains. */
export function legacyVerificationDigits(random: (length: number) => Uint32Array = (length) => crypto.getRandomValues(new Uint32Array(length))): string {
  const value = random(1)[0] % 10 ** 6;
  return value.toString().padStart(6, "0");
}
