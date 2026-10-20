/** Host pairings persist until revoked. Legacy expiring records are retained
 * for their former 90-day lifetime plus grace, and migrate on authenticated
 * use before expiry. Persistent records are never aged out. Provisional KV
 * pointers retain a bounded TTL until the first authenticated connection;
 * that connection removes the TTL without resurrecting revoked records. */
import relayProfile from "../../../contracts/public/qc-relay-profile.v1.json" with { type: "json" };

const DAY_MS = 24 * 60 * 60_000;

/** Historical host lifetime, used only to clean up unmigrated legacy records. */
export const DEVICE_RETENTION_AFTER_LAST_USE_MS = 90 * DAY_MS;
export const DEVICE_RETENTION_GRACE_MS = 30 * DAY_MS;
export const DEVICE_RETENTION_MS = DEVICE_RETENTION_AFTER_LAST_USE_MS + DEVICE_RETENTION_GRACE_MS;
/** KV TTL of a device directory entry, in seconds. */
export const DEVICE_DIRECTORY_TTL_SECONDS = DEVICE_RETENTION_MS / 1000;
/** `lastUsedAt` is updated at most this often per persistent device. */
export const LAST_USED_WRITE_INTERVAL_MS = DAY_MS;
/** A failed KV cleanup is retried on this delay. */
export const DIRECTORY_CLEANUP_RETRY_MS = 60 * 60_000;

/** Earliest scheduled retention work for this account, kept beside the device
 * records so the one Durable Object alarm can include it. */
export const DEVICE_SWEEP_KEY = "device-retention-sweep-at-v1";
/** Device ids whose KV directory entry still has to be deleted. */
export const DIRECTORY_CLEANUP_KEY = "device-directory-cleanup-v1";
export const DEVICE_RECORD_PREFIX = "device:";

/** The predecessor a re-paired record will replace on its first connect. */
export type PendingReplacement = { deviceId: string; credentialHash: string };
/** `replacedDeviceId` is kept after adoption so every later connect can
 * acknowledge that the predecessor's credential is no longer accepted, even
 * when the first acknowledgement was lost. An id only, never a credential. */
export type RetainedDevice = { credentialHash: string; pairedAt: number; expiresAt?: number | null; lastUsedAt?: number; replaces?: PendingReplacement; replacedDeviceId?: string; pairingOperationId?: string };
/** An explicit refusal by the account's own state (the deletion fence). */
export type AccountWriteDenial = { denied: true; message: string };
/** Called inside a redemption or adoption transaction before it commits. It
 * RETURNS a denial when the account's state says no, and returns null to
 * allow. An exception means the state could not be read: it propagates, and a
 * connect reports it as retryable (503), never as a rejection (401). The
 * account-deletion fence plugs in here. */
export type AccountWriteGuard = (transaction: RetentionTransaction) => Promise<AccountWriteDenial | null>;

export type RetentionTransaction = {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<boolean>;
  list<T>(options: { prefix?: string; limit?: number }): Promise<Map<string, T>>;
  getAlarm(): Promise<number | null>;
  setAlarm(scheduledTime: number): Promise<void>;
};
export type RetentionStorage = {
  transaction<T>(callback: (transaction: RetentionTransaction) => Promise<T>): Promise<T>;
};

/** The KV namespace operations this module needs. */
export type DirectoryKv = {
  get<T>(key: string, type: "json"): Promise<T | null>;
  getWithMetadata<T, M>(key: string, type: "json"): Promise<{ value: T | null; metadata: M | null }>;
  put(key: string, value: string, options?: { expirationTtl?: number; metadata?: unknown }): Promise<void>;
  delete(key: string): Promise<void>;
};

export function deviceDirectoryKey(deviceId: string): string { return `qcrc:device:${deviceId}`; }

/** Last use: the later of `lastUsedAt` and `pairedAt`; legacy records count from pairing. */
export function lastUseOf(record: RetainedDevice): number {
  return typeof record.lastUsedAt === "number" && Number.isSafeInteger(record.lastUsedAt) && record.lastUsedAt > record.pairedAt
    ? record.lastUsedAt
    : record.pairedAt;
}

export function deletionDeadline(record: RetainedDevice): number {
  if (record.expiresAt === null) return Infinity;
  return lastUseOf(record) + DEVICE_RETENTION_MS;
}

/** The earliest of several optional deadlines: the single alarm a Durable
 * Object can hold has to serve all of them. */
export function earliestDeadline(...deadlines: Array<number | null | undefined>): number | null {
  let next: number | null = null;
  for (const deadline of deadlines) {
    if (typeof deadline !== "number" || !Number.isFinite(deadline)) continue;
    next = next === null ? deadline : Math.min(next, deadline);
  }
  return next;
}

/** Record that retention work is due at `deadline`, keeping the earliest, and
 * make sure the alarm fires no later than that. */
export async function scheduleRetentionWork(transaction: RetentionTransaction, deadline: number): Promise<void> {
  if (!Number.isFinite(deadline)) return;
  const current = await transaction.get<number>(DEVICE_SWEEP_KEY);
  const next = typeof current === "number" ? Math.min(current, deadline) : deadline;
  if (next !== current) await transaction.put(DEVICE_SWEEP_KEY, next);
  const alarm = await transaction.getAlarm();
  if (alarm === null || alarm > next) await transaction.setAlarm(next);
}

/** Write `lastUsedAt` for an authenticated device, at most once per interval.
 *
 * Re-read inside the transaction and compared by credential hash, so a
 * revocation or re-pair that won the race is never undone by a late write. */
export async function touchDeviceLastUse(storage: RetentionStorage, deviceId: string, credentialHash: string, now: number): Promise<boolean> {
  return storage.transaction(async (transaction) => {
    const key = `${DEVICE_RECORD_PREFIX}${deviceId}`;
    const record = await transaction.get<RetainedDevice>(key);
    if (!record || record.credentialHash !== credentialHash) return false;
    if (record.expiresAt === null && typeof record.lastUsedAt === "number" && now - record.lastUsedAt < LAST_USED_WRITE_INTERVAL_MS) return false;
    await transaction.put(key, { ...record, expiresAt: null, lastUsedAt: now });
    return true;
  });
}

export type SweepResult = { deleted: string[]; nextSweepAt: number | null };

/** Delete every record past its deadline and queue its KV entry for deletion,
 * in one transaction; then reschedule for the earliest remaining deadline. */
export async function sweepExpiredDevices(storage: RetentionStorage, now: number): Promise<SweepResult> {
  return storage.transaction(async (transaction) => {
    const records = await transaction.list<RetainedDevice>({ prefix: DEVICE_RECORD_PREFIX });
    const deleted: string[] = [];
    let next: number | null = null;
    for (const [key, record] of records) {
      const deadline = validRecord(record) ? deletionDeadline(record) : now;
      if (deadline <= now) {
        await transaction.delete(key);
        deleted.push(key.slice(DEVICE_RECORD_PREFIX.length));
      } else {
        next = earliestDeadline(next, deadline);
      }
    }
    const queued = await transaction.get<string[]>(DIRECTORY_CLEANUP_KEY) ?? [];
    const cleanup = [...new Set([...queued, ...deleted])];
    if (cleanup.length > 0) await transaction.put(DIRECTORY_CLEANUP_KEY, cleanup);
    for (const [key, reservation] of await transaction.list<PairingReservation>({ prefix: PAIRING_RESERVATION_PREFIX })) {
      const deviceId = key.slice(PAIRING_RESERVATION_PREFIX.length);
      const due = compactionDue(reservation, records.has(`${DEVICE_RECORD_PREFIX}${deviceId}`) && !deleted.includes(deviceId), cleanup.includes(deviceId), now);
      if (due.drop) await transaction.delete(key);
      else next = earliestDeadline(next, due.at);
    }
    if (cleanup.length > 0) next = earliestDeadline(next, now + DIRECTORY_CLEANUP_RETRY_MS);
    if (next === null) await transaction.delete(DEVICE_SWEEP_KEY);
    else await transaction.put(DEVICE_SWEEP_KEY, next);
    return { deleted, nextSweepAt: next };
  });
}

/** Delete the queued KV directory entries, then drop from the queue exactly the
 * ones that succeeded. A failed delete stays queued for the next alarm.
 *
 * Only ids this account's own records or pair attempts used ever enter the
 * queue. Device ids are server-minted random UUIDs, unique across accounts
 * only with overwhelming probability, and this account refuses an id while
 * any reservation or record holds it (`admitPairingAttempt`), so a retry can
 * only delete what was already revoked or never became live. Each id is still
 * re-checked inside the Durable Object first, because KV has
 * no conditional delete: an id whose record exists is live, so its job is
 * dropped; an id held by an admitted pair attempt (whose put may still land
 * and become record-backed) is skipped and stays queued; an id whose
 * reservation is abandoned (an unknown put an owner released) is dropped
 * without a delete, because no delete is known to come after that put. An
 * acknowledged delete of an aborted attempt's entry is recorded on its
 * reservation. */
export async function drainDirectoryCleanup(storage: RetentionStorage, kv: Pick<DirectoryKv, "delete">): Promise<{ deleted: string[]; failed: string[] }> {
  const due = await storage.transaction(async (transaction) => {
    const queued = await transaction.get<string[]>(DIRECTORY_CLEANUP_KEY) ?? [];
    const live: string[] = [];
    const ready: string[] = [];
    for (const deviceId of queued) {
      const state = (await transaction.get<PairingReservation>(`${PAIRING_RESERVATION_PREFIX}${deviceId}`))?.state;
      if (await transaction.get(`${DEVICE_RECORD_PREFIX}${deviceId}`) !== undefined || state === "abandoned") live.push(deviceId);
      else if (state !== "admitted") ready.push(deviceId);
    }
    if (live.length > 0) {
      const remaining = queued.filter((deviceId) => !live.includes(deviceId));
      if (remaining.length > 0) await transaction.put(DIRECTORY_CLEANUP_KEY, remaining);
      else await transaction.delete(DIRECTORY_CLEANUP_KEY);
    }
    return ready;
  });
  if (due.length === 0) return { deleted: [], failed: [] };
  const deleted: string[] = [];
  const failed: string[] = [];
  for (const deviceId of due) {
    try { await kv.delete(deviceDirectoryKey(deviceId)); deleted.push(deviceId); }
    catch { failed.push(deviceId); }
  }
  await storage.transaction(async (transaction) => {
    const current = await transaction.get<string[]>(DIRECTORY_CLEANUP_KEY) ?? [];
    const remaining = current.filter((deviceId) => !deleted.includes(deviceId));
    if (remaining.length > 0) await transaction.put(DIRECTORY_CLEANUP_KEY, remaining);
    else await transaction.delete(DIRECTORY_CLEANUP_KEY);
    for (const deviceId of deleted) {
      const key = `${PAIRING_RESERVATION_PREFIX}${deviceId}`;
      const reservation = await transaction.get<PairingReservation>(key);
      if (reservation?.state !== "aborted") continue;
      await transaction.put(key, { ...reservation, state: "cleanup-acked" } satisfies PairingReservation);
      // The sweep that ran before this drain saw it aborted and scheduled no
      // compaction for it, so wake the sweep when it becomes due.
      await scheduleRetentionWork(transaction, (reservation.offerExpiresAt ?? reservation.admittedAt) + PAIRING_COMPACTION_MARGIN_MS);
    }
  });
  return { deleted, failed };
}

/** Devices whose credential is still accepted. An expired record is kept for
 * the grace period so a roster can say why access stopped, but it is not a
 * device any AI app can reach. */
export function liveDeviceCount(records: Iterable<RetainedDevice>, credentialTtlMs: number, now: number): number {
  let live = 0;
  for (const record of records) if (record.expiresAt === null || (record.expiresAt ?? record.pairedAt + credentialTtlMs) > now) live += 1;
  return live;
}

export type PreviousDevice = { deviceId: string; credentialHash: string };
/** `admissionClosed`: set when a delayed admission had to be fenced and the
 * account had no room for a per-id fence; this offer admits nothing more.
 * Redemption ignores it (it needs an admitted reservation anyway). */
type PairingOffer = { codeHash: string; expiresAt: number; redeemed: boolean; admissionClosed?: boolean };

/** Redeem this account's pairing offer and create `record` as `deviceId`.
 * When the host proves it held `previous` on this same account, the new
 * record is marked as its pending replacement; nothing is revoked here.
 *
 * The proof is exact: the previous record must exist in this account's own
 * Durable Object, under exactly that device id, with exactly that credential
 * hash. Another account's device lives in another object and cannot be named
 * here at all, and a wrong or stale credential marks nothing - the pairing
 * itself still succeeds. Possession of the credential is the proof; it is not
 * required to be unexpired, because re-pairing is what a host does after its
 * credential lapses. */
export async function redeemPairingOffer(
  storage: RetentionStorage,
  input: { codeHash: string; deviceId: string; operationId?: string; record: RetainedDevice & Record<string, unknown>; previous?: PreviousDevice; now: number; guard?: AccountWriteGuard },
  equal: (left: string, right: string) => Promise<boolean>,
): Promise<{ previousReplacementPending: boolean }> {
  return storage.transaction(async (transaction) => {
    const denial = input.guard ? await input.guard(transaction) : null;
    if (denial) throw new Error(denial.message);
    const offer = await transaction.get<PairingOffer>("pairing-offer");
    if (!offer || offer.redeemed || offer.expiresAt < input.now || !await equal(input.codeHash, offer.codeHash)) throw new Error("Pairing offer is invalid or expired");
    const newKey = `${DEVICE_RECORD_PREFIX}${input.deviceId}`;
    if (await transaction.get(newKey) !== undefined) throw new Error("Device id collision");
    // A route attempt commits only while its device reservation is admitted
    // for exactly this attempt, so an abort that committed first
    // (`resolvePairingAttempt`) refuses a delayed call here, inside the
    // transaction that would create the record. A call without an attempt id
    // cannot claim a reserved device id either.
    const reservationKey = `${PAIRING_RESERVATION_PREFIX}${input.deviceId}`;
    const reservation = await transaction.get<PairingReservation>(reservationKey);
    if (input.operationId !== undefined
      ? !reservation || reservation.state !== "admitted" || reservation.operationId !== input.operationId
      : reservation !== undefined) throw new Error("Pairing attempt is not admitted");
    let replaces: PendingReplacement | undefined;
    if (input.previous && input.previous.deviceId !== input.deviceId) {
      const held = await transaction.get<RetainedDevice>(`${DEVICE_RECORD_PREFIX}${input.previous.deviceId}`);
      if (held && typeof held.credentialHash === "string" && await equal(input.previous.credentialHash, held.credentialHash)) {
        replaces = { deviceId: input.previous.deviceId, credentialHash: held.credentialHash };
      }
    }
    const { replaces: _ignored, pairingOperationId: _caller, ...record } = input.record;
    // The record names the attempt that created it, and the reservation
    // records the commit, in the same transaction.
    const committed = input.operationId !== undefined ? { ...record, pairingOperationId: input.operationId } : record;
    await transaction.put(newKey, replaces ? { ...committed, replaces } : committed);
    if (reservation) await transaction.put(reservationKey, { ...reservation, state: "committed", committedAt: input.now } satisfies PairingReservation);
    await transaction.put("pairing-offer", { ...offer, redeemed: true });
    await scheduleRetentionWork(transaction, deletionDeadline(input.record));
    return { previousReplacementPending: replaces !== undefined };
  });
}

/** The versioned two-phase re-pair route; the ordinary pair route
 * ignores the previous* fields. A Worker without this route answers 404 and
 * changes nothing, which is how a host learns the relay does not support it;
 * a host never falls back to the ordinary route with previous* fields.
 */
export const DEVICE_PAIR_REPLACE_PATH = relayProfile.deviceReplacementPath;
/** The previous device id and credential a pair request may carry, by route.
 *
 * The ordinary pair route ignores them entirely, so a Worker that does not
 * speak the two-phase contract is never asked to replace anything. The
 * replace route requires both and refuses the request, before anything is
 * read or written, when either is missing or malformed. */
export function pairingPrevious(payload: Record<string, unknown>, route: "pair" | "replace"):
  { ok: true; previous?: { deviceId: string; credential: string } } | { ok: false; error: string } {
  if (route === "pair") return { ok: true };
  const deviceId = payload.previousDeviceId;
  const credential = payload.previousDeviceCredential;
  if (typeof deviceId !== "string" || !/^[A-Za-z0-9-]{8,64}$/.test(deviceId) || typeof credential !== "string" || credential.length < 32 || credential.length > 256) {
    return { ok: false, error: "previousDeviceId and previousDeviceCredential are required" };
  }
  return { ok: true, previous: { deviceId, credential } };
}

/** Connect query parameter a host sets to receive the replacement acknowledgement. */
export const REPLACEMENT_ACK_PARAM = relayProfile.replacementAckQueryParameter;
/** The acknowledgement frame's type, shared with both native hosts. */
export const REPLACEMENT_ACK_FRAME_TYPE = relayProfile.replacementAckFrameType;
/** The frame that tells a re-paired host its previous credential is no longer
 * accepted, so it may discard it. Null when there is nothing to acknowledge. */
export function replacementAckFrame(previousDeviceId: string | null): string | null {
  return previousDeviceId ? JSON.stringify({ type: REPLACEMENT_ACK_FRAME_TYPE, previousDeviceId, previousCredentialAccepted: false }) : null;
}

/** What a connect may do, decided in one transaction just before the upgrade.
 * `admitted` carries the predecessor whose credential is no longer accepted
 * (deleted now or by an earlier connect of this record), which is what the
 * replacement acknowledgement names. `denied` means the record is gone,
 * expired or holds another credential, or the account-write guard refused. A
 * storage failure throws, and the caller reports it as retryable. Either way
 * nothing is upgraded unless the result is `admitted`. */
export type ConnectAdmission =
  | { admitted: true; revokedDeviceId: string | null; replacedDeviceId: string | null }
  | { admitted: false; reason: "record" | "fenced" };

/** Every connect, after authentication, immediately before the upgrade.
 * Serialized per account by the Durable Object transaction.
 *
 * It re-reads the record (exists, same credential hash, unexpired) and runs
 * the account-write guard inside the transaction, so a revoke, re-pair or
 * deletion that committed after authentication denies the connect. For a
 * re-pair it is the second phase: the predecessor is deleted only if it
 * is still exactly the record proven at redemption (same id, same credential
 * hash); the reference is cleared in the same transaction, so a retry is a
 * no-op that still acknowledges. */
export async function adoptReplacement(
  storage: RetentionStorage,
  input: { deviceId: string; credentialHash: string; now: number; credentialTtlMs: number; guard?: AccountWriteGuard },
): Promise<ConnectAdmission> {
  return storage.transaction(async (transaction): Promise<ConnectAdmission> => {
    const key = `${DEVICE_RECORD_PREFIX}${input.deviceId}`;
    const record = await transaction.get<RetainedDevice>(key);
    if (!record || record.credentialHash !== input.credentialHash) return { admitted: false, reason: "record" };
    if (record.expiresAt !== null && (record.expiresAt ?? record.pairedAt + input.credentialTtlMs) <= input.now) return { admitted: false, reason: "record" };
    // Nothing has been written yet, so returning commits nothing. A guard
    // exception is a failure to read state and propagates (retryable).
    if (input.guard && await input.guard(transaction)) return { admitted: false, reason: "fenced" };
    if (!record.replaces) return { admitted: true, revokedDeviceId: null, replacedDeviceId: typeof record.replacedDeviceId === "string" ? record.replacedDeviceId : null };
    const { replaces, ...rest } = record;
    if (typeof replaces.deviceId !== "string" || replaces.deviceId === input.deviceId) {
      await transaction.put(key, rest);
      return { admitted: true, revokedDeviceId: null, replacedDeviceId: null };
    }
    // Whether deleted here or already gone, the predecessor's credential is
    // not accepted after this commit; that is what the acknowledgement says.
    await transaction.put(key, { ...rest, replacedDeviceId: replaces.deviceId } satisfies RetainedDevice);
    const predecessorKey = `${DEVICE_RECORD_PREFIX}${replaces.deviceId}`;
    const predecessor = await transaction.get<RetainedDevice>(predecessorKey);
    if (!predecessor || predecessor.credentialHash !== replaces.credentialHash) return { admitted: true, revokedDeviceId: null, replacedDeviceId: replaces.deviceId };
    await transaction.delete(predecessorKey);
    const queued = await transaction.get<string[]>(DIRECTORY_CLEANUP_KEY) ?? [];
    await transaction.put(DIRECTORY_CLEANUP_KEY, [...new Set([...queued, replaces.deviceId])]);
    await scheduleRetentionWork(transaction, input.now + DIRECTORY_CLEANUP_RETRY_MS);
    return { admitted: true, revokedDeviceId: replaces.deviceId, replacedDeviceId: replaces.deviceId };
  });
}

/** The pair route's KV half, ordered around the Durable Object transaction.
 *
 * 1. The new directory entry is written first. If the transaction then fails,
 *    it points at no record and is deleted best effort.
 * 2. The Durable Object redeems the offer and creates the record, marking a
 *    proven predecessor for replacement on first connect (`adoptReplacement`,
 *    which queues the predecessor's KV entry then).
 * 3. The spent offer is deleted best effort.
 *
 * A retry with the same code is refused by the spent offer, so no step can run
 * twice against a different record.
 *
 * A failed `pair()` call does not say whether its transaction committed: the
 * reply can be lost after the record was created. So the entry is never
 * deleted on the failure alone. The attempt is admitted in the owner's Durable
 * Object before the entry is written, and on failure the owner decides, in one
 * transaction, whether THIS attempt committed (`resolvePairingAttempt`). Only
 * `aborted` authorizes the delete, and the abort has by then refused any
 * delayed `pair()` for the attempt and queued the entry for the account alarm,
 * so a failed delete here is retried there. An answer that stays ambiguous
 * keeps the entry: the attempt stays admitted, durably, and asking again is
 * safe. A committed attempt whose reply was lost is not recoverable by the
 * host (only the credential's hash is stored); it re-pairs, and the unused
 * record ages out with its entry, as for any lost pair response.
 *
 * Admission reserves the device id exclusively (`admitPairingAttempt`), so no
 * second attempt can ever hold the id an abort queued for cleanup.
 *
 * A put that fails is NOT resolved: its outcome is unknown and KV documents no
 * point after which an abandoned put can no longer apply, so any delete could
 * be overtaken by it. The attempt stays admitted (`pending`) indefinitely,
 * the id stays reserved, the alarm's cleanup skips it, and nothing deletes on
 * a guess. If such a put lands, the entry names no record (the host never
 * received a credential). Its KV TTL limits how long it can stay visible,
 * but is not evidence that the write is closed: the attempt stays pending.
 *
 * The Durable Object alone cannot tell "never sent" from "unknown put": an
 * admission whose Worker died before (or during) the put looks the same as
 * one whose put failed, so the alarm never closes an admitted reservation.
 * Only the Worker knows no put was sent, which is why an admission failure
 * is resolved above and a put failure is not. */
export async function completeDevicePairing<Receipt>(
  kv: DirectoryKv,
  input: { owner: string; pairingHash: string; deviceId: string },
  pair: () => Promise<Receipt>,
  attempt: PairingAttempt,
): Promise<Receipt> {
  const directoryKey = deviceDirectoryKey(input.deviceId);
  let admission: PairingAdmission;
  try { admission = await attempt.admit(); }
  catch (reason) {
    // No put has been sent for this attempt, so an abort here cannot be
    // overtaken by one. If the admission committed and only its reply was
    // lost, the resolution aborts it and the alarm acknowledges the (empty)
    // cleanup; if it has not run yet, the resolution fences the id so the
    // delayed admission is refused when it arrives. An unanswered resolution
    // leaves whatever committed in place.
    await resolveFailedPairing(attempt);
    throw reason;
  }
  // A returned refusal is definitive: its transaction wrote nothing, so
  // there is nothing to resolve and no capacity was used.
  if (!admission.admitted) throw new Error(PAIRING_REFUSAL_MESSAGES[admission.refused]);
  await kv.put(directoryKey, JSON.stringify({ owner: input.owner }), { expirationTtl: DEVICE_DIRECTORY_TTL_SECONDS });
  let receipt: Receipt;
  try { receipt = await pair(); }
  catch (reason) {
    if (await resolveFailedPairing(attempt) === "aborted") await kv.delete(directoryKey).catch(() => {});
    throw reason;
  }
  await kv.delete(`qcrc:pair:${input.pairingHash}`).catch(() => {});
  return receipt;
}

/** Ask the owner a bounded number of times; null when no answer arrived. */
async function resolveFailedPairing(attempt: PairingAttempt): Promise<PairingResolution | null> {
  for (let tries = 0; tries < PAIRING_RESOLVE_ATTEMPTS; tries += 1) {
    try { return await attempt.resolve(); }
    catch { /* Ambiguous: it may have committed. Asking again is idempotent. */ }
  }
  return null;
}

/** The per-device reservation the pair route holds in the owner's Durable
 * Object. Device ids are minted per request by the Worker with
 * `crypto.randomUUID()`, so a second attempt for one id is not a normal path;
 * it is refused anyway, for as long as the reservation exists: until
 * compaction, below, for a finished one (an `aborted` one becomes finished
 * when its queued cleanup is acknowledged); for good for `admitted` and
 * `abandoned`. That uniqueness is enforced within this account only, and
 * only while the reservation exists: across accounts
 * (whose Durable Objects cannot see each other, while `qcrc:device:<id>` is
 * one global KV key) it is probabilistic, resting on 122 random bits, exactly
 * as the pair route already did before reservations existed. States:
 *  - `admitted`: an attempt holds the id; its put may be in flight;
 *  - `committed`: that attempt created the record (in the same transaction);
 *  - `aborted`: that attempt can never commit; its entry is queued for cleanup
 *    in the same transaction, and the alarm's acknowledged delete moves it to
 *    `cleanup-acked`;
 *  - `cleanup-acked`: the queued delete was acknowledged;
 *  - `fenced`: resolved before its admission arrived (an admission RPC that
 *    failed in flight); no put was sent, and the delayed admission is refused;
 *  - `abandoned`: an explicit owner or operator release of an `admitted`
 *    one whose put is unknown (`abandonPairingAttempt`); nothing is deleted.
 * It holds no personal data. The retention sweep compacts terminal ones
 * (`compactionDue`); after that the id is refused only by the attempt's spent
 * or expired offer, and reuse rests on the same probabilistic uniqueness. */
export const PAIRING_RESERVATION_PREFIX = "pairing-device:";
export type PairingReservation = { operationId: string; state: "admitted" | "committed" | "aborted" | "cleanup-acked" | "fenced" | "abandoned"; admittedAt: number; committedAt?: number; abortedAt?: number; offerExpiresAt?: number };
/** - `paired`: this attempt created the record; the entry stays.
 *  - `aborted`: this attempt can no longer commit and created nothing; the
 *    entry was queued for cleanup in the abort's transaction, and may be
 *    deleted.
 *  - `unknown`: this account holds no reservation for this attempt (another
 *    attempt's id, another account, or not admitted yet), or holds a fenced
 *    or abandoned one; nothing may be deleted. When no reservation or record
 *    holds the id and the attempt's own offer is live, the delayed admission
 *    is fenced: a per-id `fenced` reservation, or at full cap that offer is
 *    closed to admission. */
export type PairingResolution = "paired" | "aborted" | "unknown";
/** Why an admission was refused. Each is definitive: nothing was written. */
export type PairingRefusal = "offer" | "offer-closed" | "device-reserved" | "open-attempts" | "reservations";
export type PairingAdmission = { admitted: true } | { admitted: false; refused: PairingRefusal };
export type PairingAttempt = { admit(): Promise<PairingAdmission>; resolve(): Promise<PairingResolution> };
export const PAIRING_RESOLVE_ATTEMPTS = 3;
/** A provisional limit, open to change. At most this many reservations
 * may be `admitted` (unresolved) at once. A pairing needs one; the margin
 * lets a person retry while a few attempts are unknown. Retryable: resolving
 * an attempt, or abandoning an unknown one, frees capacity. */
export const MAX_OPEN_PAIRING_ATTEMPTS = 4;
/** A provisional limit, open to change. At most this many
 * reservations of any state per account. Finished ones (`cleanup-acked`,
 * `fenced`, and `committed` whose record is gone) count until the retention
 * sweep compacts them, after their offer expired plus
 * PAIRING_COMPACTION_MARGIN_MS. An `aborted` one is never age-pruned
 * directly, but its cleanup was queued when it was aborted, and once the
 * alarm's delete is acknowledged it is `cleanup-acked` and compactable. A
 * `committed` one frees once its record is gone (retention, revocation or
 * replacement). Only `admitted` (an unknown or in-flight put) and
 * `abandoned` ones never free on their own. So a full account may or may not
 * free up with time, depending on what fills it. */
export const MAX_PAIRING_RESERVATIONS = 2048;
/** A provisional value, open to change. How long after the offer an
 * attempt was bound to has expired a terminal reservation is kept before
 * compaction (`compactionDue`, run by `sweepExpiredDevices`). Covers clock skew between the
 * Worker and this object; correctness rests on the offer, not on this. */
export const PAIRING_COMPACTION_MARGIN_MS = 10 * 60_000;

/** Whether a terminal reservation may be dropped, and when it may next be.
 * Never: `admitted` (an unknown or in-flight put, whatever its age) and
 * `abandoned` (an unknown put an owner released). Not `aborted` directly
 * either: its cleanup is unacknowledged, and it becomes `cleanup-acked` (and
 * so compactable) through the queued drain, not by age. `committed` only
 * once its record is gone; any of them
 * only while no cleanup for the id is queued, and only after its offer
 * expired plus the margin. After that, no delayed RPC for the attempt can
 * succeed: admit and redeem both require the offer (single use, redeemed or
 * expired, and any later offer has a new random code), redeem also requires
 * an admitted reservation for exactly its attempt, and a delayed resolve finds
 * no live offer of its own and writes nothing. The id could be admitted again
 * only by a new attempt minting the same random UUID. */
function compactionDue(reservation: PairingReservation, hasRecord: boolean, queued: boolean, now: number): { drop: boolean; at: number | null } {
  const terminal = reservation.state === "cleanup-acked" || reservation.state === "fenced" || (reservation.state === "committed" && !hasRecord);
  if (!terminal || queued) return { drop: false, at: null };
  const at = (typeof reservation.offerExpiresAt === "number" ? reservation.offerExpiresAt : reservation.admittedAt) + PAIRING_COMPACTION_MARGIN_MS;
  return at <= now ? { drop: true, at: null } : { drop: false, at };
}
/** What the pair route says for each refusal. The offer text is the one the
 * route already repeats to the person. */
export const PAIRING_REFUSAL_MESSAGES: Record<PairingRefusal, string> = {
  offer: "Pairing offer is invalid or expired",
  "offer-closed": "This pairing code can no longer be used. Open REMOTE in QC Remote and start again.",
  "device-reserved": "Device id is reserved",
  "open-attempts": "Too many pairing attempts are unresolved. Try again later.",
  reservations: "This account has reached its pairing limit.",
};
/** How the pair route answers each capacity refusal, through the public
 * failure allowlist (http-security.ts `publicCodes`). The codes are stable;
 * the messages name no account, id or count.
 *  - open attempts: 429 with Retry-After, because a resolution or an
 *    abandonment frees capacity; the wait matches the pairing rate limit's
 *    60 s period.
 *  - total reservations: 409, not 429: capacity frees only as finished
 *    reservations are compacted after their offer expired plus the margin
 *    (aborted ones once their queued cleanup is acknowledged, committed ones
 *    once their record is gone), and not at all while admitted or abandoned
 *    ones fill the account. The client cannot tell which, so no Retry-After
 *    is promised; a later attempt may still succeed.
 *  - a closed offer: 409, because that offer will never admit again; the
 *    person starts a new pairing.
 * The device-reserved refusal stays internal (it never follows from a fresh
 * id) and keeps the route's generic fallback. */
export const PAIRING_REFUSAL_RESPONSES: Readonly<Record<string, { code: string; status: number; retryAfterSeconds?: number }>> = {
  [PAIRING_REFUSAL_MESSAGES["open-attempts"]]: { code: "pairing_attempts_unresolved", status: 429, retryAfterSeconds: 60 },
  [PAIRING_REFUSAL_MESSAGES.reservations]: { code: "pairing_limit_reached", status: 409 },
  [PAIRING_REFUSAL_MESSAGES["offer-closed"]]: { code: "pairing_offer_closed", status: 409 },
};

/** Reserve the device id for one pair-route attempt, before its directory
 * entry is written. In order, and before anything is written, it refuses:
 * an offer that is missing, redeemed, expired or for another code (so no
 * invalid request uses capacity); an id any reservation or record holds; and
 * an account at either cap, counting unresolved (`admitted`) reservations.
 * A refusal is returned, not thrown, so the caller can tell it from a failed
 * call. The reservation records the offer's expiry, which bounds when a
 * delayed RPC for this attempt could still match a live offer. */
export async function admitPairingAttempt(
  storage: RetentionStorage,
  input: { operationId: string; deviceId: string; codeHash: string; now: number },
  equal: (left: string, right: string) => Promise<boolean>,
): Promise<PairingAdmission> {
  return storage.transaction(async (transaction): Promise<PairingAdmission> => {
    const offer = await transaction.get<PairingOffer>("pairing-offer");
    if (!offer || offer.redeemed || offer.expiresAt < input.now || !await equal(input.codeHash, offer.codeHash)) return { admitted: false, refused: "offer" };
    if (offer.admissionClosed) return { admitted: false, refused: "offer-closed" };
    const key = `${PAIRING_RESERVATION_PREFIX}${input.deviceId}`;
    if (await transaction.get(key) !== undefined || await transaction.get(`${DEVICE_RECORD_PREFIX}${input.deviceId}`) !== undefined) return { admitted: false, refused: "device-reserved" };
    const reservations = await transaction.list<PairingReservation>({ prefix: PAIRING_RESERVATION_PREFIX });
    if ([...reservations.values()].filter((reservation) => reservation.state === "admitted").length >= MAX_OPEN_PAIRING_ATTEMPTS) return { admitted: false, refused: "open-attempts" };
    if (reservations.size >= MAX_PAIRING_RESERVATIONS) return { admitted: false, refused: "reservations" };
    await transaction.put(key, { operationId: input.operationId, state: "admitted", admittedAt: input.now, offerExpiresAt: offer.expiresAt } satisfies PairingReservation);
    await scheduleRetentionWork(transaction, offer.expiresAt + PAIRING_COMPACTION_MARGIN_MS);
    return { admitted: true };
  });
}

/** An explicit owner or operator release of an unresolved attempt whose put
 * is unknown, which is the only way such an attempt stops counting against
 * the open cap. It decides nothing about KV: the reservation stops being
 * admitted (so the attempt can never commit, and the id stays refused), but
 * nothing is queued or deleted, because the abandoned put could still land
 * after any delete. The put stays unknown: if it lands, its entry names no
 * record, and its KV TTL limits how long it stays visible, which is not
 * evidence that the write is closed. An abandoned reservation is never
 * compacted, resolves as `unknown` and authorizes no delete. Returns false
 * when the id holds no admitted reservation. */
export async function abandonPairingAttempt(storage: RetentionStorage, deviceId: string, now: number): Promise<boolean> {
  return storage.transaction(async (transaction) => {
    const key = `${PAIRING_RESERVATION_PREFIX}${deviceId}`;
    const reservation = await transaction.get<PairingReservation>(key);
    if (reservation?.state !== "admitted") return false;
    await transaction.put(key, { ...reservation, state: "abandoned", abortedAt: now } satisfies PairingReservation);
    return true;
  });
}

/** Decide a failed attempt from committed state, in one transaction: either
 * this attempt committed, or its reservation is aborted here so it can never
 * commit afterwards, and its entry is queued for the alarm. Repeating it
 * returns the same answer. */
export async function resolvePairingAttempt(
  storage: RetentionStorage,
  operationId: string,
  deviceId: string,
  now: number,
  attemptOffer: { codeHash: string; equal: (left: string, right: string) => Promise<boolean> },
): Promise<PairingResolution> {
  return storage.transaction(async (transaction): Promise<PairingResolution> => {
    const key = `${PAIRING_RESERVATION_PREFIX}${deviceId}`;
    const reservation = await transaction.get<PairingReservation>(key);
    if (!reservation) {
      // The admission has not run (its RPC failed in flight, and may still
      // arrive). Fence the id so it is refused then. No put was sent for an
      // attempt that was never admitted, so nothing is queued.
      // A delayed admission can only match the offer this attempt used
      // (`attemptOffer`, the code hash the Worker admitted with), and only
      // while that offer is live: one that is missing, redeemed or expired
      // never becomes live again, and a later offer has a new random code.
      // So:
      //  - the live offer is not this attempt's (none, spent, or a newer
      //    one): admission validation already refuses it; nothing is written
      //    and no offer is closed;
      //  - this attempt's offer is live and there is room under
      //    MAX_PAIRING_RESERVATIONS: a per-id fence, kept until that offer's
      //    expiry plus the margin;
      //  - this attempt's offer is live and there is no room: that offer is fenced
      //    (`admissionClosed`), so every admission for it is refused. That
      //    rewrites the one offer entry in place and allocates nothing.
      // Every allocation path checks the same cap, so the count is bounded.
      const offer = await transaction.get<PairingOffer>("pairing-offer");
      const live = offer !== undefined && !offer.redeemed && offer.expiresAt >= now && await attemptOffer.equal(attemptOffer.codeHash, offer.codeHash);
      if (live && await transaction.get(`${DEVICE_RECORD_PREFIX}${deviceId}`) === undefined) {
        const count = (await transaction.list<PairingReservation>({ prefix: PAIRING_RESERVATION_PREFIX })).size;
        if (count < MAX_PAIRING_RESERVATIONS) {
          await transaction.put(key, { operationId, state: "fenced", admittedAt: now, abortedAt: now, offerExpiresAt: offer.expiresAt } satisfies PairingReservation);
          await scheduleRetentionWork(transaction, offer.expiresAt + PAIRING_COMPACTION_MARGIN_MS);
        } else if (!offer.admissionClosed) {
          await transaction.put("pairing-offer", { ...offer, admissionClosed: true } satisfies PairingOffer);
        }
      }
      return "unknown";
    }
    if (reservation.operationId !== operationId) return "unknown";
    if (reservation.state === "committed") return "paired";
    // A fence was never admitted, and an abandoned attempt's put is unknown:
    // neither authorizes a delete.
    if (reservation.state === "fenced" || reservation.state === "abandoned") return "unknown";
    if (reservation.state !== "admitted") return "aborted";
    await transaction.put(key, { ...reservation, state: "aborted", abortedAt: now } satisfies PairingReservation);
    const queued = await transaction.get<string[]>(DIRECTORY_CLEANUP_KEY) ?? [];
    await transaction.put(DIRECTORY_CLEANUP_KEY, [...new Set([...queued, deviceId])]);
    await scheduleRetentionWork(transaction, now + DIRECTORY_CLEANUP_RETRY_MS);
    return "aborted";
  });
}

type DirectoryMetadata = { refreshedAt?: number; persistent?: boolean };

/** Remove a live device's legacy KV TTL once after the Durable
 * Object accepted its credential. `stillPaired` is asked after the write, so a
 * revocation that raced it is followed by a delete instead of leaving a
 * resurrected pointer. */
export async function refreshDeviceDirectory(kv: DirectoryKv, deviceId: string, owner: string, now: number, stillPaired: () => Promise<boolean>): Promise<boolean> {
  const key = deviceDirectoryKey(deviceId);
  const { value, metadata } = await kv.getWithMetadata<{ owner?: string }, DirectoryMetadata>(key, "json");
  if (!value || value.owner !== owner) return false;
  if (metadata?.persistent) return false;
  await kv.put(key, JSON.stringify({ owner }), { metadata: { refreshedAt: now, persistent: true } });
  if (!await stillPaired()) {
    await kv.delete(key);
    return false;
  }
  return true;
}

function validRecord(record: unknown): record is RetainedDevice {
  if (!record || typeof record !== "object") return false;
  const value = record as Partial<RetainedDevice>;
  return typeof value.pairedAt === "number" && Number.isSafeInteger(value.pairedAt);
}
