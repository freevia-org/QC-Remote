import { env, exports } from "cloudflare:workers";
import { runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { http, HttpResponse } from "msw";
import { describe, expect, it } from "vitest";
import { network } from "./network-setup.ts";
import {
  ACCOUNT_DELETION_KEY,
  InvalidProviderWritesJournalError,
  PROVIDER_0_10_4_MAX_LIFETIME_MS,
  PROVIDER_WRITES_DELETION_RETRY_MAX_MS,
  PROVIDER_WRITES_KEY,
  PROVIDER_WRITES_SWEEP_ATTEMPTS_KEY,
  PROVIDER_WRITES_SWEEP_KEY,
  beginAccountDeletion,
  fenceProviderKv,
  PROVIDER_WRITE_REFUSED_MESSAGE,
  scheduleProviderWritesRetention,
  runAccountDeletion,
  type DeletionStorage,
  type ProviderWriteFence,
  type ProviderKv,
} from "../../src/account-deletion.ts";
import type { ConnectorGrantRegistry } from "../../src/connector-grants.ts";
import { DEVICE_RETENTION_MS, DEVICE_SWEEP_KEY } from "../../src/device-retention.ts";
import type { DeviceSession } from "../../src/index.ts";

type ProxyableKv = KVNamespace & {
  put(key: string, value: unknown, options?: unknown): Promise<void>;
};

const encoder = new TextEncoder();
const base64url = (bytes: ArrayBuffer | Uint8Array) => btoa(String.fromCharCode(...(bytes instanceof ArrayBuffer ? new Uint8Array(bytes) : bytes)))
  .replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/u, "");

async function pkceChallenge(verifier: string): Promise<string> {
  return base64url(await crypto.subtle.digest("SHA-256", encoder.encode(verifier)));
}

async function signedGoogleIdentity(audience: string, subject: string): Promise<{ token: string; jwk: JsonWebKey & { kid: string } }> {
  const pair = await crypto.subtle.generateKey({ name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" }, true, ["sign", "verify"]) as CryptoKeyPair;
  const publicJwk = await crypto.subtle.exportKey("jwk", pair.publicKey) as JsonWebKey;
  const jwk = { ...publicJwk, kid: "runtime-smoke-key" };
  const head = base64url(encoder.encode(JSON.stringify({ alg: "RS256", kid: jwk.kid })));
  const payload = base64url(encoder.encode(JSON.stringify({
    iss: "https://accounts.google.com", aud: audience, email_verified: true,
    exp: Math.floor(Date.now() / 1_000) + 300, iat: Math.floor(Date.now() / 1_000),
    sub: subject, email: `${subject}@example.test`, name: "Runtime smoke user",
  })));
  const content = `${head}.${payload}`;
  const signature = await crypto.subtle.sign("RSASSA-PKCS1-v1_5", pair.privateKey, encoder.encode(content));
  return { token: `${content}.${base64url(signature)}`, jwk };
}

async function workerRequest(path: string, init?: RequestInit): Promise<Response> {
  return exports.default.fetch(new Request(`https://qcrc.example.test${path}`, { ...init, redirect: "manual" }));
}

function makeFence(events: string[], refusedKey?: string): ProviderWriteFence {
  return {
    async admitProviderWrite(key, generation, expiresAt) {
      events.push(`admit:${key}:${generation ?? "none"}:${expiresAt ?? "none"}`);
      return key === refusedKey
        ? { admitted: false }
        : { admitted: true, admissionId: `admission:${key}` };
    },
    async settleProviderWrite(admissionId) {
      events.push(`settle:${admissionId}`);
      return "keep";
    },
    async closeCompensation(admissionId, _now) {
      events.push(`close-compensation:${admissionId}`);
    },
    async markProviderDelete(key) {
      events.push(`mark-delete:${key}`);
    },
  };
}

async function invokeProductionAlarmAndTraceProviderDeletes(target: ReturnType<typeof env.DEVICE_SESSION.getByName>, attempted: string[]): Promise<void> {
  await runInDurableObject(target, async (instance: DeviceSession) => {
    // Wrap only the real KV binding's delete method; the production alarm,
    // deletion handler, and Workerd storage remain the code under test.
    const instrumented = instance as unknown as { alarm(): Promise<void>; providerKv(): ProviderKv };
    const prototypeMethod = Object.getPrototypeOf(instance) as { providerKv: () => ProviderKv };
    const originalProviderKv = prototypeMethod.providerKv.bind(instance);
    const native = originalProviderKv();
    instrumented.providerKv = () => new Proxy(native, {
      get(targetKv, property) {
        if (property === "delete") return async (key: string) => { attempted.push(key); await native.delete(key); };
        const value = Reflect.get(targetKv, property, targetKv) as unknown;
        return typeof value === "function" ? value.bind(targetKv) : value;
      },
    });
    try { await instrumented.alarm(); }
    finally { delete (instrumented as unknown as { providerKv?: () => ProviderKv }).providerKv; }
  });
}

describe("provider KV proxy against a Workers KV binding", () => {
  it("passes unowned puts and deletes through without invoking the write fence", async () => {
    const realKv = env.TEST_KV;
    const key = `client:${crypto.randomUUID()}:unowned-pass-through`;
    const events: string[] = [];
    const fenced = fenceProviderKv(realKv as unknown as ProxyableKv, () => makeFence(events));

    try {
      await fenced.put(key, "unowned-value", { expirationTtl: 120, metadata: { source: "workers-runtime-smoke" } });
      await expect(fenced.get(key)).resolves.toBe("unowned-value");
      await expect(fenced.getWithMetadata(key)).resolves.toMatchObject({ value: "unowned-value", metadata: { source: "workers-runtime-smoke" } });

      await fenced.delete(key);
      await expect(fenced.get(key)).resolves.toBeNull();
      expect(events).toEqual([]);
    } finally {
      await realKv.delete(key);
    }
  });

  it("preserves native receivers, forwards KV write options, and fences only owned mutations", async () => {
    const realKv = env.TEST_KV;
    const id = crypto.randomUUID();
    const subject = `kv-proxy-${id}`;
    const unownedA = `client:${id}:a`;
    const unownedB = `client:${id}:b`;
    const unownedDelete = `client:${id}:delete`;
    const grantExpiration = Math.floor(Date.now() / 1_000) + 3_600;
    const grantKey = `grant:access-${subject}:grant-expiration`;
    const tokenKey = `token:access-${subject}:grant-expiration:refresh-expiration`;
    const refusedKey = `token:access-${subject}:grant-expiration:refused`;
    const cleanupKeys = [unownedA, unownedB, unownedDelete, grantKey, tokenKey, refusedKey];
    const events: string[] = [];
    const fixedNow = 2_000_000_000_000;
    const fenced = fenceProviderKv(realKv as unknown as ProxyableKv, (actualSubject) => {
      expect(actualSubject).toBe(subject);
      return makeFence(events, refusedKey);
    }, () => fixedNow);

    try {
      await realKv.put(unownedA, "a", { metadata: { source: "workers-runtime-smoke" } });
      await realKv.put(unownedB, "b");
      await realKv.put(unownedDelete, "delete-me");

      // Detect the illegal-invocation receiver bug without treating an
      // immediately visible distributed write as a product guarantee.
      await expect(fenced.get(unownedA)).resolves.toBe("a");
      await expect(fenced.getWithMetadata(unownedA)).resolves.toMatchObject({ value: "a", metadata: { source: "workers-runtime-smoke" } });
      // Calling list exercises the native namespace receiver. Its contents are
      // deliberately not asserted: KV list visibility is eventually consistent.
      const page = await fenced.list({ prefix: `client:${id}:`, limit: 1 });
      expect(Array.isArray(page.keys)).toBe(true);
      expect(typeof page.list_complete).toBe("boolean");

      const grantValue = JSON.stringify({ metadata: { accountGeneration: 7 } });
      await fenced.put(grantKey, grantValue, { expiration: grantExpiration });
      await fenced.put(tokenKey, "refresh", { expirationTtl: 120 });
      await fenced.delete(grantKey);
      await fenced.delete(unownedDelete);
      await expect(fenced.put(refusedKey, "must-not-land")).rejects.toThrow(PROVIDER_WRITE_REFUSED_MESSAGE);

      expect(events).toEqual([
        `admit:${grantKey}:7:${grantExpiration * 1_000}`,
        `settle:admission:${grantKey}`,
        `admit:${tokenKey}:none:${fixedNow + 120_000}`,
        `settle:admission:${tokenKey}`,
        `mark-delete:${grantKey}`,
        `admit:${refusedKey}:none:none`,
      ]);
    } finally {
      await Promise.all(cleanupKeys.map((key) => realKv.delete(key)));
    }
  });
});

describe("OAuth provider lifecycle on the fenced Workerd KV binding", () => {
  it("runs the production authorize route, Google callback, code exchange, refresh, and revoke handlers", async () => {
    const id = crypto.randomUUID();
    const redirectUri = "https://client.example.test/oauth/callback";
    const verifier = `runtime-smoke-${id}-pkce-verifier-with-more-than-forty-three-characters`;
    const subject = `runtime-smoke-${id}`;
    const google = await signedGoogleIdentity(env.GOOGLE_CLIENT_ID, subject);
    network.use(
      http.post("https://oauth2.googleapis.com/token", () => HttpResponse.json({ id_token: google.token })),
      http.get("https://www.googleapis.com/oauth2/v3/certs", () => HttpResponse.json({ keys: [google.jwk] })),
    );

    const registration = await workerRequest("/register", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        client_name: "Workers runtime smoke",
        redirect_uris: [redirectUri],
        token_endpoint_auth_method: "none",
        response_types: ["code"],
        grant_types: ["authorization_code", "refresh_token"],
      }),
    });
    expect(registration.status).toBe(201);
    const client = await registration.json() as { client_id: string };

    const authorizeUrl = new URL("https://qcrc.example.test/authorize");
    authorizeUrl.search = new URLSearchParams({
      response_type: "code", client_id: client.client_id, redirect_uri: redirectUri,
      scope: "qc:read offline_access", state: `state-${id}`,
      code_challenge: await pkceChallenge(verifier), code_challenge_method: "S256",
    }).toString();
    const consent = await workerRequest(`${authorizeUrl.pathname}${authorizeUrl.search}`);
    expect(consent.status).toBe(200);
    const html = await consent.text();
    const nonce = html.match(/name="nonce" value="([^"]+)"/u)?.[1];
    const csrf = consent.headers.get("set-cookie")?.match(/(__Host-qcrc-csrf-[^;]+)/u)?.[1];
    expect(nonce).toBeTruthy();
    expect(csrf).toBe(`${csrf?.split("=")[0]}=${nonce}`);

    const googleRedirect = await workerRequest("/authorize", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded", cookie: csrf! },
      body: new URLSearchParams({ nonce: nonce! }),
    });
    expect(googleRedirect.status, await googleRedirect.clone().text()).toBe(302);
    const googleUrl = new URL(googleRedirect.headers.get("location")!);
    expect(googleUrl.origin).toBe("https://accounts.google.com");
    const flowCookie = googleRedirect.headers.get("set-cookie")?.match(/(__Host-qcrc-flow-[^;]+)/u)?.[1];
    expect(flowCookie).toBeTruthy();

    const callback = await workerRequest(`/auth/google/mcp/callback?${new URLSearchParams({ state: googleUrl.searchParams.get("state")!, code: "runtime-smoke-google-code" })}`, {
      headers: { cookie: flowCookie! },
    });
    expect(callback.status).toBe(302);
    const authorizationCode = new URL(callback.headers.get("location")!).searchParams.get("code");
    expect(authorizationCode).toBeTruthy();

    const exchange = await workerRequest("/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ grant_type: "authorization_code", client_id: client.client_id, code: authorizationCode!, redirect_uri: redirectUri, code_verifier: verifier }),
    });
    expect(exchange.status).toBe(200);
    const issued = await exchange.json() as { access_token: string; refresh_token: string };

    const refresh = await workerRequest("/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ grant_type: "refresh_token", client_id: client.client_id, refresh_token: issued.refresh_token }),
    });
    expect(refresh.status).toBe(200);
    const rotated = await refresh.json() as { access_token: string; refresh_token: string };
    expect(rotated.access_token).not.toBe(issued.access_token);
    expect(rotated.refresh_token).not.toBe(issued.refresh_token);

    const revoke = await workerRequest("/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ client_id: client.client_id, token: rotated.refresh_token, token_type_hint: "refresh_token" }),
    });
    expect(revoke.status).toBe(200);
  });

});

describe("provider retention on Durable Object storage and alarms", () => {
  const stub = () => env.DEVICE_SESSION.getByName(`workers-runtime-smoke-${crypto.randomUUID()}`);
  const admissionId = "0123456789abcdef0123456789abcdef";
  const providerKey = "token:access-runtime-smoke:grant-1:refresh-1";

  it("runs production DeviceSession.alarm and keeps deletion ahead of provider/device sweeps", async () => {
    const target = stub();
    const deviceId = `runtime-smoke-${crypto.randomUUID()}`;
    const now = Date.now();
    const expiredAt = now - DEVICE_RETENTION_MS - 1;
    await env.QCRC_OAUTH.put(`qcrc:device:${deviceId}`, JSON.stringify({ owner: "runtime-smoke" }));
    await runInDurableObject(target, async (_instance: DeviceSession, state) => {
      const deletion = await beginAccountDeletion(state.storage, "runtime-smoke-subject", now);
      await state.storage.put(ACCOUNT_DELETION_KEY, { ...deletion, retryAt: now - 1 });
      await state.storage.put(`device:${deviceId}`, {
        credentialHash: "runtime-smoke-hash", name: "fixture", pairedAt: expiredAt,
        lastUsedAt: expiredAt, identityProvider: "google",
      });
      await state.storage.put(DEVICE_SWEEP_KEY, now - 1);
      await state.storage.put(PROVIDER_WRITES_KEY, {
        open: { [admissionId]: { key: providerKey, admittedAt: now - 1, state: "writing" } },
        keys: { [providerKey]: { expiresAt: now - 1 } },
      });
      await state.storage.put(PROVIDER_WRITES_SWEEP_KEY, now - 1);
      await state.storage.put(PROVIDER_WRITES_SWEEP_ATTEMPTS_KEY, 3);
      // Keep the alarm in the future so Workerd does not dispatch it before
      // the explicit runDurableObjectAlarm call below.
      await state.storage.setAlarm(now + 60_000);
    });

    expect(await runDurableObjectAlarm(target)).toBe(true);
    await runInDurableObject(target, async (_instance: DeviceSession, state) => {
      const deletion = await state.storage.get<{ status: string; step?: string }>(ACCOUNT_DELETION_KEY);
      const journal = await state.storage.get<{ open: Record<string, unknown>; keys: Record<string, unknown> }>(PROVIDER_WRITES_KEY);
      expect(deletion).toMatchObject({ status: "pending", step: "provider-writes" });
      expect(journal?.open[admissionId]).toBeDefined();
      expect(journal?.keys[providerKey]).toBeDefined();
      // A normal journal sweep deletes this counter before sweeping; it remains
      // because production alarm returned early for the pending deletion.
      expect(await state.storage.get(PROVIDER_WRITES_SWEEP_ATTEMPTS_KEY)).toBe(3);
      expect(await state.storage.get(`device:${deviceId}`)).toBeDefined();
      expect(await state.storage.getAlarm()).toBeGreaterThan(Date.now());
    });
    await env.QCRC_OAUTH.delete(`qcrc:device:${deviceId}`);
  });

  it("backs off a stuck provider-write deletion across production alarms and caps at 24 hours", async () => {
    const target = stub();
    const startedAt = Date.now();
    const stuckProviderKey = `token:access-runtime-smoke-${crypto.randomUUID()}:grant-1:refresh-1`;
    const attemptedDeletes: string[] = [];
    await runInDurableObject(target, async (_instance: DeviceSession, state) => {
        const deletion = await beginAccountDeletion(state.storage, "runtime-smoke-stuck-subject", startedAt);
        await state.storage.put(ACCOUNT_DELETION_KEY, { ...deletion, retryAt: startedAt - 1 });
        await state.storage.put(PROVIDER_WRITES_KEY, {
          open: { [admissionId]: { key: stuckProviderKey, admittedAt: startedAt, state: "writing" } },
          keys: { [stuckProviderKey]: { expiresAt: startedAt - 1 } },
        });
        // Keep Workerd from dispatching in the background; the test invokes the
        // real alarm explicitly, then advances only the persisted retry deadline.
        await state.storage.setAlarm(startedAt + 60_000);
      });
      for (const expectedCount of [1, 2]) {
        const before = Date.now();
        await runInDurableObject(target, async (_instance: DeviceSession, state) => {
          const deletion = await state.storage.get<{ status: string; providerWritesRetryCount?: number; retryAt?: number }>(ACCOUNT_DELETION_KEY);
          await state.storage.put(ACCOUNT_DELETION_KEY, { ...deletion, retryAt: before - 1 });
          await state.storage.setAlarm(before + 60_000);
        });
        await invokeProductionAlarmAndTraceProviderDeletes(target, attemptedDeletes);
        expect(attemptedDeletes.filter((key) => key === stuckProviderKey)).toHaveLength(expectedCount);
        await runInDurableObject(target, async (_instance: DeviceSession, state) => {
          const deletion = await state.storage.get<{ status: string; step?: string; providerWritesRetryCount?: number; retryAt?: number }>(ACCOUNT_DELETION_KEY);
          expect(deletion).toMatchObject({ status: "pending", step: "provider-writes", providerWritesRetryCount: expectedCount });
          expect(deletion?.retryAt).toBeGreaterThan(before);
          const expectedDelay = Math.min(5 * 60_000 * 2 ** expectedCount, PROVIDER_WRITES_DELETION_RETRY_MAX_MS);
          expect(Math.abs(deletion!.retryAt! - before - expectedDelay)).toBeLessThanOrEqual(1_000);
          expect(await state.storage.getAlarm()).toBeLessThanOrEqual(deletion!.retryAt!);
          expect(await state.storage.get<{ open: Record<string, unknown> }>(PROVIDER_WRITES_KEY)).toMatchObject({ open: { [admissionId]: expect.any(Object) } });
        });
      }

      const capCheckAt = Date.now();
      await runInDurableObject(target, async (_instance: DeviceSession, state) => {
        const deletion = await state.storage.get<{ providerWritesRetryCount?: number }>(ACCOUNT_DELETION_KEY);
        await state.storage.put(ACCOUNT_DELETION_KEY, { ...deletion, providerWritesRetryCount: 9, retryAt: capCheckAt - 1 });
        await state.storage.setAlarm(capCheckAt + 60_000);
      });
      await invokeProductionAlarmAndTraceProviderDeletes(target, attemptedDeletes);
      expect(attemptedDeletes.filter((key) => key === stuckProviderKey)).toHaveLength(3);
      await runInDurableObject(target, async (_instance: DeviceSession, state) => {
        const deletion = await state.storage.get<{ status: string; providerWritesRetryCount?: number; retryAt?: number }>(ACCOUNT_DELETION_KEY);
        expect(deletion).toMatchObject({ status: "pending", providerWritesRetryCount: 9 });
        expect(Math.abs(deletion!.retryAt! - capCheckAt - PROVIDER_WRITES_DELETION_RETRY_MAX_MS)).toBeLessThanOrEqual(1_000);
        expect(await state.storage.get<{ open: Record<string, unknown> }>(PROVIDER_WRITES_KEY)).toMatchObject({ open: { [admissionId]: expect.any(Object) } });
      });
  });

  it("fails closed on a malformed journal while an admitted provider put is delayed", async () => {
    const target = stub();
    const subject = `runtime-smoke-malformed-journal-${crypto.randomUUID()}`;
    const key = `grant:access-${subject}:grant-1`;
    const now = Date.now();
    const values = new Map<string, string>();
    const providerDeletes: string[] = [];
    let revokeCalls = 0;
    let releasePut!: () => void;
    let markPutStarted!: () => void;
    const putStarted = new Promise<void>((resolve) => { markPutStarted = resolve; });
    const delayedPut = new Promise<void>((resolve) => { releasePut = resolve; });
    const providerKv: ProviderKv & { put(key: string, value: unknown, options?: unknown): Promise<void> } = {
      async put(name, value) {
        markPutStarted();
        await delayedPut;
        values.set(name, String(value));
      },
      async delete(name) { providerDeletes.push(name); values.delete(name); },
      async list({ prefix }) {
        return { keys: [...values.keys()].filter((name) => name.startsWith(prefix)).map((name) => ({ name })), list_complete: true };
      },
    };
    const registry: ConnectorGrantRegistry = {
      async listUserGrants() { return { items: [] }; },
      async revokeGrant() { revokeCalls += 1; },
    };

    await runInDurableObject(target, async (instance: DeviceSession, state) => {
      const storage = state.storage as unknown as DeletionStorage;
      const fenced = fenceProviderKv(providerKv, (actualSubject) => {
        expect(actualSubject).toBe(subject);
        return {
          async admitProviderWrite(writeKey, generation, expiresAt) {
            return instance.admitProviderWrite(writeKey, generation, expiresAt);
          },
          async settleProviderWrite(id) { return instance.settleProviderWrite(id); },
          async closeCompensation(id, at) { return instance.closeCompensation(id, at); },
          async markProviderDelete(writeKey) { return instance.markProviderDelete(writeKey); },
        };
      }, () => now);
      const inFlightPut = fenced.put(key, JSON.stringify({ metadata: { accountGeneration: 0 } }), { expirationTtl: 60 });
      await putStarted;

      // Model an unexpected persisted shape after J1 has committed but before
      // the provider's J2 put returns. Deletion must not normalize this to an
      // empty journal and pass its open-write barrier.
      await state.storage.put(PROVIDER_WRITES_KEY, { open: null, keys: {} });
      await beginAccountDeletion(storage, subject, now);

      await expect(runAccountDeletion(storage, providerKv, registry, providerKv, now, undefined, null))
        .rejects.toBeInstanceOf(InvalidProviderWritesJournalError);
      expect(providerDeletes).toEqual([]);
      expect(revokeCalls).toBe(0);
      expect(await state.storage.get<{ status: string; step?: string }>(ACCOUNT_DELETION_KEY))
        .toMatchObject({ status: "pending", step: "provider-writes" });
      expect(values.has(key)).toBe(false);

      releasePut();
      await expect(inFlightPut).rejects.toBeInstanceOf(InvalidProviderWritesJournalError);
      expect(values.has(key)).toBe(true);
      expect(providerDeletes).toEqual([]);
      expect(revokeCalls).toBe(0);
      expect(await state.storage.get<{ status: string }>(ACCOUNT_DELETION_KEY))
        .toMatchObject({ status: "pending", step: "provider-writes" });

      // Run the production alarm handler for a due deletion. It must retain a
      // future retry even though journal validation prevents deletion work.
      const alarmNow = Date.now();
      const pending = await state.storage.get<{ status: string; retryAt: number }>(ACCOUNT_DELETION_KEY);
      await state.storage.put(ACCOUNT_DELETION_KEY, { ...pending, retryAt: alarmNow - 1 });
      // Keep Workerd from auto-dispatching before this explicit production
      // handler call; the deletion marker itself is already due.
      await state.storage.setAlarm(alarmNow + 24 * 60 * 60_000);
      await expect(instance.alarm()).rejects.toBeInstanceOf(InvalidProviderWritesJournalError);
      const afterAlarm = await state.storage.get<{ status: string; step?: string; retryAt?: number }>(ACCOUNT_DELETION_KEY);
      expect(afterAlarm).toMatchObject({ status: "pending", step: "provider-writes" });
      expect(afterAlarm?.retryAt).toBeLessThan(Date.now());
      const retryAlarm = await state.storage.getAlarm();
      expect(retryAlarm).not.toBeNull();
      expect(retryAlarm!).toBeGreaterThan(Date.now());
      expect(retryAlarm!).toBeLessThan(alarmNow + 60 * 60_000);
      expect(providerDeletes).toEqual([]);
      expect(revokeCalls).toBe(0);

      // Cloudflare consumes the alarm before dispatch, so the handler must
      // also recover when getAlarm() is null at entry.
      await state.storage.deleteAlarm();
      const nullAlarmNow = Date.now();
      await expect(instance.alarm()).rejects.toBeInstanceOf(InvalidProviderWritesJournalError);
      const afterNullAlarm = await state.storage.get<{ status: string; step?: string }>(ACCOUNT_DELETION_KEY);
      expect(afterNullAlarm).toMatchObject({ status: "pending", step: "provider-writes" });
      const nullPathRetryAlarm = await state.storage.getAlarm();
      expect(nullPathRetryAlarm).not.toBeNull();
      expect(nullPathRetryAlarm!).toBeGreaterThan(Date.now());
      expect(nullPathRetryAlarm!).toBeGreaterThan(nullAlarmNow);
      expect(providerDeletes).toEqual([]);
      expect(revokeCalls).toBe(0);
    });
  });

  it("runs due shared journal work while a pending deletion retry is still in the future", async () => {
    const target = stub();
    const now = Date.now();
    const staleKey = `token:access-runtime-smoke-${crypto.randomUUID()}:grant-1:old`;
    await runInDurableObject(target, async (instance: DeviceSession, state) => {
      const deletion = await beginAccountDeletion(state.storage, "runtime-smoke-future-delete", now);
      await state.storage.put(ACCOUNT_DELETION_KEY, { ...deletion, step: "directory", retryAt: now + 60 * 60_000 });
      await state.storage.put(PROVIDER_WRITES_KEY, { open: {}, keys: { [staleKey]: { expiresAt: now - 24 * 60 * 60_000 - 1_000 } } });
      await state.storage.put(PROVIDER_WRITES_SWEEP_KEY, now - 1);
      await state.storage.put(PROVIDER_WRITES_SWEEP_ATTEMPTS_KEY, 4);
      // Exercise a due shared alarm directly and deterministically in Workerd.
      await state.storage.setAlarm(now - 1);
      // Workerd consumes the alarm before calling alarm(); model that state so
      // production sees the same null getAlarm() value during dispatch.
      await state.storage.deleteAlarm();
      await instance.alarm();

      expect(await state.storage.get(PROVIDER_WRITES_KEY)).toBeUndefined();
      expect(await state.storage.get(PROVIDER_WRITES_SWEEP_ATTEMPTS_KEY)).toBeUndefined();
      const stillPending = await state.storage.get<{ status: string; retryAt: number }>(ACCOUNT_DELETION_KEY);
      expect(stillPending).toMatchObject({ status: "pending" });
      expect(stillPending?.retryAt).toBe(now + 60 * 60_000);
      expect(await state.storage.getAlarm()).toBe(now + 60 * 60_000);
    });
  });

  it("continues a due reconciling deletion through production DeviceSession.alarm", async () => {
    const target = stub();
    const now = Date.now();
    await runInDurableObject(target, async (instance: DeviceSession, state) => {
      await state.storage.put(ACCOUNT_DELETION_KEY, {
        version: 1, status: "reconciling", operationId: admissionId,
        startedAt: now - 60_000, accessRevokedAt: now - 30_000,
        subject: `runtime-smoke-reconcile-${crypto.randomUUID()}`,
        lastLegacyDeleteAt: now - 20 * 60_000, retryAt: now - 1, seenLegacy: {},
      });
      await state.storage.setAlarm(now - 1);
      await state.storage.deleteAlarm();
      await instance.alarm();

      const reconciling = await state.storage.get<{ status: string; retryAt: number }>(ACCOUNT_DELETION_KEY);
      expect(reconciling?.status).toBe("reconciling");
      expect(reconciling?.retryAt).toBeGreaterThan(now);
      expect(await state.storage.getAlarm()).toBe(reconciling?.retryAt);
    });
  });

  it("schedules a bounded retry for an expired tombstone through production alarm", async () => {
    const target = stub();
    const now = Date.now();
    const tombstoneAt = now - PROVIDER_0_10_4_MAX_LIFETIME_MS - 24 * 60 * 60_000;
    await runInDurableObject(target, async (_instance: DeviceSession, state) => {
      await state.storage.put(PROVIDER_WRITES_KEY, {
        open: { [admissionId]: { key: providerKey, admittedAt: tombstoneAt, state: "compensating" } },
        keys: { [providerKey]: { deleteIntent: true, deletedAt: tombstoneAt } },
      });
      await scheduleProviderWritesRetention(state.storage, now);
    });
    expect(await runDurableObjectAlarm(target)).toBe(true);
    const firstRetry = await runInDurableObject(target, (_instance: DeviceSession, state) => state.storage.getAlarm());
    expect(firstRetry).toBeGreaterThan(now);
    expect(firstRetry).toBeLessThanOrEqual(now + 24 * 60 * 60_000 + 5_000);
    await runInDurableObject(target, async (_instance: DeviceSession, state) => {
      const journal = await state.storage.get<{ open: Record<string, unknown>; keys: Record<string, unknown> }>(PROVIDER_WRITES_KEY);
      expect(journal?.open[admissionId]).toBeDefined();
      expect(journal?.keys[providerKey]).toBeDefined();
      expect(await state.storage.get(PROVIDER_WRITES_SWEEP_KEY)).toBe(firstRetry);
    });
  });
});
