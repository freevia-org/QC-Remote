import { deletionDeadline, scheduleRetentionWork, type PairingReservation, type RetainedDevice, type RetentionTransaction } from "./device-retention.ts";
import { ACCOUNT_DELETION_KEY, ACCOUNT_GENERATION_KEY, PROVIDER_WRITES_KEY, PROVIDER_WRITES_SWEEP_KEY, PROVIDER_WRITES_SWEEP_ATTEMPTS_KEY, validAccountDeletionState, validProviderWrites } from "./account-deletion.ts";
import { validHostPairingState, type HostPairingState } from "./host-pairing-state.ts";
import type { LateResultLog, LateResultState } from "./late-results.ts";
import { MCP_NAME_RETENTION_MS } from "./mcp-presence.ts";

export const DEVICE_SESSION_INVENTORY_MAX_IDS = 32;
export const DEVICE_SESSION_INVENTORY_MAX_KEYS = 4_096;
export const DEVICE_SESSION_LATE_RESULTS_KEY = "late-results";

export type DeviceSessionInventoryKind = "owner" | "host-pairing" | "empty" | "mixed" | "unknown" | "malformed";

type InventoryFailure = {
  schemaVersion: 1;
  complete: false;
  outcome: "incomplete" | "error";
  reason: "key-limit" | "state-changed" | "storage-read-failed";
};

export type CompleteDeviceSessionInventory = {
  schemaVersion: 1;
  /** The bounded storage list was complete and internally consistent when it
   * was read. This is not a freshness guarantee after the RPC returns and does
   * not prove that an independently due alarm did not run. */
  complete: true;
  kind: DeviceSessionInventoryKind;
  keyCount: number;
  deviceRecordCount: number;
  pairingReservationCount: number;
  directoryCleanupCount: number;
  presenceRowCount: number;
  unknownKeyCount: number;
  malformedKeyCount: number;
  alarmAt?: number;
  sweepAt?: number;
  earliestDeviceDeletionAt?: number;
  latestDeviceDeletionAt?: number;
  pairingOfferExpiresAt?: number;
  hostPairingExpiresAt?: number;
  presenceRetentionUntil?: number;
};

export type DeviceSessionInventory = CompleteDeviceSessionInventory | InventoryFailure;

export type DeviceSessionInventoryStorage = {
  list<T>(options: { limit: number }): Promise<Map<string, T>>;
  getAlarm(): Promise<number | null>;
};

export type DeviceSessionRetentionBackfillResult =
  | { schemaVersion: 1; complete: true; accepted: true; outcome: "scheduled" | "skipped"; kind: "owner" | "host-pairing" | "empty" }
  | { schemaVersion: 1; complete: true; accepted: false; outcome: "refused"; kind: "mixed" | "unknown" | "malformed" }
  | InventoryFailure;

export type DeviceSessionRetentionTransaction = RetentionTransaction & DeviceSessionInventoryStorage;
export type DeviceSessionRetentionStorage = {
  transaction<T>(callback: (transaction: DeviceSessionRetentionTransaction) => Promise<T>): Promise<T>;
};

const HOST_PAIRING_STATE_KEY = "host-pairing-state-v1";
const OWNER_EXACT_KEYS = new Set([
  DEVICE_SESSION_LATE_RESULTS_KEY,
  "mcp-presence-v1",
  "pairing-offer",
  "usage:daily",
  "device-retention-sweep-at-v1",
  "device-directory-cleanup-v1",
  ACCOUNT_DELETION_KEY,
  ACCOUNT_GENERATION_KEY,
  PROVIDER_WRITES_KEY,
  PROVIDER_WRITES_SWEEP_KEY,
  PROVIDER_WRITES_SWEEP_ATTEMPTS_KEY,
]);

export async function restoreDeviceSessionLateResults(
  storage: { get<T>(key: string): Promise<T | undefined> },
  log: Pick<LateResultLog<string>, "restore">,
): Promise<void> {
  const stored = await storage.get<unknown>(DEVICE_SESSION_LATE_RESULTS_KEY);
  log.restore(validLateResults(stored) ? stored : { abandoned: [], late: [] });
}

/** The constructor's complete persistent-state initialization, expressed as
 * injected operations so its read-only behavior can be verified without a
 * Worker runtime. The auto-response setup is runtime state; the only durable
 * operation is one read of the bounded late-result log. */
export function initializeDeviceSessionState(
  configureAutoResponse: () => void,
  blockConcurrencyWhile: (callback: () => Promise<void>) => unknown,
  storage: { get<T>(key: string): Promise<T | undefined> },
  log: Pick<LateResultLog<string>, "restore">,
): void {
  configureAutoResponse();
  blockConcurrencyWhile(() => restoreDeviceSessionLateResults(storage, log));
}

export function validateDeviceSessionInventoryIds(value: unknown): string[] {
  if (!Array.isArray(value) || value.length > DEVICE_SESSION_INVENTORY_MAX_IDS) throw new Error("Inventory batch is invalid");
  if (new Set(value).size !== value.length || value.some((id) => typeof id !== "string" || !/^[a-f0-9]{64}$/.test(id))) {
    throw new Error("Inventory object id is invalid");
  }
  return value;
}

function safeTimestamp(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function validOptionalString(value: unknown): boolean {
  return value === undefined || typeof value === "string";
}

function validRetainedDeviceForInventory(value: unknown): value is RetainedDevice {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Partial<RetainedDevice>;
  if (typeof record.credentialHash !== "string" || !/^[a-f0-9]{64}$/.test(record.credentialHash) || !safeTimestamp(record.pairedAt)) return false;
  if (record.expiresAt !== undefined && !safeTimestamp(record.expiresAt)) return false;
  if (record.lastUsedAt !== undefined && !safeTimestamp(record.lastUsedAt)) return false;
  if (!validOptionalString(record.replacedDeviceId) || !validOptionalString(record.pairingOperationId)) return false;
  if (record.replaces !== undefined) {
    if (!record.replaces || typeof record.replaces !== "object" || Array.isArray(record.replaces)) return false;
    if (typeof record.replaces.deviceId !== "string"
      || typeof record.replaces.credentialHash !== "string"
      || !/^[a-f0-9]{64}$/.test(record.replaces.credentialHash)) return false;
  }
  const current = value as Record<string, unknown>;
  const pairing = current.pairing;
  const pairingValid = pairing === undefined || (
    pairing !== null
    && typeof pairing === "object"
    && !Array.isArray(pairing)
    && typeof (pairing as Record<string, unknown>).email === "string"
    && typeof (pairing as Record<string, unknown>).device === "string"
    && safeTimestamp((pairing as Record<string, unknown>).pairedAt)
  );
  if (typeof current.name === "string" && current.identityProvider === "google") return pairingValid;
  // Explicit legacy contract: before the current display/authentication fields,
  // a retained record contained only retention/replacement fields. A partial
  // current record is malformed rather than silently treated as legacy.
  return current.name === undefined && current.identityProvider === undefined && pairing === undefined;
}

function validPairingOffer(value: unknown): value is { expiresAt: number } {
  if (!value || typeof value !== "object") return false;
  const offer = value as { codeHash?: unknown; expiresAt?: unknown; redeemed?: unknown; admissionClosed?: unknown };
  return typeof offer.codeHash === "string"
    && safeTimestamp(offer.expiresAt)
    && typeof offer.redeemed === "boolean"
    && (offer.admissionClosed === undefined || typeof offer.admissionClosed === "boolean");
}

function validPairingReservation(value: unknown): value is PairingReservation {
  if (!value || typeof value !== "object") return false;
  const reservation = value as Partial<PairingReservation>;
  return typeof reservation.operationId === "string"
    && ["admitted", "committed", "aborted", "cleanup-acked", "fenced", "abandoned"].includes(String(reservation.state))
    && safeTimestamp(reservation.admittedAt)
    && (reservation.committedAt === undefined || safeTimestamp(reservation.committedAt))
    && (reservation.abortedAt === undefined || safeTimestamp(reservation.abortedAt))
    && (reservation.offerExpiresAt === undefined || safeTimestamp(reservation.offerExpiresAt));
}

function validLateResults(value: unknown): value is LateResultState<string> {
  if (!value || typeof value !== "object") return false;
  const state = value as { abandoned?: unknown; late?: unknown };
  if (!Array.isArray(state.abandoned) || !Array.isArray(state.late) || state.abandoned.length > 32 || state.late.length > 32) return false;
  const abandonedValid = state.abandoned.every((entry) => {
    if (!Array.isArray(entry) || entry.length !== 2 || typeof entry[0] !== "string" || !entry[1] || typeof entry[1] !== "object") return false;
    const abandoned = entry[1] as { owner?: unknown; method?: unknown; deadlineMs?: unknown; abandonedAt?: unknown };
    return validOptionalString(abandoned.owner)
      && typeof abandoned.method === "string"
      && safeTimestamp(abandoned.deadlineMs)
      && safeTimestamp(abandoned.abandonedAt);
  });
  const lateValid = state.late.every((entry) => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) return false;
    const late = entry as { method?: unknown; deadlineMs?: unknown; arrivedAfterDeadlineMs?: unknown; ok?: unknown };
    return typeof late.method === "string"
      && safeTimestamp(late.deadlineMs)
      && safeTimestamp(late.arrivedAfterDeadlineMs)
      && typeof late.ok === "boolean";
  });
  return abandonedValid && lateValid;
}

type InventoryPresence = {
  connectedAtUnixMs: number;
  lastSeenAtUnixMs: number;
  expiresAtUnixMs: number;
  rememberedUntilUnixMs: number;
};

function validPresence(value: unknown): value is InventoryPresence[] {
  return Array.isArray(value) && value.every((row) => {
    if (!row || typeof row !== "object") return false;
    const presence = row as Record<string, unknown>;
    return typeof presence.id === "string"
      && typeof presence.key === "string"
      && typeof presence.name === "string"
      && safeTimestamp(presence.connectedAtUnixMs)
      && safeTimestamp(presence.lastSeenAtUnixMs)
      && safeTimestamp(presence.expiresAtUnixMs)
      && safeTimestamp(presence.rememberedUntilUnixMs)
      && ["clientId", "grantId", "version", "device", "browser", "ipAddress", "country", "city"].every((key) => validOptionalString(presence[key]));
  });
}

function presenceRetentionDeadline(row: InventoryPresence): number {
  const retentionLimit = row.lastSeenAtUnixMs + MCP_NAME_RETENTION_MS;
  return Math.min(row.rememberedUntilUnixMs, retentionLimit);
}

function validUsage(value: unknown): boolean {
  if (!value || typeof value !== "object") return false;
  const usage = value as { day?: unknown; commands?: unknown; lastCommandAt?: unknown };
  return typeof usage.day === "string"
    && /^\d{4}-\d{2}-\d{2}$/.test(usage.day)
    && Number.isSafeInteger(usage.commands)
    && Number(usage.commands) >= 0
    && (usage.lastCommandAt === undefined || safeTimestamp(usage.lastCommandAt));
}

function validCleanup(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

function alarmValue(value: number | null): number | undefined | false {
  if (value === null) return undefined;
  return safeTimestamp(value) ? value : false;
}

/** A single bounded, read-only snapshot of one DeviceSession object.
 *
 * The result deliberately contains no storage keys, ids, names, hashes, state
 * payloads or error strings. A caller must retain the opaque object id it used
 * to address the Durable Object. `complete: true` means only that this bounded,
 * point-in-time list was not truncated and its before/after alarm observation
 * was consistent; it is not a freshness guarantee after return and cannot
 * suppress an independently due alarm. */
export function classifyDeviceSessionSnapshot(snapshot: Map<string, unknown>, alarm: number | null): DeviceSessionInventory {
  if (snapshot.size > DEVICE_SESSION_INVENTORY_MAX_KEYS) {
    return { schemaVersion: 1, complete: false, outcome: "incomplete", reason: "key-limit" };
  }
  const parsedAlarm = alarmValue(alarm);
  if (parsedAlarm === false) {
    return { schemaVersion: 1, complete: false, outcome: "incomplete", reason: "state-changed" };
  }

  let hostPairing: HostPairingState | undefined;
  let ownerKeys = 0;
  let deviceRecordCount = 0;
  let pairingReservationCount = 0;
  let directoryCleanupCount = 0;
  let presenceRowCount = 0;
  let unknownKeyCount = 0;
  let malformedKeyCount = 0;
  let sweepAt: number | undefined;
  let pairingOfferExpiresAt: number | undefined;
  let presenceRetentionUntil: number | undefined;
  const deviceDeadlines: number[] = [];

  for (const [key, value] of snapshot) {
    if (key === HOST_PAIRING_STATE_KEY) {
      if (validHostPairingState(value)) hostPairing = value;
      else malformedKeyCount += 1;
      continue;
    }
    if (key.startsWith("device:")) {
      ownerKeys += 1;
      if (validRetainedDeviceForInventory(value)) {
        deviceRecordCount += 1;
        deviceDeadlines.push(deletionDeadline(value as RetainedDevice));
      } else malformedKeyCount += 1;
      continue;
    }
    if (key.startsWith("pairing-device:")) {
      ownerKeys += 1;
      if (validPairingReservation(value)) pairingReservationCount += 1;
      else malformedKeyCount += 1;
      continue;
    }
    if (!OWNER_EXACT_KEYS.has(key)) {
      unknownKeyCount += 1;
      continue;
    }
    ownerKeys += 1;
    if (key === DEVICE_SESSION_LATE_RESULTS_KEY && !validLateResults(value)) malformedKeyCount += 1;
    else if (key === "mcp-presence-v1") {
      if (!validPresence(value)) malformedKeyCount += 1;
      else {
        presenceRowCount = value.length;
        presenceRetentionUntil = value.length === 0 ? undefined : Math.max(...value.map(presenceRetentionDeadline));
      }
    } else if (key === "pairing-offer") {
      if (!validPairingOffer(value)) malformedKeyCount += 1;
      else pairingOfferExpiresAt = value.expiresAt;
    } else if (key === "usage:daily" && !validUsage(value)) malformedKeyCount += 1;
    else if (key === "device-retention-sweep-at-v1") {
      if (!safeTimestamp(value)) malformedKeyCount += 1;
      else sweepAt = value;
    } else if (key === "device-directory-cleanup-v1") {
      if (!validCleanup(value)) malformedKeyCount += 1;
      else directoryCleanupCount = value.length;
    } else if (key === ACCOUNT_DELETION_KEY && !validAccountDeletionState(value)) malformedKeyCount += 1;
    else if (key === ACCOUNT_GENERATION_KEY && !(Number.isSafeInteger(value) && Number(value) >= 0)) malformedKeyCount += 1;
    else if (key === PROVIDER_WRITES_KEY && !validProviderWrites(value)) malformedKeyCount += 1;
    else if (key === PROVIDER_WRITES_SWEEP_KEY && !safeTimestamp(value)) malformedKeyCount += 1;
    else if (key === PROVIDER_WRITES_SWEEP_ATTEMPTS_KEY && !(Number.isSafeInteger(value) && Number(value) >= 0 && Number(value) <= 9)) malformedKeyCount += 1;
  }

  // Unknown keys take precedence over mixed host/owner state: a result must
  // never imply that every row was understood when any key family was not.
  const kind: DeviceSessionInventoryKind = snapshot.size === 0 ? "empty"
    : malformedKeyCount > 0 ? "malformed"
    : unknownKeyCount > 0 ? "unknown"
    : hostPairing && ownerKeys > 0 ? "mixed"
    : hostPairing ? "host-pairing"
    : ownerKeys > 0 ? "owner"
    : "unknown";

  return {
    schemaVersion: 1,
    complete: true,
    kind,
    keyCount: snapshot.size,
    deviceRecordCount,
    pairingReservationCount,
    directoryCleanupCount,
    presenceRowCount,
    unknownKeyCount,
    malformedKeyCount,
    ...(parsedAlarm === undefined ? {} : { alarmAt: parsedAlarm }),
    ...(sweepAt === undefined ? {} : { sweepAt }),
    ...(deviceDeadlines.length === 0 ? {} : {
      earliestDeviceDeletionAt: Math.min(...deviceDeadlines),
      latestDeviceDeletionAt: Math.max(...deviceDeadlines),
    }),
    ...(pairingOfferExpiresAt === undefined ? {} : { pairingOfferExpiresAt }),
    ...(hostPairing === undefined ? {} : { hostPairingExpiresAt: hostPairing.session.expiresAt }),
    ...(presenceRetentionUntil === undefined ? {} : { presenceRetentionUntil }),
  };
}

export async function inspectDeviceSessionStorage(storage: DeviceSessionInventoryStorage): Promise<DeviceSessionInventory> {
  let before: number | null;
  let after: number | null;
  let snapshot: Map<string, unknown>;
  try {
    before = await storage.getAlarm();
    snapshot = await storage.list<unknown>({ limit: DEVICE_SESSION_INVENTORY_MAX_KEYS + 1 });
    after = await storage.getAlarm();
  } catch {
    return { schemaVersion: 1, complete: false, outcome: "error", reason: "storage-read-failed" };
  }
  if (before !== after) {
    return { schemaVersion: 1, complete: false, outcome: "incomplete", reason: "state-changed" };
  }
  return classifyDeviceSessionSnapshot(snapshot, before);
}

/** Atomically reclassify one object and make the ordinary retention alarm due.
 * The alarm remains the sole deletion path: this call never deletes a record,
 * directory entry, presence row, socket, or alarm. Repeating it can schedule
 * another harmless sweep, but cannot extend a retention deadline. */
export async function requestLegacyRetentionBackfill(
  storage: DeviceSessionRetentionStorage,
  now = Date.now(),
): Promise<DeviceSessionRetentionBackfillResult> {
  if (!safeTimestamp(now)) return { schemaVersion: 1, complete: false, outcome: "error", reason: "storage-read-failed" };
  try {
    return await storage.transaction(async (transaction) => {
      const snapshot = await transaction.list<unknown>({ limit: DEVICE_SESSION_INVENTORY_MAX_KEYS + 1 });
      const inventory = classifyDeviceSessionSnapshot(snapshot, await transaction.getAlarm());
      if (!inventory.complete) return inventory;
      if (inventory.kind === "owner") {
        await scheduleRetentionWork(transaction, now);
        return { schemaVersion: 1, complete: true, accepted: true, outcome: "scheduled", kind: "owner" };
      }
      if (inventory.kind === "empty" || inventory.kind === "host-pairing") {
        return { schemaVersion: 1, complete: true, accepted: true, outcome: "skipped", kind: inventory.kind };
      }
      return { schemaVersion: 1, complete: true, accepted: false, outcome: "refused", kind: inventory.kind };
    });
  } catch {
    return { schemaVersion: 1, complete: false, outcome: "error", reason: "storage-read-failed" };
  }
}

export async function inspectDeviceSessionIds(
  value: unknown,
  inspect: (id: string) => Promise<DeviceSessionInventory>,
): Promise<Array<{ id: string; inventory: DeviceSessionInventory }>> {
  const ids = validateDeviceSessionInventoryIds(value);
  const results: Array<{ id: string; inventory: DeviceSessionInventory }> = [];
  for (const id of ids) results.push({ id, inventory: await inspect(id) });
  return results;
}

export async function backfillDeviceSessionIds(
  value: unknown,
  backfill: (id: string) => Promise<DeviceSessionRetentionBackfillResult>,
): Promise<Array<{ id: string; backfill: DeviceSessionRetentionBackfillResult }>> {
  const ids = validateDeviceSessionInventoryIds(value);
  const results: Array<{ id: string; backfill: DeviceSessionRetentionBackfillResult }> = [];
  for (const id of ids) results.push({ id, backfill: await backfill(id) });
  return results;
}
