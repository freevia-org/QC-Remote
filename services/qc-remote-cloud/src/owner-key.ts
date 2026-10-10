/**
 * Maps an authenticated account to its isolated Durable Object namespace.
 *
 * Single-owner compatibility mode retains one shared namespace; production
 * must never fall back to it, because that would cross account
 * boundaries if an identity policy admitted more than one person.
 */
export type RelayEnvironment = "private" | "production";

/** Compatibility privileges require the exact opt-in; missing or misspelled
 * configuration inherits production isolation and protection. */
export function relayEnvironment(value: unknown): RelayEnvironment {
  return value === "private" ? "private" : "production";
}

export const PRIVATE_OWNER_KEY = "private";

const OWNER_KEY_PREFIX = "owner:";

/** The account behind an owner key, where the key still carries one.
 *
 * A route that holds a device's owner key can name the account whose connector
 * grants must be retired without a second lookup. The inverse lives beside the
 * function it inverts so the two cannot drift: change the format below and this
 * stops compiling against it.
 *
 * Compatibility mode collapses every account onto one namespace and records no subject,
 * so there is nothing to recover and nothing may be guessed - the caller is
 * expected to do nothing rather than act on a wrong account. */
export function ownerSubject(environment: RelayEnvironment, ownerKey: string): string | undefined {
  if (relayEnvironment(environment) === "private") return undefined;
  if (!ownerKey.startsWith(OWNER_KEY_PREFIX)) return undefined;
  return ownerKey.slice(OWNER_KEY_PREFIX.length) || undefined;
}

export function ownerSessionKey(environment: RelayEnvironment, subject: string): string {
  if (relayEnvironment(environment) === "private") return PRIVATE_OWNER_KEY;
  const identity = subject.trim();
  if (!identity) throw new Error("QC Remote identity is required");
  return `${OWNER_KEY_PREFIX}${identity}`;
}
