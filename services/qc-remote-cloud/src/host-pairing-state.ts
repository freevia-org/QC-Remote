import type { HostPairingSession, PairingMetadata } from "./host-pairing.ts";

/** `pairingCode` is present only while the result is `completed` and not yet
 * collected. The first authenticated poll returns it and, in the same
 * transaction, moves the state to `delivered` without it, so the plaintext is
 * held only until the host that created the session collects it once. */
export type HostPairingResult = { pairingCode?: string; pairing: PairingMetadata; completedAt: number };
export type HostPairingState = {
  version: 1;
  status: "pending" | "claimed" | "completed" | "delivered" | "cancelled";
  session: HostPairingSession;
  claimId?: string;
  result?: HostPairingResult;
};

export type HostPairingTransaction = {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<boolean>;
  setAlarm(time: number | Date): Promise<void>;
  deleteAlarm(): Promise<void>;
};
export type HostPairingStorage = {
  transaction<T>(callback: (transaction: HostPairingTransaction) => Promise<T>): Promise<T>;
};

const HOST_PAIRING_STATE_KEY = "host-pairing-state-v1";

function validSession(value: unknown): value is HostPairingSession {
  if (!value || typeof value !== "object") return false;
  const session = value as Partial<HostPairingSession>;
  return typeof session.receiverSecretHash === "string"
    && /^[a-f0-9]{64}$/.test(session.receiverSecretHash)
    && Number.isSafeInteger(session.expiresAt)
    && session.expiresAt! > 0
    && (session.createdAt === undefined || Number.isSafeInteger(session.createdAt))
    && session.pairingCode === undefined
    && session.completedAt === undefined
    && session.pairing === undefined
    && (session.host === undefined || validDeclaredHost(session.host))
    && (session.origin === undefined || validRequestLocation(session.origin));
}

function validDeclaredHost(value: unknown): boolean {
  if (!value || typeof value !== "object") return false;
  const host = value as { label?: unknown; platform?: unknown };
  return (host.label === undefined || typeof host.label === "string") && (host.platform === undefined || typeof host.platform === "string");
}

function validRequestLocation(value: unknown): boolean {
  if (!value || typeof value !== "object") return false;
  const location = value as { country?: unknown; city?: unknown };
  return (location.country === undefined || typeof location.country === "string") && (location.city === undefined || typeof location.city === "string");
}

function validPairing(value: unknown): value is PairingMetadata {
  if (!value || typeof value !== "object") return false;
  const pairing = value as Partial<PairingMetadata>;
  return typeof pairing.email === "string" && typeof pairing.device === "string" && Number.isSafeInteger(pairing.pairedAt);
}

export function validHostPairingState(value: unknown): value is HostPairingState {
  if (!value || typeof value !== "object") return false;
  const state = value as Partial<HostPairingState>;
  if (state.version !== 1 || !["pending", "claimed", "completed", "delivered", "cancelled"].includes(String(state.status)) || !validSession(state.session)) return false;
  if (state.status === "pending" || state.status === "cancelled") return state.claimId === undefined && state.result === undefined;
  if (typeof state.claimId !== "string" || !/^[a-f0-9]{48}$/.test(state.claimId)) return false;
  if (state.status === "claimed") return state.result === undefined;
  if (state.status === "delivered") return validDeliveredResult(state.result);
  return validResult(state.result);
}

function validResult(value: unknown): value is HostPairingResult & { pairingCode: string } {
  if (!value || typeof value !== "object") return false;
  const result = value as Partial<HostPairingResult>;
  return typeof result.pairingCode === "string" && /^[a-f0-9]{48}$/.test(result.pairingCode) && validPairing(result.pairing) && Number.isSafeInteger(result.completedAt);
}

/** A collected result keeps only what the confirmation page needs to say the
 * link was used; the plaintext code must be gone. */
function validDeliveredResult(value: unknown): value is HostPairingResult {
  if (!value || typeof value !== "object") return false;
  const result = value as Partial<HostPairingResult>;
  return result.pairingCode === undefined && validPairing(result.pairing) && Number.isSafeInteger(result.completedAt);
}

async function readOrInitialize(transaction: HostPairingTransaction, candidate?: HostPairingSession): Promise<HostPairingState | null> {
  const stored = await transaction.get<unknown>(HOST_PAIRING_STATE_KEY);
  if (stored !== undefined) return validHostPairingState(stored) ? stored : null;
  const initial = candidateState(candidate);
  if (!initial) return null;
  await transaction.put(HOST_PAIRING_STATE_KEY, initial);
  await transaction.setAlarm(initial.session.expiresAt);
  return initial;
}

function candidateState(value: unknown): HostPairingState | null {
  if (validSession(value)) return { version: 1, status: "pending", session: value };
  if (!value || typeof value !== "object") return null;
  const candidate = value as HostPairingSession;
  const { pairingCode, completedAt, pairing, ...session } = candidate;
  if (!validSession(session) || typeof completedAt !== "number" || !Number.isSafeInteger(completedAt) || !validPairing(pairing)) return null;
  // A completed KV record that still carries a code predates the Durable
  // Object authority. Migrate it as completed so the host can still
  // collect the code once, while an unguessable sentinel claim can never be
  // presented by a callback to reopen it.
  if (typeof pairingCode === "string" && /^[a-f0-9]{48}$/.test(pairingCode)) {
    return { version: 1, status: "completed", session, claimId: "0".repeat(48), result: { pairingCode, completedAt, pairing } };
  }
  // A completed mirror without a code is what this relay writes now: the code
  // only ever lives in the Durable Object. Rebuilt from it, the session is one
  // whose code has already been collected, so it fails closed.
  if (pairingCode === undefined) {
    return { version: 1, status: "delivered", session, claimId: "0".repeat(48), result: { completedAt, pairing } };
  }
  return null;
}

export async function initializeHostPairingState(storage: HostPairingStorage, session: HostPairingSession): Promise<boolean> {
  if (!validSession(session)) return false;
  return storage.transaction(async (transaction) => {
    if (await transaction.get(HOST_PAIRING_STATE_KEY) !== undefined) return false;
    await transaction.put(HOST_PAIRING_STATE_KEY, { version: 1, status: "pending", session } satisfies HostPairingState);
    await transaction.setAlarm(session.expiresAt);
    return true;
  });
}

/** Remove the isolated pairing record at expiry. Owner sessions use the same
 * class but never write this key, so their MCP-presence alarm remains intact. */
export async function expireHostPairingState(storage: HostPairingStorage, now: number): Promise<boolean> {
  return storage.transaction(async (transaction) => {
    const value = await transaction.get<unknown>(HOST_PAIRING_STATE_KEY);
    if (value === undefined) return false;
    if (!validHostPairingState(value) || value.session.expiresAt <= now) {
      await transaction.delete(HOST_PAIRING_STATE_KEY);
      await transaction.deleteAlarm();
      return true;
    }
    await transaction.setAlarm(value.session.expiresAt);
    return true;
  });
}

/** Route a Durable Object alarm to pairing expiry or the existing presence
 * refresh. Pairing objects share the class, but their namespaced ids must not
 * refresh an owner's presence lease. */
export async function dispatchDeviceSessionAlarm(
  storage: HostPairingStorage,
  now: number,
  refreshPresence: () => Promise<unknown>,
): Promise<void> {
  if (await expireHostPairingState(storage, now)) return;
  await refreshPresence();
}

export async function readHostPairingState(storage: HostPairingStorage, candidate?: HostPairingSession): Promise<HostPairingState | null> {
  return storage.transaction((transaction) => readOrInitialize(transaction, candidate));
}

export async function claimHostPairingState(storage: HostPairingStorage, candidate: HostPairingSession | null, claimId: string, now: number): Promise<{ ok: true; session: HostPairingSession; claimId: string } | { ok: false; reason: "invalid-session" | "expired" | "used" }> {
  if (!/^[a-f0-9]{48}$/.test(claimId)) return { ok: false, reason: "invalid-session" };
  return storage.transaction(async (transaction) => {
    const state = await readOrInitialize(transaction, candidate ?? undefined);
    if (!state) return { ok: false, reason: "invalid-session" };
    if (state.session.expiresAt <= now) return { ok: false, reason: "expired" };
    if (state.status !== "pending") return { ok: false, reason: "used" };
    await transaction.put(HOST_PAIRING_STATE_KEY, { ...state, status: "claimed", claimId });
    return { ok: true, session: state.session, claimId };
  });
}

export async function completeHostPairingState(storage: HostPairingStorage, claimId: string, pairingCode: string, pairing: PairingMetadata, now: number): Promise<boolean> {
  if (!/^[a-f0-9]{48}$/.test(claimId) || !/^[a-f0-9]{48}$/.test(pairingCode) || !validPairing(pairing) || !Number.isSafeInteger(now)) return false;
  return storage.transaction(async (transaction) => {
    const state = await readOrInitialize(transaction);
    if (!state || state.status !== "claimed" || state.claimId !== claimId || state.session.expiresAt <= now) return false;
    const result: HostPairingResult = { pairingCode, pairing, completedAt: now };
    await transaction.put(HOST_PAIRING_STATE_KEY, { ...state, status: "completed", result });
    return true;
  });
}

export async function cancelHostPairingState(storage: HostPairingStorage, candidate: HostPairingSession | null, now: number): Promise<"cancelled" | "invalid-session" | "expired" | "used"> {
  return storage.transaction(async (transaction) => {
    const state = await readOrInitialize(transaction, candidate ?? undefined);
    if (!state) return "invalid-session";
    if (state.session.expiresAt <= now) return "expired";
    if (state.status !== "pending") return "used";
    await transaction.put(HOST_PAIRING_STATE_KEY, { ...state, status: "cancelled" });
    return "cancelled";
  });
}

/** The host's poll. The completed code is returned exactly once.
 *
 * The write that strips it happens in the same Durable Object transaction as
 * the read that returns it, so two polls racing for one result serialize: the
 * first gets the code, the second sees `delivered` and gets `null`, which the
 * route reports as the same 404 an expired session gets. If that write fails,
 * the transaction throws and nothing is returned, so a storage error can never
 * hand the code out while leaving it stored. A response lost in transit costs
 * the host a new pairing session; that is the price of not keeping the code. */
export async function pollHostPairingState(storage: HostPairingStorage, candidate: HostPairingSession | null, receiverSecretHash: string, now: number): Promise<{ state: "waiting"; expiresAt: number } | { state: "complete"; pairingCode: string; expiresAt: number } | null> {
  return storage.transaction(async (transaction) => {
    const state = await readOrInitialize(transaction, candidate ?? undefined);
    if (!state || state.session.expiresAt <= now || !constantTimeEqual(state.session.receiverSecretHash, receiverSecretHash)) return null;
    if (state.status === "pending" || state.status === "claimed") return { state: "waiting", expiresAt: state.session.expiresAt };
    if (state.status === "completed" && validResult(state.result)) {
      const { pairingCode, ...kept } = state.result;
      await transaction.put(HOST_PAIRING_STATE_KEY, { ...state, status: "delivered", result: kept } satisfies HostPairingState);
      return { state: "complete", pairingCode, expiresAt: state.session.expiresAt };
    }
    return null;
  });
}

function constantTimeEqual(left: string, right: string): boolean {
  if (left.length !== right.length) return false;
  let difference = 0;
  for (let index = 0; index < left.length; index += 1) difference |= left.charCodeAt(index) ^ right.charCodeAt(index);
  return difference === 0;
}
