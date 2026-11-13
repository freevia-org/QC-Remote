import test from "node:test";
import assert from "node:assert/strict";
import { readBoundedText } from "../src/http-security.ts";
import { normalizeAppleCallback, readAppleNotification } from "../src/apple-auth.ts";
import { handleGoogleAuthorization } from "../src/access-auth.ts";

/** A body that counts how many bytes were pulled from it. */
function countingBody(chunkBytes: number, chunks: number) {
  const state = { pulled: 0, cancelled: false };
  let sent = 0;
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      if (sent >= chunks) { controller.close(); return; }
      sent += 1;
      state.pulled += chunkBytes;
      controller.enqueue(new Uint8Array(chunkBytes).fill(97));
    },
    cancel() { state.cancelled = true; },
  });
  return { state, body };
}
const streamed = (url: string, body: ReadableStream<Uint8Array>, headers: Record<string, string> = {}) =>
  new Request(url, { method: "POST", headers, body, duplex: "half" } as RequestInit);

test("readBoundedText refuses a declared oversize body without reading it", async () => {
  const { state, body } = countingBody(1024, 100);
  const request = streamed("https://relay.test/x", body, { "content-length": "20000" });
  assert.equal(await readBoundedText(request, 16_384), null);
  // The stream primes one chunk on construction; nothing is read after that.
  assert.ok(state.pulled <= 1024, `pulled ${state.pulled}`);
});

test("readBoundedText refuses a non-numeric content-length", async () => {
  const request = new Request("https://relay.test/x", { method: "POST", headers: { "content-length": "12abc" }, body: "x" });
  assert.equal(await readBoundedText(request, 16_384), null);
});

test("readBoundedText stops pulling shortly after the cap when nothing is declared", async () => {
  const { state, body } = countingBody(1024, 5_000);
  assert.equal(await readBoundedText(streamed("https://relay.test/x", body), 16_384), null);
  // Cap plus the chunks the stream's own queue reads ahead; never the whole body.
  assert.ok(state.pulled <= 16_384 + 8 * 1024, `pulled ${state.pulled}`);
  assert.ok(state.pulled < 5_000 * 1024);
});

test("readBoundedText returns a body at exactly the cap and an empty body", async () => {
  assert.equal((await readBoundedText(new Request("https://relay.test/x", { method: "POST", body: "a".repeat(16_384) }), 16_384))?.length, 16_384);
  assert.equal(await readBoundedText(new Request("https://relay.test/x", { method: "POST", body: "a".repeat(16_385) }), 16_384), null);
  assert.equal(await readBoundedText(new Request("https://relay.test/x"), 16_384), "");
});

test("Apple callback and notification bodies are bounded while streaming", async () => {
  const { state, body } = countingBody(1024, 5_000);
  const request = streamed("https://relay.test/auth/apple/pair/callback", body, { "content-type": "application/x-www-form-urlencoded" });
  await assert.rejects(normalizeAppleCallback(request), /Invalid Apple callback size/);
  assert.ok(state.pulled < 5_000 * 1024);
  const declared = new Request("https://relay.test/auth/apple/notifications", { method: "POST", headers: { "content-length": "20000" }, body: "{}" });
  await assert.rejects(readAppleNotification(declared, {}), /Invalid Apple notification size/);
});

/** A KV that counts every call and holds only what a test puts in it. */
function countingKv(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  const calls: string[] = [];
  return {
    calls,
    get: async (key: string) => { calls.push(`get ${key}`); return values.has(key) ? JSON.parse(values.get(key)!) : null; },
    put: async (key: string, value: string) => { calls.push(`put ${key}`); values.set(key, value); },
    delete: async (key: string) => { calls.push(`delete ${key}`); values.delete(key); },
  };
}
const authorizePost = (nonce: string, withCookie = true) => new Request("https://relay.test/authorize", {
  method: "POST",
  headers: { "content-type": "application/x-www-form-urlencoded", ...(withCookie ? { cookie: `__Host-qcrc-csrf-${nonce}=${nonce}` } : {}) },
  body: new URLSearchParams({ nonce }),
});
const env = (kv: ReturnType<typeof countingKv>) => ({ QCRC_OAUTH: kv, COOKIE_ENCRYPTION_KEY: "test-only-cookie-secret", GOOGLE_CLIENT_ID: "g", GOOGLE_CLIENT_SECRET: "s", OAUTH_PROVIDER: {} }) as never;

test("POST /authorize refuses a malformed or overlong nonce before any KV call", async () => {
  for (const nonce of ["x", "a".repeat(600), "has space and/slashes", "a".repeat(31), "a".repeat(33)]) {
    const kv = countingKv();
    const response = await handleGoogleAuthorization(authorizePost(nonce), env(kv));
    assert.equal(response?.status, 400, nonce);
    assert.deepEqual(kv.calls, [], nonce);
  }
});

test("POST /authorize with an unknown well-formed nonce reads once and deletes nothing", async () => {
  const nonce = "A".repeat(32);
  const kv = countingKv();
  const response = await handleGoogleAuthorization(authorizePost(nonce), env(kv));
  assert.equal(response?.status, 400);
  assert.deepEqual(kv.calls, [`get qcrc:approval:${nonce}`]);
});
