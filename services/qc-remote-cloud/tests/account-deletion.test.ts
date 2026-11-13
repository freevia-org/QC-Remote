import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  ACCOUNT_DELETION_KEY, ACCOUNT_GENERATION_KEY, KV_VISIBILITY_MARGIN_MS, LEGACY_RECONCILE_INTERVAL_MS, LEGACY_RECONCILE_MAX_INTERVAL_MS,
  PROVIDER_WRITES_KEY, PROVIDER_WRITE_REFUSED_MESSAGE, accountDeletionDeadline, admitMcpRequest, admitProviderWrite, beginAccountDeletion,
  claimLegacyKey, fenceProviderKv, markProviderDelete, publicDeletionStatus, readAccountDeletion, reconcileLegacy, settleProviderWrite,
  validAccountDeletionState, type LegacyDrainAttestation, type ProviderKv, type ProviderWriteFence, type ReconcilingAccountDeletion,
} from "../src/account-deletion.ts";
import type { ConnectorGrantRegistry } from "../src/connector-grants.ts";
import { fakeStorage } from "./fakes.ts";

const T0 = 1_800_000_000_000;
const HOUR = 60 * 60_000;
const OPERATION = "a".repeat(32);

// The provider the deletion fence's proof (account-deletion.ts) rests on. The
// proof's comments cite the 0.10.4 source; the dependency is pinned to the
// version below and the proof has NOT been re-verified against it. This test
// only makes any change to the installed provider impossible to miss: a bump
// must come with a re-qualification of the fence (every grant/token mutation
// goes through env.OAUTH_KV, helpers are cached on env.OAUTH_PROVIDER, token
// keys are token:<userId>:<grantId>:<id>) and an update of this constant.
const PINNED_OAUTH_PROVIDER_VERSION = "1.2.4";

test("the OAuth provider the deletion fence depends on has not changed unnoticed", () => {
  const manifest = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")) as { dependencies: Record<string, string> };
  assert.equal(manifest.dependencies["@cloudflare/workers-oauth-provider"], PINNED_OAUTH_PROVIDER_VERSION, "the dependency must stay an exact pin");
  let installed: string | undefined;
  for (const base of ["../node_modules", "../../../node_modules"]) {
    try { installed = (JSON.parse(readFileSync(new URL(`${base}/@cloudflare/workers-oauth-provider/package.json`, import.meta.url), "utf8")) as { version: string }).version; break; } catch { /* try the next */ }
  }
  assert.equal(installed, PINNED_OAUTH_PROVIDER_VERSION, "re-qualify the deletion fence (account-deletion.ts) before changing the OAuth provider version");
});

function reconciling(overrides: Partial<ReconcilingAccountDeletion> = {}): ReconcilingAccountDeletion {
  return { version: 1, status: "reconciling", operationId: OPERATION, startedAt: T0, accessRevokedAt: T0, subject: "sub-1", lastLegacyDeleteAt: T0, retryAt: T0 + KV_VISIBILITY_MARGIN_MS, ...overrides };
}

function registry(grants: Array<{ id: string; expiresAt?: number }> = []) {
  const revoked: string[] = [];
  const api: ConnectorGrantRegistry = {
    listUserGrants: async () => ({ items: grants.filter((grant) => !revoked.includes(grant.id)) }),
    revokeGrant: async (id: string) => { revoked.push(id); },
  } as unknown as ConnectorGrantRegistry;
  return { api, revoked };
}
const emptyKv: ProviderKv = { list: async () => ({ keys: [], list_complete: true }), delete: async () => {} };

async function reconcileAt(storage: ReturnType<typeof fakeStorage>, now: number, attestation: LegacyDrainAttestation | null, grants = registry()) {
  return reconcileLegacy(storage as never, grants.api, emptyKv, now, attestation, () => {});
}

test("a reconciliation that cannot complete backs off instead of waking hourly for ever", async () => {
  const storage = fakeStorage();
  await storage.transaction((t) => t.put(ACCOUNT_DELETION_KEY, reconciling()));
  let now = T0 + KV_VISIBILITY_MARGIN_MS;
  const intervals: number[] = [];
  for (let pass = 0; pass < 12; pass += 1) {
    const outcome = await reconcileAt(storage, now, null);
    assert.deepEqual(outcome, { status: "pending", operationId: OPERATION, step: "legacy-reconciliation" });
    const state = await storage.transaction((t) => readAccountDeletion(t)) as ReconcilingAccountDeletion;
    intervals.push(state.retryAt - now);
    now = state.retryAt;
  }
  assert.equal(intervals[0], LEGACY_RECONCILE_INTERVAL_MS);
  for (let index = 1; index < intervals.length; index += 1) assert.ok(intervals[index] >= intervals[index - 1], `non-decreasing at ${index}`);
  assert.ok(intervals[3] > intervals[0]);
  assert.equal(intervals.at(-1), LEGACY_RECONCILE_MAX_INTERVAL_MS);
  assert.ok(intervals.every((interval) => interval <= LEGACY_RECONCILE_MAX_INTERVAL_MS));
  // Fail-closed status is unchanged: still pending, access revoked, no completion time.
  assert.deepEqual(publicDeletionStatus(await storage.transaction((t) => readAccountDeletion(t)), null), { status: "pending", operationId: OPERATION, startedAt: T0, step: "legacy-reconciliation", accessRevoked: true, until: null });
});

test("finding a legacy grant resets the backoff to one hour", async () => {
  const storage = fakeStorage();
  await storage.transaction((t) => t.put(ACCOUNT_DELETION_KEY, reconciling({ lastLegacyDeleteAt: T0 })));
  const now = T0 + 20 * HOUR;
  const grants = registry([{ id: "legacygrant1", expiresAt: Math.floor((now + 5 * HOUR) / 1000) }]);
  await reconcileAt(storage, now, null, grants);
  assert.deepEqual(grants.revoked, ["legacygrant1"]);
  const state = await storage.transaction((t) => readAccountDeletion(t)) as ReconcilingAccountDeletion;
  assert.equal(state.lastLegacyDeleteAt, now);
  assert.equal(state.retryAt - now, LEGACY_RECONCILE_INTERVAL_MS);
});

test("a reconciliation with an attestation that has passed still completes on the first clean pass", async () => {
  const storage = fakeStorage();
  await storage.transaction((t) => t.put(ACCOUNT_DELETION_KEY, reconciling()));
  const attestation: LegacyDrainAttestation = { lastPossibleLegacyCompletionAt: T0, maxLegacyLifetimeMs: 1_000, evidence: "test" };
  const now = T0 + KV_VISIBILITY_MARGIN_MS + 1;
  const outcome = await reconcileAt(storage, now, attestation);
  assert.equal(outcome?.status, "deleted");
  const state = await storage.transaction((t) => readAccountDeletion(t));
  assert.equal(state?.status, "deleted");
});

test("a completable reconciliation keeps its fixed interval and completion time", async () => {
  const storage = fakeStorage();
  await storage.transaction((t) => t.put(ACCOUNT_DELETION_KEY, reconciling({ lastLegacyDeleteAt: T0 })));
  const attestation: LegacyDrainAttestation = { lastPossibleLegacyCompletionAt: T0 + 90 * 60_000, maxLegacyLifetimeMs: 0, evidence: "test" };
  await reconcileAt(storage, T0 + 20 * 60_000, attestation);
  const state = await storage.transaction((t) => readAccountDeletion(t)) as ReconcilingAccountDeletion;
  // One hour after this pass is earlier than the completion time, so it wins.
  assert.equal(state.retryAt, T0 + 20 * 60_000 + LEGACY_RECONCILE_INTERVAL_MS);
});

test("a failing pass counts as activity and keeps the account pending", async () => {
  const storage = fakeStorage();
  await storage.transaction((t) => t.put(ACCOUNT_DELETION_KEY, reconciling({ lastLegacyDeleteAt: T0 })));
  const broken = { listUserGrants: async () => { throw new Error("kv 503"); }, revokeGrant: async () => {} } as unknown as ConnectorGrantRegistry;
  const now = T0 + 30 * HOUR;
  const outcome = await reconcileLegacy(storage as never, broken, emptyKv, now, null, () => {});
  assert.equal(outcome?.status, "pending");
  const state = await storage.transaction((t) => readAccountDeletion(t)) as ReconcilingAccountDeletion;
  assert.equal(state.lastLegacyDeleteAt, now);
  assert.equal(state.retryAt - now, LEGACY_RECONCILE_INTERVAL_MS);
});

test("beginAccountDeletion purges account data, keeps the fence and is idempotent", async () => {
  const storage = fakeStorage();
  await storage.transaction(async (t) => {
    await t.put("device:d1", { pairedAt: T0 });
    await t.put("pairing-offer", { codeHash: "abc", expiresAt: T0, redeemed: false });
    await t.put(ACCOUNT_GENERATION_KEY, 3);
  });
  const marker = await beginAccountDeletion(storage as never, "sub-1", T0, OPERATION);
  assert.equal(marker.status, "pending");
  assert.deepEqual(marker.deviceIds, ["d1"]);
  assert.equal(marker.offerCodeHash, "abc");
  assert.equal(storage.data.has("device:d1"), false);
  assert.equal(storage.data.get(ACCOUNT_GENERATION_KEY), 3);
  assert.equal(validAccountDeletionState(storage.data.get(ACCOUNT_DELETION_KEY)), true);
  assert.deepEqual(await beginAccountDeletion(storage as never, "sub-1", T0 + 1, "b".repeat(32)), marker);
  assert.equal(accountDeletionDeadline(marker), marker.retryAt);
  assert.equal(await admitMcpRequest(storage as never, 3), "deleting");
  assert.deepEqual(await admitProviderWrite(storage as never, "grant:access-sub-1:g1", 3, undefined, T0), { admitted: false });
});

test("provider writes are journalled before they land and a tombstone refuses a later put", async () => {
  const storage = fakeStorage();
  const admitted = await admitProviderWrite(storage as never, "grant:access-sub-1:g1", 0, undefined, T0, "c".repeat(32));
  assert.deepEqual(admitted, { admitted: true, admissionId: "c".repeat(32) });
  assert.equal(await settleProviderWrite(storage as never, "c".repeat(32)), "keep");
  assert.deepEqual(await admitProviderWrite(storage as never, "grant:access-sub-1:g1", 7, undefined, T0), { admitted: false }, "a stale generation is refused");
  await markProviderDelete(storage as never, "grant:access-sub-1:g1", T0);
  assert.deepEqual(await admitProviderWrite(storage as never, "grant:access-sub-1:g1", 0, undefined, T0), { admitted: false }, "a deleted grant is not resurrected");
  assert.deepEqual(await admitProviderWrite(storage as never, "token:access-sub-1:g1:t1", 0, undefined, T0), { admitted: false }, "a token needs a live journalled grant");
  assert.equal(await claimLegacyKey(storage as never, "grant:access-sub-1:g1", T0), false, "a journalled provider write is never claimed as legacy");
  assert.ok(storage.data.has(PROVIDER_WRITES_KEY));
});

/** A KV that records its calls; `failClientPut` makes client rewrites throw. */
function providerKv(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  const calls: string[] = [];
  const state = { failClientPut: false, failClientGet: false };
  return {
    values, calls, state,
    put: async (key: string, value: unknown) => {
      calls.push(`put ${key}`);
      if (key.startsWith("client:") && state.failClientPut) throw new Error("kv 503");
      values.set(key, String(value));
    },
    delete: async (key: string) => { calls.push(`delete ${key}`); values.delete(key); },
    get: async (key: string) => { calls.push(`get ${key}`); if (key.startsWith("client:") && state.failClientGet) throw new Error("kv 503"); return values.get(key) ?? null; },
  };
}
const keepFence = (): ProviderWriteFence => ({
  admitProviderWrite: async () => ({ admitted: true, admissionId: "d".repeat(32) }),
  settleProviderWrite: async () => "keep",
  closeCompensation: async () => {},
  markProviderDelete: async () => {},
});
const grantKey = "grant:access-sub-1:grant1";
const legacyGrant = JSON.stringify({ clientId: "client1", refreshTokenId: "r", expiresAt: 4_000_000_000, metadata: { accountGeneration: 0 } });
const currentGrant = JSON.stringify({ clientId: "client1", refreshTokenId: "r", metadata: { accountGeneration: 0 } });

test("a legacy grant upgrade migrates its client once; refresh rotations of current grants cost no client write", async () => {
  const kv = providerKv({ "client:client1": "{\"id\":1}" });
  const fenced = fenceProviderKv(kv, () => keepFence());
  await fenced.put(grantKey, legacyGrant, { expiration: 4_000_000_000 });
  assert.deepEqual(kv.calls.filter((call) => call.includes("client:")), ["get client:client1", "put client:client1"]);
  assert.equal(JSON.parse(kv.values.get(grantKey)!).expiresAt, undefined, "the upgraded grant no longer expires");
  kv.calls.length = 0;
  for (let rotation = 0; rotation < 3; rotation += 1) await fenced.put(grantKey, currentGrant);
  assert.deepEqual(kv.calls.filter((call) => call.includes("client:")), []);
});

test("a failing client migration never fails the grant put", async () => {
  for (const failure of ["failClientPut", "failClientGet"] as const) {
    const kv = providerKv({ "client:client1": "{}" });
    kv.state[failure] = true;
    const fenced = fenceProviderKv(kv, () => keepFence());
    await fenced.put(grantKey, legacyGrant, { expiration: 4_000_000_000 });
    assert.ok(kv.values.has(grantKey), failure);
  }
});

test("a refused provider write still throws the refusal", async () => {
  const kv = providerKv();
  const fenced = fenceProviderKv(kv, () => ({ ...keepFence(), admitProviderWrite: async () => ({ admitted: false }) }));
  await assert.rejects(fenced.put(grantKey, currentGrant), new RegExp(PROVIDER_WRITE_REFUSED_MESSAGE.slice(0, 30)));
  assert.equal(kv.values.has(grantKey), false);
});
