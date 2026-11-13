import test from "node:test";
import assert from "node:assert/strict";
import {
  DEVICE_SWEEP_KEY, DIRECTORY_CLEANUP_KEY, DIRECTORY_CLEANUP_RETRY_MS, MAX_OPEN_PAIRING_ATTEMPTS, PAIRING_COMPACTION_MARGIN_MS,
  PAIRING_REFUSAL_MESSAGES, PAIRING_REFUSAL_RESPONSES, PAIRING_RESERVATION_PREFIX, abandonPairingAttempt, admitPairingAttempt,
  completeDevicePairing, drainDirectoryCleanup, redeemPairingOffer, refreshDeviceDirectory, removeDeviceRecord, resolvePairingAttempt,
  sweepExpiredDevices, type PairingReservation,
} from "../src/device-retention.ts";
import { equal, fakeDirectoryKv, fakeStorage } from "./fakes.ts";

const NOW = 1_800_000_000_000;
const OFFER_MS = 10 * 60_000;
const record = (extra: object = {}) => ({ credentialHash: "h", pairedAt: NOW, expiresAt: null, lastUsedAt: NOW, name: "Host", ...extra });

async function withOffer(storage: ReturnType<typeof fakeStorage>, codeHash: string, expiresAt: number) {
  await storage.transaction((t) => t.put("pairing-offer", { codeHash, expiresAt, redeemed: false }));
}

test("removing a device record queues its directory entry in the same transaction", async () => {
  const storage = fakeStorage();
  await storage.transaction((t) => t.put("device:d1", record()));
  assert.equal(await removeDeviceRecord(storage, "d1", NOW), true);
  assert.equal(storage.data.has("device:d1"), false);
  assert.deepEqual(storage.data.get(DIRECTORY_CLEANUP_KEY), ["d1"]);
  assert.equal(storage.data.get(DEVICE_SWEEP_KEY), NOW + DIRECTORY_CLEANUP_RETRY_MS);
  assert.equal(storage.alarm, NOW + DIRECTORY_CLEANUP_RETRY_MS);
  // A second call finds nothing to remove and queues nothing more.
  assert.equal(await removeDeviceRecord(storage, "d1", NOW), false);
  assert.deepEqual(storage.data.get(DIRECTORY_CLEANUP_KEY), ["d1"]);
});

test("a revoke whose KV delete fails leaves the entry queued until a later drain succeeds", async () => {
  const storage = fakeStorage();
  const kv = fakeDirectoryKv({ "qcrc:device:d1": { owner: "o" } });
  await storage.transaction((t) => t.put("device:d1", record()));
  await removeDeviceRecord(storage, "d1", NOW);
  kv.state.failDelete = true;
  assert.deepEqual(await drainDirectoryCleanup(storage, kv), { deleted: [], failed: ["d1"] });
  assert.deepEqual(storage.data.get(DIRECTORY_CLEANUP_KEY), ["d1"]);
  assert.equal(kv.values.has("qcrc:device:d1"), true);
  kv.state.failDelete = false;
  assert.deepEqual(await drainDirectoryCleanup(storage, kv), { deleted: ["d1"], failed: [] });
  assert.equal(storage.data.has(DIRECTORY_CLEANUP_KEY), false);
  assert.equal(kv.values.has("qcrc:device:d1"), false);
});

test("the drain keeps an entry whose record is live again", async () => {
  const storage = fakeStorage();
  const kv = fakeDirectoryKv({ "qcrc:device:d1": { owner: "o" } });
  await storage.transaction(async (t) => { await t.put("device:d1", record()); await t.put(DIRECTORY_CLEANUP_KEY, ["d1"]); });
  assert.deepEqual(await drainDirectoryCleanup(storage, kv), { deleted: [], failed: [] });
  assert.deepEqual(kv.state.deletes, []);
  assert.equal(storage.data.has(DIRECTORY_CLEANUP_KEY), false);
});

test("a connect that lost a race with a revoke queues the pointer with the owner", async () => {
  const kv = fakeDirectoryKv({ "qcrc:device:d1": { owner: "o" } });
  const queued: number[] = [];
  assert.equal(await refreshDeviceDirectory(kv, "d1", "o", NOW, async () => false, async () => { queued.push(1); }), false);
  assert.deepEqual(queued, [1]);
  assert.deepEqual(kv.state.deletes, []);
});

test("without a working queue the revoked connect falls back to the inline delete; a failed check is queued, not guessed", async () => {
  const kv = fakeDirectoryKv({ "qcrc:device:d1": { owner: "o" } });
  assert.equal(await refreshDeviceDirectory(kv, "d1", "o", NOW, async () => false, async () => { throw new Error("rpc down"); }), false);
  assert.deepEqual(kv.state.deletes, ["qcrc:device:d1"]);
  const second = fakeDirectoryKv({ "qcrc:device:d2": { owner: "o" } });
  const queued: number[] = [];
  assert.equal(await refreshDeviceDirectory(second, "d2", "o", NOW, async () => { throw new Error("rpc down"); }, async () => { queued.push(1); }), false);
  assert.deepEqual(queued, [1]);
  assert.deepEqual(second.state.deletes, []);
  await assert.rejects(refreshDeviceDirectory(second, "d2", "o", NOW, async () => { throw new Error("rpc down"); }), /rpc down/);
});

test("a paired connect leaves the pointer alone", async () => {
  const kv = fakeDirectoryKv({ "qcrc:device:d1": { owner: "o" } });
  assert.equal(await refreshDeviceDirectory(kv, "d1", "o", NOW, async () => true, async () => assert.fail("not queued")), true);
  assert.deepEqual(kv.state.deletes, []);
});

async function admitOpenAttempts(storage: ReturnType<typeof fakeStorage>, count: number, now: number) {
  for (let index = 0; index < count; index += 1) {
    assert.deepEqual(await admitPairingAttempt(storage, { operationId: `op${index}`, deviceId: `dev${index}`, codeHash: "code", now }, equal), { admitted: true });
  }
}

test("unresolved pairing attempts are abandoned by the sweep once their offer has lapsed, freeing the open cap", async () => {
  const storage = fakeStorage();
  const kv = fakeDirectoryKv();
  await withOffer(storage, "code", NOW + OFFER_MS);
  await admitOpenAttempts(storage, MAX_OPEN_PAIRING_ATTEMPTS, NOW);
  assert.deepEqual(await admitPairingAttempt(storage, { operationId: "op-x", deviceId: "dev-x", codeHash: "code", now: NOW }, equal), { admitted: false, refused: "open-attempts" });

  // Before the offer plus margin has passed nothing is abandoned.
  const early = NOW + OFFER_MS + PAIRING_COMPACTION_MARGIN_MS - 1;
  const first = await sweepExpiredDevices(storage, early);
  assert.equal(first.nextSweepAt, NOW + OFFER_MS + PAIRING_COMPACTION_MARGIN_MS);
  for (let index = 0; index < MAX_OPEN_PAIRING_ATTEMPTS; index += 1) assert.equal((storage.data.get(`${PAIRING_RESERVATION_PREFIX}dev${index}`) as PairingReservation).state, "admitted");

  const late = NOW + OFFER_MS + PAIRING_COMPACTION_MARGIN_MS;
  await sweepExpiredDevices(storage, late);
  for (let index = 0; index < MAX_OPEN_PAIRING_ATTEMPTS; index += 1) {
    const reservation = storage.data.get(`${PAIRING_RESERVATION_PREFIX}dev${index}`) as PairingReservation;
    assert.equal(reservation.state, "abandoned");
    assert.equal(reservation.abortedAt, late);
  }
  // Nothing was queued or deleted for them: an abandoned put decides nothing about KV.
  assert.equal(storage.data.has(DIRECTORY_CLEANUP_KEY), false);
  assert.deepEqual(await drainDirectoryCleanup(storage, kv), { deleted: [], failed: [] });
  assert.deepEqual(kv.state.deletes, []);

  // A fresh offer now admits a new attempt; the abandoned ids stay reserved and are never compacted.
  await withOffer(storage, "code2", late + OFFER_MS);
  assert.deepEqual(await admitPairingAttempt(storage, { operationId: "op-new", deviceId: "dev-new", codeHash: "code2", now: late }, equal), { admitted: true });
  assert.deepEqual(await admitPairingAttempt(storage, { operationId: "op-again", deviceId: "dev0", codeHash: "code2", now: late }, equal), { admitted: false, refused: "device-reserved" });
  await sweepExpiredDevices(storage, late + 365 * 24 * 60 * 60_000);
  assert.equal((storage.data.get(`${PAIRING_RESERVATION_PREFIX}dev0`) as PairingReservation).state, "abandoned");
});

test("a delayed redeem for an abandoned attempt still cannot commit", async () => {
  const storage = fakeStorage();
  await withOffer(storage, "code", NOW + OFFER_MS);
  await admitOpenAttempts(storage, 1, NOW);
  await sweepExpiredDevices(storage, NOW + OFFER_MS + PAIRING_COMPACTION_MARGIN_MS);
  await assert.rejects(redeemPairingOffer(storage, { codeHash: "code", deviceId: "dev0", operationId: "op0", record: record(), now: NOW + OFFER_MS + PAIRING_COMPACTION_MARGIN_MS }, equal), /invalid or expired/);
  await withOffer(storage, "code", NOW + 3 * OFFER_MS);
  await assert.rejects(redeemPairingOffer(storage, { codeHash: "code", deviceId: "dev0", operationId: "op0", record: record(), now: NOW + OFFER_MS + PAIRING_COMPACTION_MARGIN_MS }, equal), /not admitted/);
  assert.equal(storage.data.has("device:dev0"), false);
  assert.equal(await resolvePairingAttempt(storage, "op0", "dev0", NOW + 3 * OFFER_MS, { codeHash: "code", equal }), "unknown");
});

test("an explicit operator release abandons only an admitted attempt", async () => {
  const storage = fakeStorage();
  await withOffer(storage, "code", NOW + OFFER_MS);
  await admitOpenAttempts(storage, 1, NOW);
  assert.equal(await abandonPairingAttempt(storage, "dev0", NOW), true);
  assert.equal(await abandonPairingAttempt(storage, "dev0", NOW), false);
  assert.equal(await abandonPairingAttempt(storage, "missing", NOW), false);
});

test("the open-attempt refusal no longer promises that waiting a minute helps", () => {
  const response = PAIRING_REFUSAL_RESPONSES[PAIRING_REFUSAL_MESSAGES["open-attempts"]];
  assert.equal(response.status, 409);
  assert.equal(response.retryAfterSeconds, undefined);
  assert.equal(response.code, "pairing_attempts_unresolved");
});

test("a pairing attempt resolves to aborted (queued for cleanup), paired, or unknown from committed state", async () => {
  const storage = fakeStorage();
  await withOffer(storage, "code", NOW + OFFER_MS);
  await admitOpenAttempts(storage, 2, NOW);
  assert.equal(await resolvePairingAttempt(storage, "op0", "dev0", NOW, { codeHash: "code", equal }), "aborted");
  assert.equal(await resolvePairingAttempt(storage, "op0", "dev0", NOW, { codeHash: "code", equal }), "aborted");
  assert.deepEqual(storage.data.get(DIRECTORY_CLEANUP_KEY), ["dev0"]);
  await redeemPairingOffer(storage, { codeHash: "code", deviceId: "dev1", operationId: "op1", record: record(), now: NOW }, equal);
  assert.equal(await resolvePairingAttempt(storage, "op1", "dev1", NOW, { codeHash: "code", equal }), "paired");
  assert.equal(await resolvePairingAttempt(storage, "other", "dev1", NOW, { codeHash: "code", equal }), "unknown");
});

test("completeDevicePairing deletes the entry only on an aborted resolution", async () => {
  const storage = fakeStorage();
  const kv = fakeDirectoryKv();
  await withOffer(storage, "code", NOW + OFFER_MS);
  const attempt = (operationId: string, deviceId: string) => ({
    admit: () => admitPairingAttempt(storage, { operationId, deviceId, codeHash: "code", now: NOW }, equal),
    resolve: () => resolvePairingAttempt(storage, operationId, deviceId, NOW, { codeHash: "code", equal }),
  });
  await assert.rejects(completeDevicePairing(kv, { owner: "o", pairingHash: "ph", deviceId: "devA" }, async () => { throw new Error("pair lost"); }, attempt("opA", "devA")), /pair lost/);
  assert.deepEqual(kv.state.deletes, ["qcrc:device:devA"]);
});
