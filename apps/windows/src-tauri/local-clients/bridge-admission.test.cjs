"use strict";
const assert = require("node:assert/strict");
const test = require("node:test");
const { createBridgeEnvelope, refusalReplies } = require("./bridge.cjs");

const envelope = (options = {}) => createBridgeEnvelope({
  input: { destroyed: false },
  output: { destroyed: false, writableEnded: false, write() { return true; }, once() {}, on() {} },
  ...options,
});
const toolCall = (id, padding = "") => ({ jsonrpc: "2.0", id, method: "tools/call", params: { name: "x", arguments: { padding } } });
const quiet = { write() {} };

test("an oversize request is refused as not retryable, never as busy", async () => {
  const bridge = envelope({ requestBytes: 1024 });
  const refusal = bridge.admit(toolCall(1, "x".repeat(2048)));
  assert.equal(refusal.refused, "oversize");
  const sent = [];
  refusalReplies(async (value) => { sent.push(value); }, quiet, { queued: 0, chain: Promise.resolve() })(
    toolCall(1), refusal, true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(sent[0].error.data.retryable, false);
  assert.match(sent[0].error.message, /larger than/);
});

test("a full ordinary queue is busy and retryable; admitted calls keep their slots", () => {
  const bridge = envelope();
  const held = [];
  for (let id = 0; id < 8; id++) {
    const release = bridge.admit(toolCall(id));
    assert.equal(typeof release, "function");
    held.push(release);
  }
  assert.deepEqual(bridge.admit(toolCall(9)), { refused: "busy" });
  for (const release of held) release();
  assert.equal(typeof bridge.admit(toolCall(10)), "function");
});

test("a batch of refusals is written one at a time and never tears the bridge down", async () => {
  let inFlightWrites = 0;
  let peak = 0;
  const sent = [];
  const state = { queued: 0, chain: Promise.resolve() };
  const reply = refusalReplies(async (value) => {
    peak = Math.max(peak, ++inFlightWrites);
    await new Promise((resolve) => setImmediate(resolve));
    inFlightWrites--;
    sent.push(value);
  }, quiet, state);
  for (let id = 0; id < 30; id++) reply(toolCall(id), { refused: "busy" }, true);
  await state.chain;
  assert.equal(peak, 1, "stdout headroom is never asked for more than one refusal");
  assert.equal(sent.filter((value) => value.error.data.retryable).length, sent.length);
  assert.equal(sent.length, 30);
});

test("refusals beyond the queue cap and unanswerable messages are dropped with a diagnostic", async () => {
  const notes = [];
  const state = { queued: 0, chain: Promise.resolve() };
  const reply = refusalReplies(async () => { await new Promise((resolve) => setImmediate(resolve)); },
    { write: (text) => notes.push(text) }, state);
  reply({ jsonrpc: "2.0", method: "notifications/roots/list_changed" }, { refused: "busy" }, false);
  assert.equal(notes.length, 1);
  for (let id = 0; id < 200; id++) reply(toolCall(id), { refused: "busy" }, true);
  assert.ok(notes.length > 1, "overflow is reported, not fatal");
  await state.chain;
});
