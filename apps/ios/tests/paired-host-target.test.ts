import assert from "node:assert/strict";
import test from "node:test";
import { pairedHostConnection, readPairedHostTarget } from "../src/paired-host-target.ts";
import type { RuntimeStatus } from "@qc-remote/client";
const status = (extra: any = {}): RuntimeStatus => ({ platform: "fixture", gatewayAvailable: true, message: "fixture", usbDiagnostics: { phase: "disconnected", connected: false, synchronized: false, detail: "Physical USB absent" }, ...extra });
const demo = { source: "demo" as const, sessionId: "same-host-ui-demo", generation: 1 };

test("host demo is ready without physical USB while retaining the host gateway route", () => {
  const state = pairedHostConnection(status({ brokerTarget: demo }));
  assert.equal(state.phase, "ready"); assert.equal(state.source, "device"); assert.equal(state.demo, false);
  assert.equal(state.brokerTarget?.source, "demo"); assert.match(state.detail, /host.*demo device/);
});
test("legacy absence, syncing and API failure never imply a connected demo", () => {
  assert.equal(pairedHostConnection(status()).phase, "disconnected");
  assert.equal(pairedHostConnection(status()).brokerTarget, undefined);
  assert.equal(pairedHostConnection(status({ usbDiagnostics: { connected: true, synchronized: false, phase: "syncing" } })).phase, "syncing");
  assert.equal(pairedHostConnection(status({ gatewayAvailable: false, brokerTarget: demo })).phase, "disconnected");
  assert.equal(pairedHostConnection(status({ gatewayAvailable: false, brokerTarget: demo })).brokerTarget, undefined);
});
test("unknown or malformed host target metadata fails closed", () => {
  for (const brokerTarget of [{ ...demo, source: "virtual" }, { ...demo, generation: 0 }, { ...demo, sessionId: "" }]) assert.throws(() => pairedHostConnection(status({ brokerTarget })), /identity is unsupported/);
});
test("explicit host demo refresh reads the same selection before and after the snapshot", async () => {
  const calls: string[] = [];
  const observed = await readPairedHostTarget({ callTool: async name => { calls.push(name); return name === "get_qc_status" ? status({ brokerTarget: demo }) : { presetName: "Host demo", blocks: [] }; } });
  assert.deepEqual(calls, ["get_qc_status", "get_current_preset", "get_qc_status"]);
  assert.equal(observed.snapshot?.presetName, "Host demo"); assert.equal(observed.connection.source, "device");
});
test("demo to physical, withdrawal and return to same demo with new generation discard the read without retry", async () => {
  for (const after of [status({ brokerTarget: { source: "physical", sessionId: "usb", generation: 2 } }), status(), status({ brokerTarget: { ...demo, generation: 3 } })]) {
    const calls: string[] = [];
    await assert.rejects(readPairedHostTarget({ callTool: async name => { calls.push(name); return name === "get_current_preset" ? { presetName: "Old demo", blocks: [] } : calls.length === 1 ? status({ brokerTarget: demo }) : after; } }), /target changed during refresh/);
    assert.equal(calls.length, 3);
  }
});
test("unavailable hosts perform one read and retain unavailable state instead of loading a local demo", async () => {
  const calls: string[] = [];
  const result = await readPairedHostTarget({ callTool: async name => { calls.push(name); return status(); } });
  assert.deepEqual(calls, ["get_qc_status"]); assert.equal(result.snapshot, undefined); assert.equal(result.connection.phase, "disconnected");
});
test("legacy physical reads remain finite and malformed snapshots are rejected", async () => {
  const live = status({ usbDiagnostics: { connected: true, synchronized: true, phase: "ready" } });
  const calls: string[] = [];
  const result = await readPairedHostTarget({ callTool: async name => { calls.push(name); return name === "get_qc_status" ? live : { presetName: "QC", blocks: [] }; } });
  assert.equal(result.snapshot?.presetName, "QC"); assert.equal(calls.length, 2);
  await assert.rejects(readPairedHostTarget({ callTool: async name => name === "get_qc_status" ? live : {} }), /malformed PresetSnapshot/);
});


test("explicit targetReady overrides stale physical diagnostics and requires target identity", () => {
  const usb = { connected: true, synchronized: true, phase: "ready" };
  assert.equal(pairedHostConnection(status({ targetReady: false, brokerTarget: demo, usbDiagnostics: usb })).phase, "disconnected");
  assert.equal(pairedHostConnection(status({ targetReady: false, brokerTarget: demo, usbDiagnostics: usb })).brokerTarget, undefined);
  assert.throws(() => pairedHostConnection(status({ targetReady: true })), /has no identity/);
  assert.equal(pairedHostConnection(status({ targetReady: true, brokerTarget: demo })).phase, "ready");
});
