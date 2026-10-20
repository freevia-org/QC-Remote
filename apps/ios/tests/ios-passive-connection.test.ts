import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";

/** Execute the actual iOS service with a synthetic SDK/Keychain. No cloud,
 * browser, simulator, saved profile, or installed application is accessed. */
async function loadService(t: any) {
  const state = { calls: [] as string[], failConnect: true, client: undefined as any,
    saved: JSON.stringify({ tokens: { access_token: "fixture-only", token_type: "bearer" } }),
    appEvents: new Map<string, any>(), documentEvents: new Map<string, any>() };
  const replaceGlobal = (key: string, value: unknown) => {
    const original = Object.getOwnPropertyDescriptor(globalThis, key);
    Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
    t.after(() => { if (original) Object.defineProperty(globalThis, key, original); else Reflect.deleteProperty(globalThis, key); });
  };
  replaceGlobal("__qcIosTest", state);
  const storage = new Map([["qc.remote.ios.connection-mode-v1", "remote"]]);
  replaceGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), removeItem: (key: string) => storage.delete(key) });
  replaceGlobal("document", { visibilityState: "visible", addEventListener: (key: string, callback: any) => state.documentEvents.set(key, callback) });
  const mocks: Record<string, string> = {
    "@capacitor/app": `export const App={addListener:async (name,fn)=>{globalThis.__qcIosTest.appEvents.set(name,fn);return {remove:async()=>{}}}};`,
    "@capacitor/browser": `export const Browser={open:async()=>{},close:async()=>{}};`,
    "@capacitor/core": `export const Capacitor={isNativePlatform:()=>true};export const registerPlugin=()=>({});`,
    "@modelcontextprotocol/sdk/client/index.js": `export class Client { constructor(){globalThis.__qcIosTest.client=this;} async connect(){const s=globalThis.__qcIosTest;s.calls.push('initialize');if(s.failConnect)throw Error('HTTP 503');}async callTool({name}){const s=globalThis.__qcIosTest;s.calls.push(name);return name==='get_current_preset'?{presetName:'fixture',blocks:[]}:{deviceName:'fixture',usbDiagnostics:{connected:true,synchronized:true}};}async close(){this.onclose?.();}}`,
    "@modelcontextprotocol/sdk/client/streamableHttp.js": `export class StreamableHTTPClientTransport {constructor(url,options){if(options.reconnectionOptions.maxRetries!==0)throw Error('SDK reconnect must be disabled');}async finishAuth(){}}`,
    "@modelcontextprotocol/sdk/client/auth.js": `export class UnauthorizedError extends Error {} export const auth=async()=>"AUTHORIZED";`,
    "@qc-remote/client": `export const brokerTargetKey=x=>x?JSON.stringify([x.source,x.sessionId,x.generation]):undefined;export const validateGatewayResult=(method,x)=>x;export const GATEWAY_CLIENT_METHODS={};export const withDeviceConnectionSource=x=>x;export const createGatewayClientTransport=fn=>({invoke:fn});`,
    "@qc-remote/core": `export const parseControlAccessMode=()=>"read-only";export const controlAccessPermitsAction=()=>true;export const SHARED_QC_ACTIONS=[];export const createQcGatewayTransport=x=>x;export const parseAppMcpResult=x=>x;export const invokeAppMcp=(method,args,opts)=>opts.callTool(method==='system.status'?'get_qc_status':'get_current_preset',args);`,
    "@qc-remote/theme": `export const QC_BRAND={publicRelayEndpoint:'https://example.test'};`,
    "../native-bridge": `export const QCIosNativeBridge={secureGet:async()=>({value:globalThis.__qcIosTest.saved}),secureSet:async({value})=>{globalThis.__qcIosTest.saved=value;},addListener:async()=>({remove:async()=>{}}),setScreenAwake:async()=>{},platformCapabilities:async()=>({deviceFamily:'iphone'})};`,
    "./workflow-confirmations": `export const confirmIosWorkflow=async()=>true;`,
  };
  const result = await build({ entryPoints: [fileURLToPath(new URL("../src/ios-services.ts", import.meta.url))], bundle: true, write: false, platform: "node", format: "esm", plugins: [{ name: "synthetic-native-and-sdk", setup(build) {
    build.onResolve({ filter: /.*/ }, args => mocks[args.path] ? { path: args.path, namespace: "mock" } : undefined);
    build.onLoad({ filter: /.*/, namespace: "mock" }, args => ({ contents: mocks[args.path], loader: "js" }));
  } }] });
  const service = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString("base64")}#${Math.random()}`);
  return { state, service };
}

test("failed connection, passive scans, background and resume never retry MCP; explicit demand reconnects", async t => {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval", "Date"] });
  const { state, service } = await loadService(t);
  await assert.rejects(service.publicRelay.start(), /503/);
  assert.deepEqual(state.calls, ["initialize"]);
  for (let i = 0; i < 100; i++) { await service.QcUsbNative.scan(); await service.publicRelay.status(); }
  t.mock.timers.tick(86_400_000);
  assert.deepEqual(state.calls, ["initialize"]);
  state.appEvents.get("appStateChange")({ isActive: false });
  state.appEvents.get("appStateChange")({ isActive: true });
  t.mock.timers.tick(86_400_000);
  assert.deepEqual(state.calls, ["initialize"], "foreground resume must wait for demand");
  assert.equal((await service.publicRelay.status()).paired, true, "network failure preserves saved sign-in");
  state.failConnect = false;
  await service.QcUsbNative.connect();
  assert.deepEqual(state.calls, ["initialize", "initialize", "get_qc_status"]);
  await service.QcUsbNative.gatewayInvoke({ method: "system.status" });
  await service.QcUsbNative.gatewayInvoke({ method: "system.status" });
  assert.deepEqual(state.calls.slice(-2), ["get_qc_status", "get_qc_status"], "ordinary guard reads are fresh");
  state.client.onclose();
  for (let i = 0; i < 100; i++) await service.QcUsbNative.scan();
  const beforeIdle = state.calls.length;
  t.mock.timers.tick(86_400_000);
  assert.equal(state.calls.length, beforeIdle, "a disconnected session must not restart itself");
  await service.refreshRemoteObservation();
  assert.deepEqual(state.calls.slice(-4), ["initialize", "get_qc_status", "get_qc_status", "get_current_preset"]);
  const beforeBackground = state.calls.length;
  state.appEvents.get("appStateChange")({ isActive: false });
  t.mock.timers.tick(86_400_000);
  state.appEvents.get("appStateChange")({ isActive: true });
  t.mock.timers.tick(86_400_000);
  assert.equal(state.calls.length, beforeBackground);
  assert.equal((await service.publicRelay.status()).paired, true);
  await service.publicRelay.unpair();
  assert.equal((await service.publicRelay.status()).paired, false, "only explicit sign-out clears saved credentials");
});
