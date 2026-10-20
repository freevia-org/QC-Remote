/** Deleting a whole remote account: every record the relay keeps for one
 * Google subject, not just one device.
 *
 * Google sign-in creates a relay account keyed to the Google subject: the
 * Durable Object `owner:<sub>` (device records, the pairing offer, the daily
 * usage counter, MCP activity), KV directory entries pointing at it, and the
 * AI-app grants and tokens the OAuth provider files as `access-<sub>`. This
 * module removes all of it. It runs inside the account's own Durable Object,
 * which also retries it from its alarm; the OAuth provider's grant API and
 * KV are passed in, so every step can be driven against fakes.
 *
 * WHY "DELETED" IS TRUE WHEN IT IS SAID (the proof; also in the plan file)
 *
 * Account data lives in this object's storage and in KV (directory and
 * pairing entries, and the provider's `grant:<userId>:*` / `token:<userId>:*`).
 *
 * Object storage: every write that can add account data (offers, pairing
 * redemption, connects, usage, MCP activity) checks the deletion state inside
 * the transaction that commits it, and `begin` purges the rest in one
 * transaction. After `pending` commits, none of them can commit.
 *
 * Provider KV: the OAuth provider (0.10.4) performs every grant/token
 * mutation - authorization, code exchange, refresh rotation, revocation,
 * cleanup - through the KV binding it is given, and that binding is wrapped
 * (`fenceProviderKv`). Each grant/token put is a journalled operation:
 *
 *  J1 admit: one transaction here refuses while a deletion is pending
 *     (and refuses a stale-generation grant, or a token whose grant is not
 *     journalled at the current generation), and records the exact key and an
 *     open admission BEFORE the put is issued;
 *  J2 the put is issued;
 *  J3 settle: after the put returned or failed, one transaction closes the
 *     admission; if a deletion began meanwhile the writer deletes the key.
 *
 * Deletion, after `pending`, admits nothing (J1). It waits - with no
 * timeout - until every admission is closed (J3), then deletes every
 * journalled key exactly, revokes every grant the provider lists, deletes
 * every key under the subject's grant/token prefixes (legacy data written
 * before the journal existed), confirms both prefixes list empty, and only
 * then writes the receipt. Every put that could carry the account's data was
 * therefore either refused, or journalled and observed to have returned
 * before deletion deleted its key: no put can land after the delete that
 * follows it, because the put had already been acknowledged.
 *
 * A writer that dies between J1 and J3 leaves its admission open. Its put
 * may or may not have reached KV, and nothing can prove which. The deletion
 * then stays PENDING ("cleanup in progress"): it deletes that exact key on
 * every alarm, the page says so, and it never reports "deleted" for it.
 *
 * Stated assumptions, both about Cloudflare KV: an acknowledged put is
 * ordered before a delete of the same key issued after the acknowledgement;
 * and legacy keys (written by a relay version without the journal) are old
 * enough to appear in a list. */
import { connectorGrantUser, type ConnectorGrantRegistry } from "./connector-grants.ts";
import { LEGACY_REFRESH_TOKEN_TTL_SECONDS } from "./oauth-lifetimes.ts";
import relayProfile from "../../../contracts/public/qc-relay-profile.v1.json" with { type: "json" };

export const ACCOUNT_DELETION_KEY = "account-deletion-v1";
/** Never deleted and never reset: a grant or callback from before a deletion
 * must not become valid again because the counter started over. One integer. */
export const ACCOUNT_GENERATION_KEY = "account-generation-v1";
/** The provider-write journal: open admissions and journalled keys. */
export const PROVIDER_WRITES_KEY = "provider-writes-v1";
export const PROVIDER_WRITES_SWEEP_KEY = "provider-writes-retention-at-v1";
export const PROVIDER_WRITES_SWEEP_ATTEMPTS_KEY = "provider-writes-retention-attempts-v1";
export const PROVIDER_WRITES_SWEEP_RETRY_MS = 5 * 60_000;
export const PROVIDER_WRITES_SWEEP_RETRY_MAX_MS = 24 * 60 * 60_000;
const PROVIDER_WRITES_RETENTION_GRACE_MS = 24 * 60 * 60_000;
export const ACCOUNT_DELETION_RETRY_MS = 5 * 60_000;
/** A permanently unresolved provider write must remain fail-closed without
 * waking its Durable Object every five minutes forever. */
export const PROVIDER_WRITES_DELETION_RETRY_MAX_MS = 24 * 60 * 60_000;
const PROVIDER_WRITES_DELETION_RETRY_MAX_COUNT = 9;
/** Receipt retention is bounded; the permanent account-generation fence above
 * rejects pre-deletion grants even after this receipt is removed. */
export const ACCOUNT_DELETION_RECEIPT_MS = (LEGACY_REFRESH_TOKEN_TTL_SECONDS + 24 * 60 * 60) * 1_000;
export const ACCOUNT_DELETION_WEB_PATH = relayProfile.accountDeletionWebPath;
/** Operator evidence that un-journalled writers have DRAINED (plan 7.10):
 * `lastPossibleLegacyCompletionAt` is the latest moment any Worker version
 * that writes grants/tokens outside the journal could still complete a KV
 * put, established from runtime evidence (zero requests for every old
 * version in Workers analytics, plus the longest request duration observed
 * for them), not from a deploy timestamp; `maxLegacyLifetimeMs` is verified
 * for every old version's provider version and options. */
export type LegacyDrainAttestation = { lastPossibleLegacyCompletionAt: number; maxLegacyLifetimeMs: number; evidence: string };
/** EXPLICITLY null, and fail-closed: no such evidence exists. While null, no
 * deletion reports "deleted"; it stays in legacy reconciliation with access
 * revoked. Setting it requires that evidence, or a revised, bounded legacy
 * contract reviewed as such; it is not a tuning knob. */
export const LEGACY_DRAIN_ATTESTATION: LegacyDrainAttestation | null = null;
/** The longest a grant/token written by provider 0.10.4 with the HISTORICAL 30-day options can
 * live: 600 s as a code grant (oauth-provider.js 3664), then 30 days from the
 * exchange (1966; oauth-lifetimes.ts), never extended by refresh (2085-2094),
 * with a KV expiration clamp of at most 65 s (2766-2768). It does not bound
 * writers of other versions or options; the attestation must. */
export const PROVIDER_0_10_4_MAX_LIFETIME_MS = (600 + LEGACY_REFRESH_TOKEN_TTL_SECONDS + 65) * 1_000;
/** RETRY POLICY, not a proof of absence: a legacy sweep counts as clean only
 * this long after the last delete. KV list visibility is documented at about
 * 60 s plus any cacheTtl and is not guaranteed. */
export const KV_VISIBILITY_MARGIN_MS = 10 * 60_000;
export const LEGACY_RECONCILE_INTERVAL_MS = 60 * 60_000;
/** Refusal texts that cross the Durable Object RPC boundary; matched by text. */
export const ACCOUNT_DELETION_PENDING_MESSAGE = "This remote account is being deleted. Finish or wait for the deletion at https://qcrc.freevia.org/account/delete before signing in again.";
export const STALE_GENERATION_MESSAGE = "This AI app was signed in to a remote account that has since been deleted. Remove QC Remote from the app and connect it again.";
export const PROVIDER_WRITE_REFUSED_MESSAGE = "QC Remote refused to store this authorization because the remote account is being deleted or was deleted.";
const DEVICE_RECORD_PREFIX = "device:";
const DIRECTORY_CLEANUP_KEY = "device-directory-cleanup-v1";
const DEVICE_SWEEP_KEY = "device-retention-sweep-at-v1";
const DIRECTORY_CLEANUP_RETRY_MS = 60 * 60_000;
const PAIRING_OFFER_KEY = "pairing-offer";
const FENCE_KEYS = new Set([ACCOUNT_DELETION_KEY, ACCOUNT_GENERATION_KEY, PROVIDER_WRITES_KEY]);
const GRANT_REVOCATION_PASSES = 20;

export type DeletionStep = "provider-writes" | "grants" | "directory" | "legacy-reconciliation";
export type PendingAccountDeletion = {
  version: 1;
  status: "pending";
  operationId: string;
  startedAt: number;
  /** Needed to revoke the subject's grants from the alarm; dropped at finish. */
  subject: string;
  deviceIds: string[];
  offerCodeHash?: string;
  grantsRevokedAt?: number;
  retryAt?: number;
  step?: DeletionStep;
  /** Consecutive provider-write barrier retries; saturates at the 24-hour cap. */
  providerWritesRetryCount?: number;
};
/** No personal data. */
export type AccountDeletionReceipt = { version: 1; status: "deleted"; operationId: string; startedAt: number; deletedAt: number; retainUntil: number };
/** Access is revoked and every journalled key is deleted; keys the journal
 * cannot account for (written before it existed, or by an older Worker
 * version) are still being reconciled. The account is usable again under a
 * new generation; `subject` is kept only for the reconciliation sweeps. */
export type ReconcilingAccountDeletion = { version: 1; status: "reconciling"; operationId: string; startedAt: number; accessRevokedAt: number; subject: string; lastLegacyDeleteAt: number; retryAt: number;
  /** Every legacy grant this reconciliation revoked, with its observed expiry
   * in ms (null: unknown, which keeps the deletion pending). */
  seenLegacy?: Record<string, number | null> };
export type AccountDeletionState = PendingAccountDeletion | ReconcilingAccountDeletion | AccountDeletionReceipt;

export type DeletionTransaction = {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<boolean>;
  list<T>(options?: { prefix?: string }): Promise<Map<string, T>>;
  getAlarm(): Promise<number | null>;
  setAlarm(scheduledTime: number): Promise<void>;
  deleteAlarm(): Promise<void>;
};
export type DeletionStorage = { transaction<T>(callback: (transaction: DeletionTransaction) => Promise<T>): Promise<T> };
export type DeletionKv = { delete(key: string): Promise<void> };
/** The provider's KV, for the final sweep of the subject's grant and token keys. */
export type ProviderKv = DeletionKv & { list(options: { prefix: string; cursor?: string }): Promise<{ keys: Array<{ name: string }>; list_complete: boolean; cursor?: string }> };

export function deviceDirectoryKey(deviceId: string): string { return `qcrc:device:${deviceId}`; }
export function pairingDirectoryKey(codeHash: string): string { return `qcrc:pair:${codeHash}`; }

export function newOperationId(): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(16)), (part) => part.toString(16).padStart(2, "0")).join("");
}
export function isOperationId(value: unknown): value is string { return typeof value === "string" && /^[a-f0-9]{32}$/.test(value); }

const validTimestamp = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const validStringArray = (value: unknown): value is string[] => Array.isArray(value) && value.every((entry) => typeof entry === "string");

/** Strict shape guard reused by the read path and the retention inventory. */
export function validAccountDeletionState(value: unknown): value is AccountDeletionState {
  if (!value || typeof value !== "object") return false;
  const state = value as Partial<AccountDeletionState>;
  if (state.version !== 1 || !isOperationId(state.operationId) || !validTimestamp(state.startedAt)) return false;
  if (state.status === "deleted") return validTimestamp(state.deletedAt) && validTimestamp(state.retainUntil);
  if (state.status === "reconciling") {
    if (typeof state.subject !== "string" || !validTimestamp(state.accessRevokedAt) || !validTimestamp(state.lastLegacyDeleteAt) || !validTimestamp(state.retryAt)) return false;
    return state.seenLegacy === undefined || (
      state.seenLegacy !== null
      && typeof state.seenLegacy === "object"
      && !Array.isArray(state.seenLegacy)
      && Object.values(state.seenLegacy).every((expiresAt) => expiresAt === null || validTimestamp(expiresAt))
    );
  }
  return state.status === "pending"
    && validStringArray(state.deviceIds)
    && typeof state.subject === "string"
    && (state.offerCodeHash === undefined || typeof state.offerCodeHash === "string")
    && (state.grantsRevokedAt === undefined || validTimestamp(state.grantsRevokedAt))
    && (state.retryAt === undefined || validTimestamp(state.retryAt))
    && (state.providerWritesRetryCount === undefined || (Number.isSafeInteger(state.providerWritesRetryCount) && state.providerWritesRetryCount >= 0 && state.providerWritesRetryCount <= PROVIDER_WRITES_DELETION_RETRY_MAX_COUNT))
    && (state.step === undefined || ["provider-writes", "grants", "directory", "legacy-reconciliation"].includes(state.step));
}

export async function readAccountDeletion(transaction: Pick<DeletionTransaction, "get">): Promise<AccountDeletionState | null> {
  const stored = await transaction.get<unknown>(ACCOUNT_DELETION_KEY);
  return validAccountDeletionState(stored) ? stored : null;
}

export async function readAccountGeneration(transaction: Pick<DeletionTransaction, "get">): Promise<number> {
  return grantGeneration(await transaction.get<unknown>(ACCOUNT_GENERATION_KEY));
}

/** A grant's recorded generation; grants from before the fence carry none, which is 0. */
export function grantGeneration(value: unknown): number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : 0;
}

/** The check every account write makes inside its own transaction. */
export async function assertAccountWritable(transaction: Pick<DeletionTransaction, "get">, generation?: number): Promise<void> {
  if ((await readAccountDeletion(transaction))?.status === "pending") throw new Error(ACCOUNT_DELETION_PENDING_MESSAGE);
  if (generation !== undefined && grantGeneration(generation) !== await readAccountGeneration(transaction)) throw new Error(STALE_GENERATION_MESSAGE);
}

/** The account-write guard for device-retention.ts: a typed denial while a
 * deletion is pending, null to allow; a storage exception propagates. */
export async function accountDeletionDenial(transaction: Pick<DeletionTransaction, "get">): Promise<{ denied: true; message: string } | null> {
  return (await readAccountDeletion(transaction))?.status === "pending" ? { denied: true, message: ACCOUNT_DELETION_PENDING_MESSAGE } : null;
}

/** Whether an MCP request under `generation` may proceed. */
export async function admitMcpRequest(storage: DeletionStorage, generation: unknown): Promise<"ok" | "deleting" | "stale"> {
  return storage.transaction(async (transaction) => {
    if ((await readAccountDeletion(transaction))?.status === "pending") return "deleting";
    return grantGeneration(generation) === await readAccountGeneration(transaction) ? "ok" : "stale";
  });
}

// ---- Provider-write journal (the proof's KV half; plan section 7.10) ----

type OpenWrite = { key: string; admittedAt: number; state: "writing" | "compensating" };
type ProviderWrites = {
  /** "writing": admitted, the put's outcome is not known (in flight, threw,
   * or timed out). "compensating": the put returned but must not stay; an
   * acknowledged delete of the key is still owed. Neither ages out. */
  open: Record<string, OpenWrite>;
  /** Every grant/token key this account's provider writes touched and not yet
   * deleted. `deleteIntent`: the provider deleted the key while a writer of it
   * was still open; the entry stays until the last such writer resolves. */
  keys: Record<string, { expiresAt?: number; generation?: number; deleteIntent?: boolean; deletedAt?: number; legacy?: boolean }>;
};

/** Strict shape guard shared by runtime mutations and retention inventory.
 * The journal protects deletion correctness, so malformed persisted state is
 * an error rather than something mutation paths may silently normalize. */
export function validProviderWrites(value: unknown): value is ProviderWrites {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Partial<ProviderWrites>;
  if (!record.open || typeof record.open !== "object" || Array.isArray(record.open)
    || !record.keys || typeof record.keys !== "object" || Array.isArray(record.keys)) return false;
  const openValid = Object.entries(record.open).every(([admissionId, entry]) => isOperationId(admissionId)
    && Boolean(entry)
    && typeof entry === "object"
    && typeof entry.key === "string"
    && validTimestamp(entry.admittedAt)
    && (entry.state === "writing" || entry.state === "compensating"));
  const keysValid = Object.values(record.keys).every((entry) => Boolean(entry)
    && typeof entry === "object"
    && (entry.expiresAt === undefined || validTimestamp(entry.expiresAt))
    && (entry.generation === undefined || (Number.isSafeInteger(entry.generation) && Number(entry.generation) >= 0))
    && (entry.deleteIntent === undefined || typeof entry.deleteIntent === "boolean")
    && (entry.deletedAt === undefined || validTimestamp(entry.deletedAt))
    && (entry.legacy === undefined || typeof entry.legacy === "boolean"));
  return openValid && keysValid;
}

export class InvalidProviderWritesJournalError extends Error {
  constructor() {
    super("Provider-write journal is invalid; refusing account-deletion progress.");
    this.name = "InvalidProviderWritesJournalError";
  }
}

/** One Durable Object alarm serves deletion, device retention, socket expiry,
 * and MCP presence. A deletion update may move it earlier but never postpone
 * work another subsystem already scheduled. */
async function scheduleSharedAlarm(transaction: Pick<DeletionTransaction, "getAlarm" | "setAlarm">, deadline: number): Promise<void> {
  const alarm = await transaction.getAlarm();
  if (alarm === null || alarm > deadline) await transaction.setAlarm(deadline);
}

/** Re-arm deletion work without postponing an earlier shared alarm. A due
 * alarm may belong to another subsystem and must remain runnable; the alarm
 * currently being dispatched returns null and receives this fallback. */
export async function scheduleAccountDeletionRetry(
  transaction: DeletionTransaction,
  now: number,
  pending?: PendingAccountDeletion,
): Promise<number> {
  let retryAt = now + ACCOUNT_DELETION_RETRY_MS;
  if (pending?.retryAt !== undefined && pending.retryAt > now) {
    retryAt = pending.retryAt;
  } else if (pending?.step === "provider-writes") {
    const current = await readAccountDeletion(transaction);
    if (current?.status === "pending" && current.operationId === pending.operationId && current.step === "provider-writes") {
      const count = current.providerWritesRetryCount ?? 0;
      const nextCount = current.retryAt === undefined
        ? count
        : Math.min(PROVIDER_WRITES_DELETION_RETRY_MAX_COUNT, count + 1);
      const delay = Math.min(ACCOUNT_DELETION_RETRY_MS * 2 ** nextCount, PROVIDER_WRITES_DELETION_RETRY_MAX_MS);
      retryAt = now + delay;
    }
  }
  const alarm = await transaction.getAlarm();
  if (alarm === null || (alarm > now && alarm > retryAt)) await transaction.setAlarm(retryAt);
  return retryAt;
}

async function readProviderWrites(transaction: Pick<DeletionTransaction, "get">): Promise<ProviderWrites> {
  const stored = await transaction.get<unknown>(PROVIDER_WRITES_KEY);
  if (stored === undefined) return { open: {}, keys: {} };
  if (!validProviderWrites(stored)) throw new InvalidProviderWritesJournalError();
  return stored;
}

const openFor = (writes: ProviderWrites, key: string) => Object.values(writes.open).filter((entry) => entry.key === key);

/** The grant key a token key belongs to: `token:<userId>:<grantId>:<id>`. */
function grantKeyOfToken(key: string): string | null {
  const parts = key.split(":");
  return parts.length === 4 && parts[0] === "token" ? `grant:${parts[1]}:${parts[2]}` : null;
}

/** Whether a journalled key may stay, given the account's state now. */
function mayStay(writes: ProviderWrites, key: string, generation: number): boolean {
  const entry = writes.keys[key];
  if (!entry || entry.deleteIntent) return false;
  if (key.startsWith("grant:")) return entry.generation === generation;
  const grantKey = grantKeyOfToken(key);
  const grant = grantKey ? writes.keys[grantKey] : undefined;
  return Boolean(grant && !grant.deleteIntent && grant.generation === generation);
}

/** Whether a journal entry can be dropped: no open writer, and past the last
 * moment the key could exist or be rewritten (its KV expiry, or for a
 * tombstone of unknown expiry: never), plus a day. Persistent grant
 * tombstones must survive indefinitely to reject delayed writers. */
function prunable(writes: ProviderWrites, name: string, entry: ProviderWrites["keys"][string], now: number): boolean {
  if (openFor(writes, name).length > 0) return false;
  const until = entry.expiresAt;
  return until !== undefined && until + 24 * 60 * 60_000 <= now;
}

function providerWritePruneAt(entry: ProviderWrites["keys"][string]): number | undefined {
  const until = entry.expiresAt;
  return until === undefined ? undefined : until + PROVIDER_WRITES_RETENTION_GRACE_MS;
}

function nextProviderWritesSweep(writes: ProviderWrites, now: number): number | null {
  let next: number | null = null;
  for (const [name, entry] of Object.entries(writes.keys)) {
    const due = providerWritePruneAt(entry);
    if (due === undefined) continue;
    const overdue = Math.max(0, now - due);
    const exponent = overdue === 0 ? 0 : Math.floor(Math.log2(overdue / PROVIDER_WRITES_SWEEP_RETRY_MS + 1));
    const retryMs = Math.min(PROVIDER_WRITES_SWEEP_RETRY_MAX_MS, PROVIDER_WRITES_SWEEP_RETRY_MS * 2 ** exponent);
    const deadline = openFor(writes, name).length > 0 && due <= now ? now + retryMs : due;
    next = next === null ? deadline : Math.min(next, deadline);
  }
  return next;
}

async function scheduleProviderWritesSweep(transaction: Pick<DeletionTransaction, "get" | "put" | "delete" | "getAlarm" | "setAlarm">, writes: ProviderWrites, now: number): Promise<void> {
  const due = nextProviderWritesSweep(writes, now);
  if (due === null) {
    await transaction.delete(PROVIDER_WRITES_SWEEP_KEY);
    return;
  }
  const previous = await transaction.get<number>(PROVIDER_WRITES_SWEEP_KEY);
  // An overdue legacy row can be rediscovered after a failed sweep. Keep the
  // bounded retry rather than collapsing it back to an already-due deadline.
  const deadline = typeof previous === "number" && previous > now
    ? due <= now ? previous : Math.min(previous, due)
    : due;
  await transaction.put(PROVIDER_WRITES_SWEEP_KEY, deadline);
  const alarm = await transaction.getAlarm();
  if (alarm === null || alarm > deadline) await transaction.setAlarm(deadline);
}

async function scheduleProviderWritesRetry(transaction: Pick<DeletionTransaction, "get" | "put" | "getAlarm" | "setAlarm">, now: number): Promise<number> {
  const previousAttempt = await transaction.get<number>(PROVIDER_WRITES_SWEEP_ATTEMPTS_KEY) ?? 0;
  const attempt = Math.min(9, Number.isSafeInteger(previousAttempt) && previousAttempt >= 0 ? previousAttempt : 0);
  const retryMs = Math.min(PROVIDER_WRITES_SWEEP_RETRY_MAX_MS, PROVIDER_WRITES_SWEEP_RETRY_MS * 2 ** attempt);
  const deadline = now + retryMs;
  await transaction.put(PROVIDER_WRITES_SWEEP_ATTEMPTS_KEY, Math.min(9, attempt + 1));
  await transaction.put(PROVIDER_WRITES_SWEEP_KEY, deadline);
  const alarm = await transaction.getAlarm();
  // setAlarm replaces the shared alarm. Never move another subsystem's
  // earlier deadline; the production handler's final refresh merges both.
  if (alarm === null || alarm > deadline) await transaction.setAlarm(deadline);
  return deadline;
}

/** Enroll journals written before the sweep marker existed. Called on object
 * startup and before presence refresh, so an old idle tombstone acquires a
 * durable alarm without waiting for another provider write. */
export async function scheduleProviderWritesRetention(transaction: Pick<DeletionTransaction, "get" | "put" | "delete" | "getAlarm" | "setAlarm">, now: number): Promise<void> {
  await scheduleProviderWritesSweep(transaction, await readProviderWrites(transaction), now);
}

export async function enrollProviderWritesRetention(storage: DeletionStorage, now: number): Promise<void> {
  await storage.transaction((transaction) => scheduleProviderWritesRetention(transaction, now));
}

/** Prune journal keys only after their provider KV values cannot exist. Open
 * writes keep their entries and receive a bounded retry through the DO alarm. */
export async function sweepProviderWrites(storage: DeletionStorage, now: number): Promise<{ pruned: number; retained: number }> {
  try {
    return await storage.transaction(async (transaction) => {
      const writes = await readProviderWrites(transaction);
      await transaction.delete(PROVIDER_WRITES_SWEEP_ATTEMPTS_KEY);
      const keys = Object.fromEntries(Object.entries(writes.keys).filter(([name, entry]) => !prunable(writes, name, entry, now)));
      const next: ProviderWrites = { open: writes.open, keys };
      const pruned = Object.keys(writes.keys).length - Object.keys(keys).length;
      if (Object.keys(keys).length === 0 && Object.keys(writes.open).length === 0) {
        await transaction.delete(PROVIDER_WRITES_KEY);
        await transaction.delete(PROVIDER_WRITES_SWEEP_KEY);
      } else {
        await transaction.put(PROVIDER_WRITES_KEY, next);
        await scheduleProviderWritesSweep(transaction, next, now);
      }
      return { pruned, retained: Object.keys(keys).length };
    });
  } catch (error) {
    await storage.transaction(async (transaction) => {
      await scheduleProviderWritesRetry(transaction, now);
    });
    throw error;
  }
}

/** J1: admit a provider put of `key`, journalling it BEFORE the write can
 * reach KV. */
export async function admitProviderWrite(storage: DeletionStorage, key: string, generation: number | undefined, expiresAt: number | undefined, now: number, admissionId = newOperationId()): Promise<{ admitted: true; admissionId: string } | { admitted: false }> {
  return storage.transaction(async (transaction) => {
    if ((await readAccountDeletion(transaction))?.status === "pending") return { admitted: false } as const;
    const current = await readAccountGeneration(transaction);
    const writes = await readProviderWrites(transaction);
    // A key the provider has deleted is a tombstone: no put of it is admitted
    // again (a refresh that read a grant before its revocation cannot bring it
    // back).
    if (writes.keys[key]?.deleteIntent) return { admitted: false } as const;
    if (key.startsWith("grant:")) {
      if (grantGeneration(generation) !== current) return { admitted: false } as const;
    } else {
      const grantKey = grantKeyOfToken(key);
      const grant = grantKey ? writes.keys[grantKey] : undefined;
      if (!grant || grant.deleteIntent || grant.generation !== current) return { admitted: false } as const;
    }
    const keys = Object.fromEntries(Object.entries(writes.keys).filter(([name, entry]) => !prunable(writes, name, entry, now)));
    keys[key] = {
      ...(expiresAt !== undefined ? { expiresAt } : {}),
      ...(key.startsWith("grant:") ? { generation: grantGeneration(generation) } : {}),
    };
    const next: ProviderWrites = { open: { ...writes.open, [admissionId]: { key, admittedAt: now, state: "writing" } }, keys };
    await transaction.put(PROVIDER_WRITES_KEY, next satisfies ProviderWrites);
    await scheduleProviderWritesSweep(transaction, next, now);
    return { admitted: true, admissionId } as const;
  });
}

/** J3, after the put RETURNED: "keep" closes the admission; "compensate"
 * moves it to compensating, and the writer owes an acknowledged delete. */
export async function settleProviderWrite(storage: DeletionStorage, admissionId: string): Promise<"keep" | "compensate"> {
  return storage.transaction(async (transaction) => {
    const writes = await readProviderWrites(transaction);
    const entry = writes.open[admissionId];
    if (!entry) return "compensate";
    const pending = (await readAccountDeletion(transaction))?.status === "pending";
    if (!pending && mayStay(writes, entry.key, await readAccountGeneration(transaction))) {
      const { [admissionId]: _closed, ...open } = writes.open;
      const next: ProviderWrites = { ...writes, open };
      await transaction.put(PROVIDER_WRITES_KEY, next satisfies ProviderWrites);
      await scheduleProviderWritesSweep(transaction, next, Date.now());
      return "keep";
    }
    await transaction.put(PROVIDER_WRITES_KEY, { ...writes, open: { ...writes.open, [admissionId]: { ...entry, state: "compensating" } } } satisfies ProviderWrites);
    return "compensate";
  });
}

/** Close a compensating admission after an ACKNOWLEDGED delete of its key
 * issued after its put returned. A writing admission is never closed here. */
export async function closeCompensation(storage: DeletionStorage, admissionId: string, now = Date.now()): Promise<void> {
  await storage.transaction(async (transaction) => {
    const writes = await readProviderWrites(transaction);
    const entry = writes.open[admissionId];
    if (!entry || entry.state !== "compensating") return;
    const { [admissionId]: _closed, ...open } = writes.open;
    // The acknowledged compensating delete is a tombstone even when the
    // original admission had no expiry or prior delete record.
    const keys = { ...writes.keys, [entry.key]: { ...(writes.keys[entry.key] ?? {}), deleteIntent: true, deletedAt: now } };
    const next: ProviderWrites = { ...writes, open, keys };
    await transaction.put(PROVIDER_WRITES_KEY, next satisfies ProviderWrites);
    await scheduleProviderWritesSweep(transaction, next, now);
  });
}

/** Legacy reconciliation's claim on a key it found by listing, made in one
 * transaction BEFORE it revokes or deletes that key. Returns false (leave the
 * key alone) when the key is journalled by a provider write - it was admitted
 * (J1) under some generation, so it is current data or already handled by the
 * journal - unless the record is itself an earlier legacy claim with no open
 * writer. Otherwise it writes a legacy tombstone, so J1 refuses any put of
 * the key from then on.
 *
 * Why this is sound against a reopened account (plan 7.12): every key the
 * current code writes passes J1, which journals it in this object before its
 * put. J1 and this claim are serialized by the object's transactions. If J1
 * ran first, the claim sees the key and leaves it; if the claim ran first, J1
 * sees the tombstone and refuses. No snapshot taken before a list call is
 * relied on. */
export async function claimLegacyKey(storage: DeletionStorage, key: string, now: number): Promise<boolean> {
  return storage.transaction(async (transaction) => {
    const writes = await readProviderWrites(transaction);
    const entry = writes.keys[key];
    if (openFor(writes, key).length > 0) return false;
    if (entry && !entry.legacy) return false;
    const next: ProviderWrites = { ...writes, keys: { ...writes.keys, [key]: { ...(entry ?? {}), deleteIntent: true, deletedAt: now, legacy: true } } };
    await transaction.put(PROVIDER_WRITES_KEY, next satisfies ProviderWrites);
    await scheduleProviderWritesSweep(transaction, next, now);
    return true;
  });
}

/** Before the provider deletes `key` (revocation, cleanup): mark the
 * tombstone in one transaction, BEFORE the KV delete is issued. From then on
 * no put of the key is admitted, and an already-admitted writer of it
 * compensates at settle. The record is kept, not dropped, so the key stays
 * accounted for until it can no longer exist. */
export async function markProviderDelete(storage: DeletionStorage, key: string, now: number): Promise<void> {
  await storage.transaction(async (transaction) => {
    const writes = await readProviderWrites(transaction);
    const next: ProviderWrites = { ...writes, keys: { ...writes.keys, [key]: { ...(writes.keys[key] ?? {}), deleteIntent: true, deletedAt: now } } };
    await transaction.put(PROVIDER_WRITES_KEY, next satisfies ProviderWrites);
    await scheduleProviderWritesSweep(transaction, next, now);
  });
}

/** The account a provider KV key belongs to, or null for keys that are not
 * an `access-<sub>` grant or token (clients, JTIs). */
export function providerKeyOwner(key: string): { subject: string; kind: "grant" | "token" } | null {
  const match = /^(grant|token):access-([^:]+):/.exec(key);
  if (!match) return null;
  try { return { subject: decodeURIComponent(match[2]), kind: match[1] as "grant" | "token" }; } catch { return null; }
}

export type ProviderWriteFence = {
  admitProviderWrite(key: string, generation: number | undefined, expiresAt: number | undefined): Promise<{ admitted: true; admissionId: string } | { admitted: false }>;
  settleProviderWrite(admissionId: string): Promise<"keep" | "compensate">;
  closeCompensation(admissionId: string, now: number): Promise<void>;
  markProviderDelete(key: string): Promise<void>;
};
type KvPut = (key: string, value: unknown, options?: unknown) => Promise<unknown>;

/** The KV expiry a put sets, in milliseconds. */
function putExpiry(options: unknown, now: number): number | undefined {
  const value = options as { expiration?: unknown; expirationTtl?: unknown } | undefined;
  if (typeof value?.expiration === "number") return value.expiration * 1000;
  if (typeof value?.expirationTtl === "number") return now + value.expirationTtl * 1000;
  return undefined;
}

/** Wrap the KV namespace handed to the OAuth provider so every put of an
 * `access-<sub>` grant or token key is journalled in that account's Durable
 * Object before it can reach KV (J1), settled only after it returned (J3),
 * and compensated by an acknowledged delete when it must not stay. Provider
 * 0.10.4 performs every grant/token mutation through `env.OAUTH_KV`. Every
 * other key passes through unchanged. */
export function fenceProviderKv<T extends { put: KvPut; delete(key: string): Promise<unknown>; get?(key: string): Promise<string | null> }>(kv: T, fenceFor: (subject: string) => ProviderWriteFence, now: () => number = () => Date.now()): T {
  const put: KvPut = async (key, value, options) => {
    const owner = providerKeyOwner(key);
    if (!owner) return kv.put(key, value, options);
    let generation: number | undefined;
    let refreshClientId: string | undefined;
    if (owner.kind === "grant") {
      try {
        const grant = JSON.parse(String(value)) as { metadata?: { accountGeneration?: unknown }; refreshTokenId?: unknown; clientId?: unknown; expiresAt?: number };
        generation = grantGeneration(grant?.metadata?.accountGeneration);
        // The pinned provider writes refreshTokenId only after a successful
        // code/refresh exchange. Upgrade still-valid legacy grants here,
        // after validation, through the same revocation/generation fence.
        if (typeof grant.refreshTokenId === "string") {
          if (typeof grant.clientId === "string") refreshClientId = grant.clientId;
          delete grant.expiresAt;
          value = JSON.stringify(grant);
          options = undefined;
        }
      } catch { generation = 0; }
    }
    const fence = fenceFor(owner.subject);
    const admission = await fence.admitProviderWrite(key, generation, putExpiry(options, now()));
    if (!admission.admitted) throw new Error(PROVIDER_WRITE_REFUSED_MESSAGE);
    // J2. If this throws or times out, the outcome is unknown: the admission
    // stays open as "writing" and nothing here closes it.
    const result = await kv.put(key, value, options);
    // J3. If settle throws, the admission also stays open as "writing".
    if (await fence.settleProviderWrite(admission.admissionId) === "keep") {
      // Existing dynamic registrations also carried a 90-day KV TTL. Only
      // migrate a client used by a validated, admitted refresh grant. Client
      // deletion revokes its grants, whose tombstones remain authoritative.
      if (refreshClientId && kv.get) {
        const clientKey = `client:${refreshClientId}`;
        const client = await kv.get(clientKey);
        if (client !== null) await kv.put(clientKey, client);
      }
      return result;
    }
    // Owed: an acknowledged delete. A failure is not swallowed; the admission
    // stays "compensating" and the deletion retries it.
    await kv.delete(key);
    await fence.closeCompensation(admission.admissionId, now());
    throw new Error(PROVIDER_WRITE_REFUSED_MESSAGE);
  };
  const remove = async (key: string) => {
    const owner = providerKeyOwner(key);
    // The tombstone is journalled first; a failed KV delete leaves the key
    // tracked, and the account's deletion deletes it again.
    if (owner) await fenceFor(owner.subject).markProviderDelete(key);
    return kv.delete(key);
  };
  return new Proxy(kv, { get(target, property) {
    if (property === "put") return put;
    if (property === "delete") return remove;
    const value = Reflect.get(target, property, target);
    return typeof value === "function" ? value.bind(target) : value;
  } });
}

/** The environment the OAuth provider runs with: its KV fenced, and its
 * cached helpers cleared. Provider 0.10.4 caches helpers on the env object
 * (`if (!env.OAUTH_PROVIDER) env.OAUTH_PROVIDER = createOAuthHelpers(env)`),
 * so a copy that kept a helper bound to the raw KV would bypass the fence. */
export function fencedProviderEnv<E extends { OAUTH_KV: unknown; OAUTH_PROVIDER?: unknown }>(env: E, fenced: unknown): E {
  return { ...env, OAUTH_KV: fenced, OAUTH_PROVIDER: undefined } as E;
}

/** The alarm this account's deletion needs, if any. */
export function accountDeletionDeadline(state: AccountDeletionState | null | undefined): number | null {
  if (!state) return null;
  if (state.status === "pending" || state.status === "reconciling") return typeof state.retryAt === "number" ? state.retryAt : null;
  return state.retainUntil;
}

/** Two levels: `accessRevoked` is true from the first step (every paired
 * device and AI app is cut off); `status:"deleted"` means cleanup is
 * complete under the proof condition. `until` (legacy reconciliation) is the
 * earliest time it can complete, or null while any input is unknown (no drain
 * attestation, or a seen legacy grant of unknown expiry). */
export type PublicDeletionStatus =
  | { status: "none" }
  | { status: "pending"; operationId: string; startedAt: number; step: DeletionStep; accessRevoked: true; until?: number | null }
  | { status: "deleted"; operationId: string; startedAt: number; deletedAt: number };

export function legacyCompletionTime(state: ReconcilingAccountDeletion, attestation: LegacyDrainAttestation | null): number | null {
  if (attestation === null) return null;
  const seen = Object.values(state.seenLegacy ?? {});
  if (seen.some((expiresAt) => expiresAt === null)) return null;
  return Math.max(attestation.lastPossibleLegacyCompletionAt + attestation.maxLegacyLifetimeMs, state.lastLegacyDeleteAt + KV_VISIBILITY_MARGIN_MS, ...(seen as number[]));
}

export function publicDeletionStatus(state: AccountDeletionState | null, attestation: LegacyDrainAttestation | null = LEGACY_DRAIN_ATTESTATION): PublicDeletionStatus {
  if (!state) return { status: "none" };
  if (state.status === "pending") return { status: "pending", operationId: state.operationId, startedAt: state.startedAt, step: state.step ?? "provider-writes", accessRevoked: true };
  if (state.status === "reconciling") return { status: "pending", operationId: state.operationId, startedAt: state.startedAt, step: "legacy-reconciliation", accessRevoked: true, until: legacyCompletionTime(state, attestation) };
  return { status: "deleted", operationId: state.operationId, startedAt: state.startedAt, deletedAt: state.deletedAt };
}

/** Delete every key except the fence; return what the KV cleanup needs. */
async function purgeAccountKeys(transaction: DeletionTransaction): Promise<{ deviceIds: string[]; offerCodeHash?: string }> {
  const ids = new Set<string>();
  let offerCodeHash: string | undefined;
  for (const [key, value] of await transaction.list<unknown>({})) {
    if (FENCE_KEYS.has(key)) continue;
    if (key.startsWith(DEVICE_RECORD_PREFIX)) ids.add(key.slice(DEVICE_RECORD_PREFIX.length));
    else if (key === DIRECTORY_CLEANUP_KEY && Array.isArray(value)) {
      for (const deviceId of value) if (typeof deviceId === "string") ids.add(deviceId);
    } else if (key === PAIRING_OFFER_KEY) {
      const codeHash = (value as { codeHash?: unknown } | undefined)?.codeHash;
      if (typeof codeHash === "string") offerCodeHash = codeHash;
    }
    await transaction.delete(key);
  }
  return { deviceIds: [...ids], ...(offerCodeHash ? { offerCodeHash } : {}) };
}

/** Step 1 of the deletion. Idempotent: a pending deletion is returned
 * unchanged. The fence (generation, provider-write admissions) is kept. */
export async function beginAccountDeletion(storage: DeletionStorage, subject: string, now: number, operationId = newOperationId()): Promise<PendingAccountDeletion> {
  return storage.transaction(async (transaction) => {
    const existing = await readAccountDeletion(transaction);
    if (existing?.status === "pending") return existing;
    const purged = await purgeAccountKeys(transaction);
    // `retryAt` is in the marker, so every path that recomputes the one
    // alarm (presence, retention) keeps the deletion's deadline.
    const marker: PendingAccountDeletion = { version: 1, status: "pending", operationId, startedAt: now, subject, step: "provider-writes", retryAt: now + ACCOUNT_DELETION_RETRY_MS, ...purged };
    await transaction.put(ACCOUNT_DELETION_KEY, marker);
    await scheduleSharedAlarm(transaction, marker.retryAt!);
    return marker;
  });
}

/** The MCP OAuth callback's early check (before the provider is asked to
 * write anything): refused while a deletion is pending. */
export async function admitGrant(storage: DeletionStorage, _admissionId: string, _now: number): Promise<{ admitted: true; generation: number } | { admitted: false }> {
  return storage.transaction(async (transaction) => {
    if ((await readAccountDeletion(transaction))?.status === "pending") return { admitted: false } as const;
    return { admitted: true, generation: await readAccountGeneration(transaction) } as const;
  });
}

/** The callback's check after the provider wrote: false makes it revoke the
 * grant it just created. The binding fence already refused or took back any
 * write that raced a deletion; this is the early, user-visible answer. */
export async function settleGrant(storage: DeletionStorage, _admissionId: string, generation: number): Promise<boolean> {
  return storage.transaction(async (transaction) => {
    if ((await readAccountDeletion(transaction))?.status === "pending") return false;
    return grantGeneration(generation) === await readAccountGeneration(transaction);
  });
}

/** A pairing attempt refused by the fence had already written its KV
 * directory entry; queue it so it cannot outlive the account. */
export async function queueRefusedDirectoryEntry(storage: DeletionStorage, deviceId: string, now: number): Promise<void> {
  await storage.transaction(async (transaction) => {
    const state = await readAccountDeletion(transaction);
    if (state?.status === "pending") {
      if (!state.deviceIds.includes(deviceId)) await transaction.put(ACCOUNT_DELETION_KEY, { ...state, deviceIds: [...state.deviceIds, deviceId] } satisfies PendingAccountDeletion);
      return;
    }
    const queued = await transaction.get<string[]>(DIRECTORY_CLEANUP_KEY) ?? [];
    await transaction.put(DIRECTORY_CLEANUP_KEY, [...new Set([...queued, deviceId])]);
    const due = now + DIRECTORY_CLEANUP_RETRY_MS;
    const sweep = await transaction.get<number>(DEVICE_SWEEP_KEY);
    if (typeof sweep !== "number" || sweep > due) await transaction.put(DEVICE_SWEEP_KEY, due);
    const alarm = await transaction.getAlarm();
    if (alarm === null || alarm > due) await transaction.setAlarm(due);
  });
}

/** Revoke every grant of `subject` and confirm none is listed. Failures are
 * not swallowed. Page one is re-read after each pass. */
export async function revokeAllConnectorGrants(registry: ConnectorGrantRegistry, subject: string): Promise<{ revoked: number; confirmedEmpty: boolean }> {
  const userId = connectorGrantUser(subject);
  let revoked = 0;
  for (let pass = 0; pass < GRANT_REVOCATION_PASSES; pass += 1) {
    const page = await registry.listUserGrants(userId);
    if (page.items.length === 0) return { revoked, confirmedEmpty: true };
    for (const grant of page.items) {
      await registry.revokeGrant(grant.id, userId);
      revoked += 1;
    }
  }
  return { revoked, confirmedEmpty: (await registry.listUserGrants(userId)).items.length === 0 };
}

/** Delete every remaining `grant:`/`token:` key of the subject (orphaned
 * tokens included) and confirm both prefixes list empty. */
async function sweepProviderKeys(kv: ProviderKv, subject: string): Promise<boolean> {
  const userId = connectorGrantUser(subject);
  for (const prefix of [`token:${userId}:`, `grant:${userId}:`]) {
    let cursor: string | undefined;
    do {
      const page = await kv.list({ prefix, ...(cursor ? { cursor } : {}) });
      for (const { name } of page.keys) await kv.delete(name);
      cursor = page.list_complete ? undefined : page.cursor;
    } while (cursor);
  }
  for (const prefix of [`token:${userId}:`, `grant:${userId}:`]) if ((await kv.list({ prefix })).keys.length > 0) return false;
  return true;
}

export type AccountDeletionOutcome =
  | { status: "deleted"; operationId: string; deletedAt: number }
  | { status: "pending"; operationId: string; step: DeletionStep };

/** Legacy reconciliation: revoke and delete every grant/token key of the
 * subject that the current journal does not hold (the current generation's
 * own data is left alone), and complete only when a sweep at least
 * `KV_VISIBILITY_MARGIN_MS` after the last delete finds nothing and the
 * legacy-writes boundary plus the maximum lifetime has passed. */
export async function reconcileLegacy(storage: DeletionStorage, registry: ConnectorGrantRegistry, providerKv: ProviderKv, now: number, attestation: LegacyDrainAttestation | null, log: (line: string) => void = (line) => console.warn(line)): Promise<AccountDeletionOutcome | null> {
  const state = await storage.transaction((transaction) => readAccountDeletion(transaction));
  if (state?.status !== "reconciling") return null;
  const userId = connectorGrantUser(state.subject);
  let found = 0;
  let failed = false;
  const seenLegacy: Record<string, number | null> = { ...(state.seenLegacy ?? {}) };
  try {
    for (const grant of (await registry.listUserGrants(userId)).items) {
      // Claimed transactionally just before the revoke; a key the journal
      // holds (the reopened account's own data, admitted at any moment, even
      // during this pass) is never touched.
      if (!await claimLegacyKey(storage, `grant:${userId}:${grant.id}`, now)) continue;
      found += 1;
      // Recorded before the revoke, so a failed revoke still counts it.
      seenLegacy[grant.id] = typeof grant.expiresAt === "number" ? grant.expiresAt * 1_000 : null;
      await registry.revokeGrant(grant.id, userId);
    }
    for (const prefix of [`token:${userId}:`, `grant:${userId}:`]) {
      let cursor: string | undefined;
      do {
        const page = await providerKv.list({ prefix, ...(cursor ? { cursor } : {}) });
        for (const { name } of page.keys) {
          if (!await claimLegacyKey(storage, name, now)) continue;
          found += 1;
          // A grant key reached only by the prefix sweep has no readable
          // expiry here: unknown, which keeps the deletion pending.
          if (name.startsWith("grant:")) { const grantId = name.split(":")[2]; if (!(grantId in seenLegacy)) seenLegacy[grantId] = null; }
          await providerKv.delete(name);
        }
        cursor = page.list_complete ? undefined : page.cursor;
      } while (cursor);
    }
  } catch (cause) {
    failed = true;
    log(JSON.stringify({ event: "account-deletion-retry", step: "legacy-reconciliation", detail: cause instanceof Error ? cause.name : "unknown" }));
  }
  return storage.transaction(async (transaction): Promise<AccountDeletionOutcome> => {
    const current = await readAccountDeletion(transaction);
    if (current?.status !== "reconciling" || current.operationId !== state.operationId) {
      return current?.status === "deleted" ? { status: "deleted", operationId: current.operationId, deletedAt: current.deletedAt } : { status: "pending", operationId: state.operationId, step: "legacy-reconciliation" };
    }
    const lastLegacyDeleteAt = found > 0 || failed ? now : current.lastLegacyDeleteAt;
    const done = legacyCompletionTime({ ...current, lastLegacyDeleteAt, seenLegacy }, attestation);
    if (!failed && found === 0 && done !== null && now >= done) {
      const receipt: AccountDeletionReceipt = { version: 1, status: "deleted", operationId: current.operationId, startedAt: current.startedAt, deletedAt: now, retainUntil: now + ACCOUNT_DELETION_RECEIPT_MS };
      await transaction.put(ACCOUNT_DELETION_KEY, receipt);
      await scheduleSharedAlarm(transaction, receipt.retainUntil);
      return { status: "deleted", operationId: receipt.operationId, deletedAt: now };
    }
    const retryAt = Math.max(now + 1_000, Math.min(now + LEGACY_RECONCILE_INTERVAL_MS, done ?? Number.POSITIVE_INFINITY));
    await transaction.put(ACCOUNT_DELETION_KEY, { ...current, lastLegacyDeleteAt, seenLegacy, retryAt } satisfies ReconcilingAccountDeletion);
    await scheduleSharedAlarm(transaction, retryAt);
    return { status: "pending", operationId: current.operationId, step: "legacy-reconciliation" };
  });
}

/** Steps 2-4 for the pending operation. Safe to call repeatedly and from the
 * alarm; each step re-reads the marker. */
export async function runAccountDeletion(storage: DeletionStorage, kv: DeletionKv, registry: ConnectorGrantRegistry, providerKv: ProviderKv, now: number, log: (line: string) => void = (line) => console.warn(line), attestation: LegacyDrainAttestation | null = LEGACY_DRAIN_ATTESTATION): Promise<AccountDeletionOutcome | null> {
  const state = await storage.transaction((transaction) => readAccountDeletion(transaction));
  if (!state) return null;
  if (state.status === "deleted") return { status: "deleted", operationId: state.operationId, deletedAt: state.deletedAt };
  if (state.status === "reconciling") return reconcileLegacy(storage, registry, providerKv, now, attestation, log);
  const retry = async (step: DeletionStep, requestedAt = now + ACCOUNT_DELETION_RETRY_MS): Promise<AccountDeletionOutcome> => {
    await storage.transaction(async (transaction) => {
      const current = await readAccountDeletion(transaction);
      if (current?.status !== "pending" || current.operationId !== state.operationId) return;
      let at = requestedAt;
      let next = { ...current, retryAt: at, step } satisfies PendingAccountDeletion;
      if (step === "provider-writes") {
        if (current.step === "provider-writes" && current.retryAt !== undefined && current.retryAt > now) {
          at = current.retryAt;
          next = { ...current, retryAt: at, step };
        } else {
          const count = current.providerWritesRetryCount ?? 0;
          const nextCount = current.retryAt === undefined
            ? count
            : Math.min(PROVIDER_WRITES_DELETION_RETRY_MAX_COUNT, count + 1);
          at = now + Math.min(ACCOUNT_DELETION_RETRY_MS * 2 ** nextCount, PROVIDER_WRITES_DELETION_RETRY_MAX_MS);
          next = { ...current, providerWritesRetryCount: nextCount, retryAt: at, step };
        }
      }
      await transaction.put(ACCOUNT_DELETION_KEY, next);
      await scheduleSharedAlarm(transaction, at);
    });
    return { status: "pending", operationId: state.operationId, step };
  };

  if (state.grantsRevokedAt === undefined) {
    // The barrier (plan 7.10). A "writing" admission (outcome unknown) keeps
    // the deletion pending with no timeout; its key gets hygiene deletes but
    // the admission is never closed here. A "compensating" admission (its put
    // returned) is closed only after this deletion's own acknowledged delete.
    const writes = await storage.transaction((transaction) => readProviderWrites(transaction));
    for (const [admissionId, entry] of Object.entries(writes.open)) {
      try {
        await providerKv.delete(entry.key);
        if (entry.state === "compensating") await closeCompensation(storage, admissionId);
      } catch { /* retried on the next pass */ }
    }
    const barrier = await storage.transaction((transaction) => readProviderWrites(transaction));
    if (Object.keys(barrier.open).length > 0) return retry("provider-writes");
    // No open admission, and none can be added while pending: every put and
    // compensation for every key has settled. Only now the exact deletes.
    try {
      for (const key of Object.keys(barrier.keys)) await providerKv.delete(key);
      const { confirmedEmpty } = await revokeAllConnectorGrants(registry, state.subject);
      if (!confirmedEmpty || !await sweepProviderKeys(providerKv, state.subject)) return retry("grants");
    } catch (cause) {
      log(JSON.stringify({ event: "account-deletion-retry", step: "grants", detail: cause instanceof Error ? cause.name : "unknown" }));
      return retry("grants");
    }
    const recorded = await storage.transaction(async (transaction) => {
      const current = await readAccountDeletion(transaction);
      if (current?.status !== "pending" || current.operationId !== state.operationId) return false;
      // The journal must still have no open admission (none can be added
      // while pending); the deleted keys are dropped from it.
      const latestWrites = await readProviderWrites(transaction);
      if (Object.keys(latestWrites.open).length > 0) return false;
      await transaction.put(PROVIDER_WRITES_KEY, { open: {}, keys: {} });
      const next: PendingAccountDeletion = { ...current, grantsRevokedAt: now, step: "directory" };
      delete next.providerWritesRetryCount;
      await transaction.put(ACCOUNT_DELETION_KEY, next);
      return true;
    });
    if (!recorded) return retry("grants");
  }

  const current = await storage.transaction((transaction) => readAccountDeletion(transaction));
  if (current?.status !== "pending" || current.operationId !== state.operationId) return retry("grants");
  const deleted: string[] = [];
  let offerDeleted = current.offerCodeHash === undefined;
  if (current.offerCodeHash !== undefined) {
    try { await kv.delete(pairingDirectoryKey(current.offerCodeHash)); offerDeleted = true; } catch { /* retried */ }
  }
  for (const deviceId of current.deviceIds) {
    try { await kv.delete(deviceDirectoryKey(deviceId)); deleted.push(deviceId); } catch { /* retried */ }
  }

  return storage.transaction(async (transaction): Promise<AccountDeletionOutcome> => {
    const latest = await readAccountDeletion(transaction);
    if (latest?.status === "deleted") return { status: "deleted", operationId: latest.operationId, deletedAt: latest.deletedAt };
    if (latest?.status !== "pending" || latest.operationId !== state.operationId) return { status: "pending", operationId: state.operationId, step: latest?.status === "reconciling" ? "legacy-reconciliation" : "grants" };
    const remaining = latest.deviceIds.filter((id) => !deleted.includes(id));
    const offerRemaining = offerDeleted ? undefined : latest.offerCodeHash;
    const leftovers = remaining.length === 0 && offerRemaining === undefined ? await purgeAccountKeys(transaction) : { deviceIds: [] as string[] };
    if (remaining.length > 0 || offerRemaining !== undefined || leftovers.deviceIds.length > 0 || leftovers.offerCodeHash) {
      const retryAt = now + ACCOUNT_DELETION_RETRY_MS;
      const next: PendingAccountDeletion = { ...latest, deviceIds: [...remaining, ...leftovers.deviceIds], retryAt, step: "directory" };
      const hash = offerRemaining ?? leftovers.offerCodeHash;
      if (hash === undefined) delete next.offerCodeHash; else next.offerCodeHash = hash;
      await transaction.put(ACCOUNT_DELETION_KEY, next);
      await scheduleSharedAlarm(transaction, retryAt);
      return { status: "pending", operationId: latest.operationId, step: "directory" };
    }
    // Journalled cleanup is complete: open the fence under a new generation
    // (the account can be used again) and reconcile what the journal cannot
    // account for.
    await transaction.put(ACCOUNT_GENERATION_KEY, await readAccountGeneration(transaction) + 1);
    const reconciling: ReconcilingAccountDeletion = { version: 1, status: "reconciling", operationId: latest.operationId, startedAt: latest.startedAt, accessRevokedAt: latest.startedAt, subject: latest.subject, lastLegacyDeleteAt: now, retryAt: now + KV_VISIBILITY_MARGIN_MS };
    await transaction.put(ACCOUNT_DELETION_KEY, reconciling);
    await scheduleSharedAlarm(transaction, reconciling.retryAt);
    return { status: "pending", operationId: latest.operationId, step: "legacy-reconciliation" };
  });
}

/** Begin (or rejoin) and run. */
export async function requestAccountDeletion(storage: DeletionStorage, kv: DeletionKv, registry: ConnectorGrantRegistry, providerKv: ProviderKv, subject: string, now: number, log?: (line: string) => void, attestation: LegacyDrainAttestation | null = LEGACY_DRAIN_ATTESTATION): Promise<AccountDeletionOutcome> {
  await beginAccountDeletion(storage, subject, now);
  return (await runAccountDeletion(storage, kv, registry, providerKv, now, log, attestation))!;
}

/** The alarm's share. Returns true when the account is mid-deletion and the
 * alarm must do nothing else. The generation is never touched here. */
export async function runAccountDeletionAlarm(storage: DeletionStorage, kv: DeletionKv, registry: ConnectorGrantRegistry, providerKv: ProviderKv, now: number, log?: (line: string) => void, attestation: LegacyDrainAttestation | null = LEGACY_DRAIN_ATTESTATION): Promise<boolean> {
  const state = await storage.transaction((transaction) => readAccountDeletion(transaction));
  if (!state) return false;
  if (state.status === "pending") {
    // Let other due shared work run while this deletion waits for its retry.
    // Returning true here would suppress presence and retention processing.
    if (state.retryAt !== undefined && now < state.retryAt) return false;
    await runAccountDeletion(storage, kv, registry, providerKv, now, log, attestation);
    return true;
  }
  if (state.status === "reconciling") {
    if (now >= state.retryAt) await reconcileLegacy(storage, registry, providerKv, now, attestation, log);
    return false;
  }
  if (now >= state.retainUntil) {
    await storage.transaction(async (transaction) => {
      const current = await readAccountDeletion(transaction);
      if (current?.status === "deleted" && current.operationId === state.operationId) await transaction.delete(ACCOUNT_DELETION_KEY);
    });
  }
  return false;
}

/** The grant id inside an OAuth redirect the provider just produced. The
 * provider issues codes and implicit tokens as `<userId>:<grantId>:<secret>`. */
export function grantIdFromRedirect(redirectTo: string, userId: string): string | null {
  let url: URL;
  try { url = new URL(redirectTo); } catch { return null; }
  const candidates = [url.searchParams.get("code"), new URLSearchParams(url.hash.slice(1)).get("access_token")];
  for (const value of candidates) {
    const parts = value?.split(":");
    if (parts?.length === 3 && parts[0] === userId && /^[A-Za-z0-9_-]{8,128}$/.test(parts[1])) return parts[1];
  }
  return null;
}
