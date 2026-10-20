import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import type { GatewayTransport } from "../../qc-client/src/index.ts";
import { createDemoBrokerProvider, type DemoBrokerRequest } from "../src/demo-broker-provider.ts";
import { createMemoryVirtualDeviceStorage, createVirtualGatewayTransport, VirtualDeviceStore } from "../src/virtual-gateway.ts";
import type { VirtualModelCatalogue } from "../src/virtual-device-defaults.ts";
import { createDemoDeviceSession } from "../src/demo-device-session.ts";

const target = { source: "demo" as const, sessionId: "ui-demo-1", generation: 1 };
const request = (sequence: number, method = "device.snapshot", params = {}): DemoBrokerRequest => ({ target, sequence, method, params });

test("the real UI VirtualDeviceStore observes reverse-provider mutations immediately", async () => {
  const catalogue = JSON.parse(readFileSync(new URL("../../../../docs/reference/qc-live-parameter-scales.json", import.meta.url), "utf8")) as VirtualModelCatalogue;
  const store = await VirtualDeviceStore.open({ storage: createMemoryVirtualDeviceStorage(), catalogue });
  const virtual = createVirtualGatewayTransport(store);
  const provider = createDemoBrokerProvider(virtual, target, () => true);
  await provider.invoke(request(1, "device.setDeviceName", { name: "Shared demo" }));
  assert.deepEqual(await provider.invoke(request(2, "device.identity")), await virtual.identity());
  assert.match(JSON.stringify(await virtual.identity()), /Shared demo/);
  assert.deepEqual(await provider.invoke(request(3)), await virtual.currentSnapshot());
});

test("UI physical/demo reactivation and concurrent opening retain one provider runtime", async () => {
  let opens = 0;
  let name = "original";
  const virtual = {
    reconnect: async () => ({ phase: "ready", detail: "Demo", source: "virtual", demo: false }),
    currentSnapshot: async () => ({ presetName: name }),
    setDeviceName: async (value: string) => { name = value; }
  } as unknown as GatewayTransport;
  const session = createDemoDeviceSession({} as GatewayTransport, async () => { opens += 1; return virtual; });
  const storage = createMemoryVirtualDeviceStorage();
  await Promise.all([session.start(storage), session.start(storage)]);
  const provider = createDemoBrokerProvider(session.virtual!, target, () => true);
  await provider.invoke(request(1, "device.setDeviceName", { name: "MCP edit" }));
  session.useDevice();
  const resumed = await session.start(storage);
  assert.equal(resumed?.transport, virtual);
  assert.deepEqual(resumed?.snapshot, { presetName: "MCP edit" });
  assert.equal(opens, 1);
});

test("MCP calls the same demo state and never falls back for an unsupported RPC", async () => {
  let value = 0;
  const transport = { currentSnapshot: async () => ({ value }), setDeviceName: async (name: string) => { value = name.length; } } as unknown as GatewayTransport;
  const provider = createDemoBrokerProvider(transport, target, () => true);
  await provider.invoke(request(1, "device.setDeviceName", { name: "Demo" }));
  assert.deepEqual(await transport.currentSnapshot(), { value: 4 });
  assert.deepEqual(await provider.invoke(request(2)), { value: 4 });
  await assert.rejects(provider.invoke(request(3, "broker.selectDevice")), { code: "METHOD_NOT_SUPPORTED" });
  await assert.rejects(provider.invoke(request(2)), { code: "INVALID_PROVIDER_REQUEST" });
});

test("queued writes are refused after physical selection and in-flight results discarded", async () => {
  let current = true;
  let finish!: () => void;
  let writes = 0;
  const transport = {
    currentSnapshot: () => new Promise<void>(resolve => { finish = resolve; }),
    setDeviceName: async () => { writes += 1; }
  } as unknown as GatewayTransport;
  const provider = createDemoBrokerProvider(transport, target, () => current);
  const reading = provider.invoke(request(1));
  await Promise.resolve();
  const queued = provider.invoke(request(2, "device.setDeviceName", { name: "old demo write" }));
  const readRejected = assert.rejects(reading, { code: "TARGET_SUPERSEDED" });
  const writeRejected = assert.rejects(queued, { code: "TARGET_SUPERSEDED" });
  current = false;
  finish();
  await Promise.all([readRejected, writeRejected]);
  assert.equal(writes, 0);
});

test("provider death, foreign generation and duplicate requests do not run effects", async () => {
  let calls = 0;
  const provider = createDemoBrokerProvider({ currentSnapshot: async () => ++calls } as unknown as GatewayTransport, target, () => true);
  await assert.rejects(provider.invoke({ ...request(1), target: { ...target, generation: 2 } }), { code: "TARGET_SUPERSEDED" });
  assert.equal(await provider.invoke(request(1)), 1);
  await assert.rejects(provider.invoke(request(1)), { code: "INVALID_PROVIDER_REQUEST" });
  provider.dispose();
  await assert.rejects(provider.invoke(request(2)), { code: "TARGET_SUPERSEDED" });
  assert.equal(calls, 1);
});

test("pending requests are bounded and withdrawal drains the queue without effects", async () => {
  let finish!: () => void;
  let calls = 0;
  const provider = createDemoBrokerProvider({ currentSnapshot: () => { calls += 1; return new Promise<void>(resolve => { finish = resolve; }); } } as unknown as GatewayTransport, target, () => true);
  const pending = Array.from({ length: 8 }, (_, i) => provider.invoke(request(i + 1)));
  const rejected = pending.map(promise => assert.rejects(promise, { code: "TARGET_SUPERSEDED" }));
  await assert.rejects(provider.invoke(request(9)), { code: "PROVIDER_BUSY" });
  provider.dispose();
  finish();
  await Promise.all(rejected);
  assert.equal(calls, 1);
});

test("return to the same demo store uses a fresh generation and rejects the old lease", async () => {
  let generation = 1;
  let name = "before";
  const virtual = { currentSnapshot: async () => ({ presetName: name }), setDeviceName: async (value: string) => { name = value; } } as unknown as GatewayTransport;
  const old = createDemoBrokerProvider(virtual, target, () => generation === 1);
  generation = 2; // physical selected
  await assert.rejects(old.invoke(request(1)), { code: "TARGET_SUPERSEDED" });
  generation = 3; // physical disconnected, same demo store resumes
  const resumedTarget = { ...target, generation };
  const resumed = createDemoBrokerProvider(virtual, resumedTarget, () => generation === 3);
  await resumed.invoke({ ...request(1, "device.setDeviceName", { name: "resumed" }), target: resumedTarget });
  assert.deepEqual(await virtual.currentSnapshot(), { presetName: "resumed" });
  await assert.rejects(resumed.invoke(request(2)), { code: "TARGET_SUPERSEDED" });
});

test("expected-state checks use the captured demo snapshot and refuse unsupported constraints", async () => {
  let writes = 0;
  const virtual = { currentSnapshot: async () => ({ presetName: "demo preset", presetPosition: 3 }), setDeviceName: async () => { writes += 1; } } as unknown as GatewayTransport;
  const provider = createDemoBrokerProvider(virtual, target, () => true);
  await assert.rejects(provider.invoke({ ...request(1, "device.setDeviceName", { name: "wrong" }), expectedState: { presetName: "physical preset" } }), { code: "STATE_CONFLICT" });
  await assert.rejects(provider.invoke({ ...request(2, "device.setDeviceName", { name: "unknown guard" }), expectedState: { unknownConstraint: true } }), { code: "INVALID_PROVIDER_REQUEST" });
  await provider.invoke({ ...request(3, "device.setDeviceName", { name: "matches" }), expectedState: { presetName: "demo preset", position: 3 } });
  assert.equal(writes, 1);
});
