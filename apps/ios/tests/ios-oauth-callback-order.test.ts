import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";

/** Execute the actual iOS service with a synthetic SDK/Keychain. No cloud,
 * browser, simulator, saved profile, or installed application is accessed. */
async function loadService(t: any) {
  const state = { calls: [] as string[], failConnect: true, client: undefined as any,
    saved: JSON.stringify({ tokens: { access_token: "fixture-only", token_type: "bearer" }, state: "fixture-state", verifier: "fixture-verifier" }),
    deviceFamily: "iphone", failExchange: false,
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
    "@modelcontextprotocol/sdk/client/auth.js": `export class UnauthorizedError extends Error {} export const auth=async(provider,options)=>{const s=globalThis.__qcIosTest;s.calls.push('exchange');if(await provider.codeVerifier()!=='fixture-verifier')throw Error('missing verifier');const response=await options.fetchFn('https://example.test/token',{method:'POST'});if(!response.ok)throw Error('code exchange failed 503');await provider.saveTokens({access_token:'exchanged-fixture',token_type:'bearer'});return 'AUTHORIZED';};`,
    "@qc-remote/client": `export const selectedBrokerTarget=x=>x.targetReady===false?undefined:x.brokerTarget;export const brokerTargetKey=x=>x?JSON.stringify([x.source,x.sessionId,x.generation]):undefined;export const validateGatewayResult=(method,x)=>x;export const GATEWAY_CLIENT_METHODS={};export const withDeviceConnectionSource=x=>x;export const createGatewayClientTransport=fn=>({invoke:fn});`,
    "@qc-remote/core": `export const parseControlAccessMode=()=>"read-only";export const controlAccessPermitsAction=()=>true;export const SHARED_QC_ACTIONS=[];export const createQcGatewayTransport=x=>x;export const parseAppMcpResult=x=>x;export const invokeAppMcp=(method,args,opts)=>opts.callTool(method==='system.status'?'get_qc_status':'get_current_preset',args);`,
    "@qc-remote/theme": `export const QC_BRAND={publicRelayEndpoint:'https://example.test'};`,
    "../native-bridge": `export const QCIosNativeBridge={secureGet:async()=>({value:globalThis.__qcIosTest.saved}),secureSet:async({value})=>{globalThis.__qcIosTest.saved=value;},addListener:async()=>({remove:async()=>{}}),setScreenAwake:async()=>{},platformCapabilities:async()=>({deviceFamily:globalThis.__qcIosTest.deviceFamily}),scanUsbDriver:async()=>({available:true,connected:false}),connectUsbDriver:async()=>({connected:true,synchronized:true,name:'fixture USB',deviceId:1})};`,
    "./workflow-confirmations": `export const confirmIosWorkflow=async()=>true;`,
  };
  const result = await build({ entryPoints: [process.env.QC_IOS_SERVICE_UNDER_TEST ?? fileURLToPath(new URL("../src/ios-services.ts", import.meta.url))], bundle: true, write: false, platform: "node", format: "esm", plugins: [{ name: "synthetic-native-and-sdk", setup(build) {
    build.onResolve({ filter: /^\.\/remote-traffic$/ }, () => ({ path: process.env.QC_IOS_REMOTE_TRAFFIC_UNDER_TEST ?? fileURLToPath(new URL("../src/remote-traffic.ts", import.meta.url)) }));
    build.onResolve({ filter: /.*/ }, args => mocks[args.path] ? { path: args.path, namespace: "mock" } : undefined);
    build.onLoad({ filter: /.*/, namespace: "mock" }, args => ({ contents: mocks[args.path], loader: "js" }));
  } }] });
  t.mock.method(globalThis, "fetch", async () => { state.calls.push("token-http"); return new Response("", { status: state.failExchange ? 503 : 200 }); });
  t.mock.method(console, "error", () => undefined);
  const service = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString("base64")}#${Math.random()}`);
  return { state, service, storage };
}

const callbackUrl = "qcremote://oauth/callback?code=fixture-code&state=fixture-state";
async function settle() { for (let i = 0; i < 40; i++) await Promise.resolve(); }

for (const callbackFirst of [true, false]) {
  for (const visibilityFirst of [true, false]) {
    test(`OAuth callback ${callbackFirst ? "before" : "after"} foreground; ${visibilityFirst ? "visibility" : "active"} event first`, async t => {
      t.mock.timers.enable({ apis: ["setTimeout", "setInterval", "Date"] });
      const { state, service } = await loadService(t);
      state.failConnect = false;
      await service.publicRelay.start();
      state.appEvents.get("appStateChange")({ isActive: false });
      (globalThis as any).document.visibilityState = "hidden";
      state.documentEvents.get("visibilitychange")();
      state.calls.length = 0;
      const send = () => state.appEvents.get("appUrlOpen")({ url: callbackUrl });
      if (callbackFirst) { send(); send(); await settle(); assert.deepEqual(state.calls, []); }
      if (visibilityFirst) {
        (globalThis as any).document.visibilityState = "visible";
        state.documentEvents.get("visibilitychange")();
        await settle(); assert.deepEqual(state.calls, []);
        state.appEvents.get("appStateChange")({ isActive: true });
      } else {
        state.appEvents.get("appStateChange")({ isActive: true });
        await settle(); assert.deepEqual(state.calls, []);
        (globalThis as any).document.visibilityState = "visible";
        state.documentEvents.get("visibilitychange")();
      }
      if (!callbackFirst) { await settle(); assert.deepEqual(state.calls, []); send(); send(); }
      await settle();
      assert.deepEqual(state.calls, ["exchange", "token-http", "initialize", "get_qc_status"]);
      assert.equal((await service.publicRelay.status()).state, "connected");
      assert.equal(JSON.parse(state.saved).tokens.access_token, "exchanged-fixture");
      assert.equal(JSON.parse(state.saved).state, undefined);
      send(); state.documentEvents.get("visibilitychange")();
      state.appEvents.get("appStateChange")({ isActive: true });
      t.mock.timers.tick(86_400_000); await settle();
      assert.equal(state.calls.filter(call => call === "exchange").length, 1);
    });
  }
}

test("wrong OAuth state never exchanges a code and reports failure without losing saved sign-in", async t => {
  const { state, service } = await loadService(t);
  const observed: string[] = [];
  await service.QcUsbNative.addListener("qcConnection", (value: any) => observed.push(value.state));
  state.appEvents.get("appStateChange")({ isActive: false });
  state.appEvents.get("appUrlOpen")({ url: callbackUrl.replace("fixture-state", "wrong-state") });
  await settle(); assert.deepEqual(state.calls, []);
  state.appEvents.get("appStateChange")({ isActive: true });
  await settle();
  assert.deepEqual(state.calls, []);
  assert.deepEqual(observed, ["disconnected"]);
  assert.equal(JSON.parse(state.saved).state, "fixture-state");
  assert.equal((await service.publicRelay.status()).paired, true);
});

test("a failed OAuth exchange is reported once and never retried by duplicate URL or idle events", async t => {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval", "Date"] });
  const { state, service } = await loadService(t);
  const observed: string[] = [];
  await service.QcUsbNative.addListener("qcConnection", (value: any) => observed.push(value.state));
  state.failExchange = true;
  state.appEvents.get("appUrlOpen")({ url: callbackUrl });
  await settle();
  assert.deepEqual(state.calls, ["exchange", "token-http"]);
  assert.deepEqual(observed, ["disconnected"]);
  state.appEvents.get("appUrlOpen")({ url: callbackUrl });
  state.appEvents.get("appStateChange")({ isActive: true });
  t.mock.timers.tick(86_400_000); await settle();
  assert.deepEqual(state.calls, ["exchange", "token-http"]);
  assert.equal((await service.publicRelay.status()).paired, true);
});

test("switching to the direct iPad driver clears only the MCP connection flag", async t => {
  const { state, service, storage } = await loadService(t);
  state.failConnect = false;
  await service.publicRelay.start();
  assert.equal((await service.publicRelay.status()).state, "connected");
  state.deviceFamily = "ipad";
  storage.set("qc.remote.ios.connection-mode-v1", "auto");
  await service.QcUsbNative.connect();
  assert.notEqual((await service.publicRelay.status()).state, "connected");
  assert.equal((await service.publicRelay.status()).paired, true);
});
