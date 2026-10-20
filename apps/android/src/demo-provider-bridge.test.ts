import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";
import type { GatewayTransport } from "@qc-remote/client";
import { readFileSync } from "node:fs";
import type { NativeDemoPort, NativeDemoRequest } from "./demo-provider-bridge.ts";

// Bundle the real shared dispatcher; avoid importing the unrelated React UI barrel in Node.
const compiled = await build({ stdin: { contents: `export * from ${JSON.stringify(fileURLToPath(new URL("./demo-provider-bridge.ts", import.meta.url)))}; export { VirtualDeviceStore, createVirtualGatewayTransport, createMemoryVirtualDeviceStorage } from ${JSON.stringify(fileURLToPath(new URL("../../../packages/typescript/qc-ui/src/virtual-gateway.ts", import.meta.url)))};`, resolveDir: fileURLToPath(new URL(".", import.meta.url)), loader: "ts" }, bundle: true, write: false, format: "esm", platform: "node", plugins: [{ name: "shared-dispatcher", setup(build) {
  build.onResolve({ filter: /^@qc-remote\/ui$/ }, () => ({ path: fileURLToPath(new URL("../../../packages/typescript/qc-ui/src/demo-broker-provider.ts", import.meta.url)) }));
} }] });
const { createNativeDemoProviderBridge, attachNativeDemoProvider, VirtualDeviceStore, createVirtualGatewayTransport, createMemoryVirtualDeviceStorage } = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`) as typeof import("./demo-provider-bridge.ts") & Pick<typeof import("@qc-remote/ui"), "VirtualDeviceStore" | "createVirtualGatewayTransport" | "createMemoryVirtualDeviceStorage">;
function fixture() {
  const publications: any[] = [], replies: any[] = [];
  const port: NativeDemoPort = { admitDemoRequest: async () => ({ admitted: true }), publishDemoProvider: async (registration: any) => { publications.push(registration); }, completeDemoRequest: async (reply: any) => { replies.push(reply); } };
  return { publications, replies, port };
}
const request = (overrides: Partial<NativeDemoRequest> = {}): NativeDemoRequest => ({ id: "native-id", sessionId: "ui-demo-session", generation: 1, sequence: 1, method: "device.snapshot", params: {}, brokerTarget: { source: "demo", sessionId: "native-issued-target", generation: 1 }, ...overrides });
const deferred = () => { let resolve!: (value: any) => void; const promise = new Promise<any>(r => { resolve = r; }); return { promise, resolve }; };

test("native requests use the exact existing demo transport and update that same view", async () => {
  const f = fixture(), calls: string[] = [];
  const gateway = { currentSnapshot: async () => { calls.push("same demo"); return { presetName: "UI demo" }; } } as unknown as GatewayTransport;
  const bridge = createNativeDemoProviderBridge(f.port, async (transport, current) => { assert.equal(transport, gateway); assert.equal(current(), true); calls.push("view refreshed"); });
  await bridge.bind({ sessionId: "ui-demo-session", gateway, current: () => true });
  assert.deepEqual(f.publications, [{ sessionId: "ui-demo-session", generation: 1 }]);
  assert.equal(await bridge.handle(request()), "completed");
  assert.deepEqual(calls, ["same demo", "view refreshed"]);
  assert.deepEqual(f.replies[0].result, { presetName: "UI demo" });
});

test("queued and in-flight old demo work is invalidated without replay onto hardware", async () => {
  const f = fixture(), gate = deferred(), started = deferred(); let effects = 0, refreshes = 0;
  const gateway = { currentSnapshot: async () => { effects++; started.resolve(undefined); return gate.promise; } } as unknown as GatewayTransport;
  const bridge = createNativeDemoProviderBridge(f.port, async () => { refreshes++; });
  await bridge.bind({ sessionId: "ui-demo-session", gateway, current: () => true });
  const first = bridge.handle(request()), second = bridge.handle(request({ id: "queued", sequence: 2 }));
  await started.promise;
  assert.equal(effects, 1);
  await bridge.bind(undefined); // Physical selection withdraws the demo registration.
  gate.resolve({ presetName: "old demo" });
  assert.equal(await first, "discarded"); assert.equal(await second, "discarded");
  assert.equal(effects, 1); assert.equal(refreshes, 0);
  assert.ok(f.replies.every(reply => reply.error?.code === "TARGET_SUPERSEDED" && !("result" in reply)));
});

test("returning to the same demo requires a fresh registration generation", async () => {
  const f = fixture(); let effects = 0;
  const gateway = { currentSnapshot: async () => { effects++; return {}; } } as unknown as GatewayTransport;
  const bridge = createNativeDemoProviderBridge(f.port);
  const session = { sessionId: "ui-demo-session", gateway, current: () => true };
  await bridge.bind(session); await bridge.bind(undefined); await bridge.bind(session);
  assert.equal(await bridge.handle(request()), "discarded");
  assert.equal(effects, 0);
  assert.equal(await bridge.handle(request({ generation: 3 })), "completed");
  assert.equal(effects, 1);
});

test("foreign, duplicate and unsupported requests cannot call another gateway method", async () => {
  const f = fixture(); let effects = 0;
  const gateway = { currentSnapshot: async () => { effects++; return {}; } } as unknown as GatewayTransport;
  const bridge = createNativeDemoProviderBridge(f.port);
  await bridge.bind({ sessionId: "ui-demo-session", gateway, current: () => true });
  assert.equal(await bridge.handle(request({ sessionId: "foreign" })), "discarded");
  assert.equal(await bridge.handle(request()), "completed");
  assert.equal(await bridge.handle(request()), "discarded");
  assert.equal(await bridge.handle(request({ sequence: 2, method: "arbitrary.command" })), "discarded");
  assert.equal(effects, 1);
});

test("native completion rejection and view refresh failure never replay or complete twice", async () => {
  const f = fixture(); let effects = 0, refreshes = 0;
  const gateway = { currentSnapshot: async () => { effects++; return {}; } } as unknown as GatewayTransport;
  f.port.completeDemoRequest = async reply => { f.replies.push(reply); throw Error("deadline expired"); };
  const bridge = createNativeDemoProviderBridge(f.port, async () => { refreshes++; throw Error("view replaced"); });
  await bridge.bind({ sessionId: "ui-demo-session", gateway, current: () => true });
  assert.equal(await bridge.handle(request()), "discarded");
  assert.equal(effects, 1); assert.equal(refreshes, 0); assert.equal(f.replies.length, 1);
  f.port.completeDemoRequest = async reply => { f.replies.push(reply); };
  assert.equal(await bridge.handle(request({ sequence: 2 })), "completed");
  assert.equal(effects, 2); assert.equal(refreshes, 1); assert.equal(f.replies.length, 2);
});

test("provider registration failure and explicit foreground teardown fail closed", async () => {
  const f = fixture(); let effects = 0;
  const gateway = { currentSnapshot: async () => { effects++; return {}; } } as unknown as GatewayTransport;
  f.port.publishDemoProvider = async () => { throw Error("native refused registration"); };
  const bridge = createNativeDemoProviderBridge(f.port);
  await assert.rejects(bridge.bind({ sessionId: "ui-demo-session", gateway, current: () => true }), /native refused/);
  assert.equal(await bridge.handle(request()), "discarded"); assert.equal(effects, 0);
  f.port.publishDemoProvider = async registration => { f.publications.push(registration); };
  await bridge.bind({ sessionId: "ui-demo-session", gateway, current: () => true });
  await bridge.dispose();
  assert.equal(await bridge.handle(request({ generation: 2 })), "discarded");
  assert.equal(effects, 0); assert.equal(f.publications.at(-1).sessionId, null);
});


test("foreground binding installs the listener before publishing and resumes with a fresh registration", async () => {
  const f = fixture(), order: string[] = []; let visible = true, changed!: () => void, serial = 0, handler: any, removed = 0;
  const port = { ...f.port, addListener: async (_name: string, listener: any) => { order.push("listener"); handler = listener; return { remove: async () => { removed++; } }; }, publishDemoProvider: async (value: any) => { order.push("publish"); f.publications.push(value); } };
  const gateway = { currentSnapshot: async () => ({ presetName: "same live UI" }) } as unknown as GatewayTransport;
  const owner = attachNativeDemoProvider({ port, gateway, current: () => true, sessionId: () => `registration-${++serial}`, visible: () => visible, subscribeVisibility: callback => { changed = callback; return () => { order.push("unsubscribe"); }; } });
  await owner.ready;
  assert.deepEqual(order.slice(0, 2), ["listener", "publish"]);
  visible = false; changed(); await Promise.resolve();
  assert.equal(f.publications.at(-1).sessionId, null);
  visible = true; changed(); await Promise.resolve();
  assert.equal(f.publications.at(-1).sessionId, "registration-2");
  assert.equal(f.publications.at(-1).generation, 3);
  await owner.dispose();
  assert.equal(removed, 1); assert.equal(f.publications.at(-1).sessionId, null);
  handler(request()); await Promise.resolve();
  assert.equal(f.replies.length, 0);
});

test("unmount before listener registration completes never publishes a live provider", async () => {
  const f = fixture(), gate = deferred(); let removed = 0, unsubscribed = 0;
  const port = { ...f.port, addListener: async () => gate.promise };
  const owner = attachNativeDemoProvider({ port, gateway: {} as GatewayTransport, current: () => true, sessionId: () => "retired", visible: () => true, subscribeVisibility: () => () => { unsubscribed++; } });
  const retired = owner.dispose();
  gate.resolve({ remove: async () => { removed++; } });
  await retired; await owner.ready;
  assert.equal(f.publications.filter(value => value.sessionId !== null).length, 0);
  assert.equal(unsubscribed, 1);
  assert.equal(removed, 1);
});

test("shared dispatcher validates native expected-state constraints before an effect", async () => {
  const f = fixture(); let effects = 0;
  const gateway = { currentSnapshot: async () => ({ presetName: "UI preset", presetPosition: 1 }), setMasterVolume: async () => { effects++; return {}; } } as unknown as GatewayTransport;
  const bridge = createNativeDemoProviderBridge(f.port);
  await bridge.bind({ sessionId: "ui-demo-session", gateway, current: () => true });
  assert.equal(await bridge.handle(request({ method: "device.setMasterVolume", params: { value: 20 }, expectedState: { presetName: "another preset" } })), "discarded");
  assert.equal(effects, 0); assert.equal(f.replies[0].error.code, "STATE_CONFLICT");
});


test("malformed expected-state constraints are forwarded and refused, never silently dropped", async () => {
  const f = fixture(); let effects = 0;
  const gateway = { currentSnapshot: async () => ({}), setMasterVolume: async () => { effects++; return {}; } } as unknown as GatewayTransport;
  const bridge = createNativeDemoProviderBridge(f.port);
  await bridge.bind({ sessionId: "ui-demo-session", gateway, current: () => true });
  await bridge.handle(request({ method: "device.setMasterVolume", params: { value: 20 }, expectedState: null as any }));
  assert.equal(effects, 0); assert.equal(f.replies[0].error.code, "INVALID_PROVIDER_REQUEST");
});


test("the mobile wrapper rechecks native admission inside the same real store queue after physical hotplug", async () => {
  const catalogue = JSON.parse(readFileSync(new URL("../../../docs/reference/qc-live-parameter-scales.json", import.meta.url), "utf8"));
  const memory = createMemoryVirtualDeviceStorage();
  let block = false, release!: () => void, writing!: () => void;
  const writingStarted = new Promise<void>(resolve => { writing = resolve; });
  const store = await VirtualDeviceStore.open({ catalogue, storage: { read: () => memory.read(), write: async value => {
    if (block) { block = false; writing(); await new Promise<void>(resolve => { release = resolve; }); }
    await memory.write(value);
  } } });
  const virtual = createVirtualGatewayTransport(store);
  block = true;
  const localFirst = virtual.setDeviceName("human first");
  await writingStarted;
  let queued!: () => void;
  const effectQueued = new Promise<void>(resolve => { queued = resolve; });
  const update = store.update.bind(store);
  store.update = (mutate, admission) => { const result = update(mutate, admission); if (admission) queued(); return result; };
  const f = fixture(); let nativeCurrent = true, admissions = 0;
  f.port.admitDemoRequest = async envelope => {
    admissions++;
    assert.equal(envelope.sessionId, "ui-demo-session");
    assert.equal(envelope.brokerTarget?.sessionId, "native-issued-target");
    return { admitted: nativeCurrent };
  };
  const bridge = createNativeDemoProviderBridge(f.port);
  await bridge.bind({ sessionId: "ui-demo-session", gateway: virtual, current: () => true }); // JS status deliberately remains old.
  const mcp = bridge.handle(request({ method: "device.setDeviceName", params: { name: "stale remote write" } }));
  await effectQueued;
  nativeCurrent = false;
  const localAfter = virtual.setDeviceName("human after physical hotplug");
  release();
  await Promise.all([localFirst, localAfter]);
  assert.equal(await mcp, "discarded");
  assert.ok(admissions >= 2, "admission must run again after the blocked UI storage operation");
  assert.match(JSON.stringify(await virtual.identity()), /human after physical hotplug/);
  assert.doesNotMatch(memory.text!, /stale remote write/);
  assert.equal(f.replies[0].error.code, "TARGET_SUPERSEDED");
});

test("missing native broker identity and admission failure cannot mutate the demo", async () => {
  const f = fixture(); let effects = 0;
  const gateway = { setDeviceName: async () => { effects++; return {}; } } as unknown as GatewayTransport;
  const bridge = createNativeDemoProviderBridge(f.port);
  await bridge.bind({ sessionId: "ui-demo-session", gateway, current: () => true });
  assert.equal(await bridge.handle(request({ method: "device.setDeviceName", params: { name: "invalid" }, brokerTarget: undefined })), "discarded");
  f.port.admitDemoRequest = async () => { throw Error("native bridge unavailable"); };
  assert.equal(await bridge.handle(request({ sequence: 2, method: "device.setDeviceName", params: { name: "not applied" } })), "discarded");
  assert.equal(effects, 0);
});
