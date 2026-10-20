import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";
import type { GatewayTransport } from "@qc-remote/client";
import type { NativeDemoRequest } from "./demo-provider-bridge.ts";

// Bundle the real shared dispatcher; avoid importing the unrelated React UI barrel in Node.
const compiled = await build({ entryPoints: [fileURLToPath(new URL("./demo-provider-bridge.ts", import.meta.url))], bundle: true, write: false, format: "esm", platform: "node", plugins: [{ name: "shared-dispatcher", setup(build) {
  build.onResolve({ filter: /^@qc-remote\/ui$/ }, () => ({ path: fileURLToPath(new URL("../../../packages/typescript/qc-ui/src/demo-broker-provider.ts", import.meta.url)) }));
} }] });
const { createNativeDemoProviderBridge } = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`) as typeof import("./demo-provider-bridge.ts");
function fixture() {
  const publications: any[] = [], replies: any[] = [];
  const port = { publishDemoProvider: async (registration: any) => { publications.push(registration); }, completeDemoRequest: async (reply: any) => { replies.push(reply); } };
  return { publications, replies, port };
}
const request = (overrides: Partial<NativeDemoRequest> = {}): NativeDemoRequest => ({ id: "native-id", sessionId: "ui-demo-session", generation: 1, sequence: 1, method: "device.snapshot", params: {}, ...overrides });
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
  const f = fixture(), gate = deferred(); let effects = 0, refreshes = 0;
  const gateway = { currentSnapshot: async () => { effects++; return gate.promise; } } as unknown as GatewayTransport;
  const bridge = createNativeDemoProviderBridge(f.port, async () => { refreshes++; });
  await bridge.bind({ sessionId: "ui-demo-session", gateway, current: () => true });
  const first = bridge.handle(request()), second = bridge.handle(request({ id: "queued", sequence: 2 }));
  await Promise.resolve(); await Promise.resolve();
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
