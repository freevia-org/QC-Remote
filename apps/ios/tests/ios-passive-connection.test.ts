import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";

/** Execute the actual iOS service with a synthetic SDK/Keychain. No cloud,
 * browser, simulator, saved profile, or installed application is accessed. */
async function loadService(t: any) {
  const state = { calls: [] as string[], failConnect: true, client: undefined as any,
    invocations: [] as any[],
    connectGate: undefined as Promise<void> | undefined,
    connectStarted: undefined as (() => void) | undefined,
    secureReadGate: undefined as Promise<void> | undefined,
    secureReads: 0,
    secureReadError: false,
    runtimeStatus: { deviceName: "fixture", usbDiagnostics: { connected: true, synchronized: true } } as any,
    toolHook: undefined as undefined | ((name: string) => any),
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
    "@modelcontextprotocol/sdk/client/index.js": `export class Client { constructor(){globalThis.__qcIosTest.client=this;this.initialized=false;} async connect(){const s=globalThis.__qcIosTest;s.calls.push('initialize');s.connectStarted?.();if(s.connectGate)await s.connectGate;if(s.failConnect)throw Error('HTTP 503');this.initialized=true;}async callTool({name}){if(!this.initialized)throw Error("SDK not initialized");const s=globalThis.__qcIosTest;s.calls.push(name);return s.toolHook?s.toolHook(name):name==='get_current_preset'?{presetName:'fixture',blocks:[]}:s.runtimeStatus;}async close(){this.onclose?.();}}`,
    "@modelcontextprotocol/sdk/client/streamableHttp.js": `export class StreamableHTTPClientTransport {constructor(url,options){if(options.reconnectionOptions.maxRetries!==0)throw Error('SDK reconnect must be disabled');}async finishAuth(){}}`,
    "@modelcontextprotocol/sdk/client/auth.js": `export class UnauthorizedError extends Error {} export const auth=async()=>"AUTHORIZED";`,
    "@qc-remote/client": `export const selectedBrokerTarget=x=>x.targetReady===false?undefined:x.brokerTarget;export const brokerTargetKey=x=>x?JSON.stringify([x.source,x.sessionId,x.generation]):undefined;export const validateGatewayResult=(method,x)=>x;export const GATEWAY_CLIENT_METHODS={};export const withDeviceConnectionSource=x=>x;export const createGatewayClientTransport=fn=>({invoke:fn});`,
    "@qc-remote/core": `export const parseControlAccessMode=()=>"read-only";export const controlAccessPermitsAction=()=>true;export const SHARED_QC_ACTIONS=[{rpc:"device.setMasterVolume",name:"set_master_volume",classification:"live-write"}];export const createQcGatewayTransport=x=>x;export const parseAppMcpResult=x=>x;export const invokeAppMcp=async(method,args,opts)=>{globalThis.__qcIosTest.invocations.push({method,target:opts.expectedBrokerTarget,supported:opts.brokerTargetGuardSupported});const result=await opts.callTool(method==='system.status'?'get_qc_status':'get_current_preset',args);if(method==='system.status')opts.onStatus?.(result);return result;};`,
    "@qc-remote/theme": `export const QC_BRAND={publicRelayEndpoint:'https://example.test'};`,
    "../native-bridge": `export const QCIosNativeBridge={secureGet:async()=>{const s=globalThis.__qcIosTest;s.secureReads++;if(s.secureReadError)throw Error("Keychain temporarily unavailable");if(s.secureReadGate)await s.secureReadGate;return {value:s.saved};},secureSet:async({value})=>{globalThis.__qcIosTest.saved=value;},addListener:async()=>({remove:async()=>{}}),setScreenAwake:async()=>{},platformCapabilities:async()=>({deviceFamily:'iphone'})};`,
    "./workflow-confirmations": `export const confirmIosWorkflow=async()=>true;`,
  };
  const result = await build({ entryPoints: [fileURLToPath(new URL("../src/ios-services.ts", import.meta.url))], bundle: true, write: false, platform: "node", format: "esm", plugins: [{ name: "synthetic-native-and-sdk", setup(build) {
    build.onResolve({ filter: /.*/ }, args => mocks[args.path] ? { path: args.path, namespace: "mock" } : undefined);
    build.onLoad({ filter: /.*/, namespace: "mock" }, args => ({ contents: mocks[args.path], loader: "js" }));
  } }] });
  const service = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString("base64")}#${Math.random()}`);
  return { state, service, storage };
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


test("actual iOS service observes host demo without USB, preserves host route and keeps raw USB diagnostics physical", async t => {
  const { state, service } = await loadService(t);
  state.failConnect = false;
  state.runtimeStatus = { gatewayAvailable: true, targetReady: true, brokerTarget: { source: "demo", sessionId: "same-Mac-UI-demo", generation: 1 }, usbDiagnostics: { connected: false, synchronized: false, phase: "disconnected" } };
  const saved = state.saved, events: any[] = [];
  await service.QcUsbNative.addListener("qcConnection", (value: any) => events.push(value));
  const observation = await service.refreshRemoteTargetObservation();
  assert.equal(observation.connection.source, "device");
  assert.equal(observation.connection.brokerTarget.source, "demo");
  assert.equal(observation.connection.phase, "ready");
  assert.equal(observation.snapshot.presetName, "fixture");
  assert.deepEqual(events, [], "explicit refresh must not start a competing availability-driven connection attempt");
  const before = state.calls.length;
  const scan = await service.QcUsbNative.scan();
  assert.equal(scan.connected, true); assert.match(scan.devices[0].name, /Demo device via paired host/);
  const diagnostics = await service.androidUsbRuntimeDiagnostics();
  assert.equal(diagnostics.connected, false, "raw USB truth must remain absent");
  assert.equal(diagnostics.android.connected, true, "the selected demo gateway is usable");
  assert.equal(state.calls.length, before, "passive scan/diagnostics must not create MCP traffic");
  assert.equal(state.saved, saved); assert.equal((await service.publicRelay.status()).paired, true);
});

test("actual iOS service keeps saved authentication while the selected host target becomes unavailable", async t => {
  const { state, service } = await loadService(t);
  state.failConnect = false;
  await service.refreshRemoteTargetObservation();
  state.runtimeStatus = { gatewayAvailable: true, targetReady: false, usbDiagnostics: { connected: false, synchronized: false, phase: "disconnected" } };
  const saved = state.saved, before = state.calls.length;
  const observation = await service.refreshRemoteTargetObservation();
  assert.equal(observation.connection.phase, "disconnected"); assert.equal(observation.snapshot, undefined);
  assert.deepEqual(state.calls.slice(before), ["get_qc_status"]);
  assert.equal((await service.QcUsbNative.scan()).connected, false);
  assert.equal(state.saved, saved); assert.equal((await service.publicRelay.status()).paired, true);
});

test("a target change during actual iOS refresh does not publish a successful observation timestamp", async t => {
  const { state, service } = await loadService(t);
  state.failConnect = false;
  const target = { source: "demo", sessionId: "same-Mac-UI-demo", generation: 1 };
  state.runtimeStatus = { gatewayAvailable: true, targetReady: true, brokerTarget: target, usbDiagnostics: { connected: false, synchronized: false, phase: "disconnected" } };
  const observations: number[] = [];
  await service.subscribeRemoteObservation((value: number) => observations.push(value));
  await service.refreshRemoteTargetObservation();
  assert.equal(observations.length, 1);
  state.toolHook = name => {
    if (name === "get_current_preset") { state.runtimeStatus = { ...state.runtimeStatus, brokerTarget: { ...target, generation: 3 } }; return { presetName: "old demo", blocks: [] }; }
    return state.runtimeStatus;
  };
  const saved = state.saved;
  await assert.rejects(service.refreshRemoteTargetObservation(), /target changed during refresh/);
  assert.equal(observations.length, 1); assert.equal(state.saved, saved);
});


test("actual service binds writes to the accepted UI target rather than a later status guard read", async t => {
  const { state, service } = await loadService(t);
  state.failConnect = false;
  const target = { source: "demo", sessionId: "accepted-UI-target", generation: 1 };
  state.runtimeStatus = { gatewayAvailable: true, targetReady: true, brokerTarget: target, capabilities: ["mcpBrokerTargetGuard"], usbDiagnostics: { connected: false, synchronized: false, phase: "disconnected" } };
  const observation = await service.refreshRemoteTargetObservation();
  const beforeAcceptance = state.calls.length;
  await assert.rejects(service.androidGatewayTransport.invoke("device.setMasterVolume", { value: 20 }), /has not been accepted/);
  assert.equal(state.calls.length, beforeAcceptance);
  observation.accept();
  state.runtimeStatus = { ...state.runtimeStatus, brokerTarget: { source: "physical", sessionId: "new-physical", generation: 2 } };
  await service.androidGatewayTransport.invoke("system.status", {});
  await service.androidGatewayTransport.invoke("device.setMasterVolume", { value: 20 });
  assert.deepEqual(state.invocations.at(-1).target, target, "latest status cannot retarget an accepted UI operation");
  assert.equal(state.invocations.at(-1).supported, true);
});

test("actual service refuses metadata-aware writes without cloud capability and never clears sign-in", async t => {
  const { state, service } = await loadService(t);
  state.failConnect = false;
  state.runtimeStatus = { gatewayAvailable: true, targetReady: true, brokerTarget: { source: "demo", sessionId: "UI", generation: 1 }, capabilities: [], usbDiagnostics: { connected: false, synchronized: false, phase: "disconnected" } };
  (await service.refreshRemoteTargetObservation()).accept();
  const saved = state.saved, calls = state.calls.length;
  await assert.rejects(service.androidGatewayTransport.invoke("device.setMasterVolume", { value: 20 }), /cannot safely bind/);
  assert.equal(state.calls.length, calls); assert.equal(state.saved, saved); assert.equal((await service.publicRelay.status()).paired, true);
});


test("parallel explicit cold reads await one SDK initialization and never call an uninitialized client", async t => {
  const { state, service } = await loadService(t);
  state.failConnect = false;
  let release!: () => void, started!: () => void;
  state.connectGate = new Promise<void>(resolve => { release = resolve; });
  const connecting = new Promise<void>(resolve => { started = resolve; });
  state.connectStarted = started;
  const first = service.refreshRemoteTargetObservation();
  await connecting;
  const second = service.androidGatewayTransport.invoke("system.status", {});
  const third = service.refreshRemoteTargetObservation();
  assert.deepEqual(state.calls, ["initialize"]);
  release();
  await Promise.all([first, second, third]);
  assert.equal(state.calls.filter(name => name === "initialize").length, 1);
  assert.equal((await service.publicRelay.status()).paired, true);
});

test("a queued reconnecting write retains its original accepted target while another observation is adopted", async t => {
  const { state, service } = await loadService(t);
  state.failConnect = false;
  const targetA = { source: "demo", sessionId: "accepted-A", generation: 1 };
  state.runtimeStatus = { gatewayAvailable: true, targetReady: true, brokerTarget: targetA, capabilities: ["mcpBrokerTargetGuard"], usbDiagnostics: { connected: false, synchronized: false, phase: "disconnected" } };
  (await service.refreshRemoteTargetObservation()).accept();
  state.runtimeStatus = { ...state.runtimeStatus, brokerTarget: { source: "physical", sessionId: "B", generation: 2 } };
  const observationB = await service.refreshRemoteTargetObservation(); // Read, not yet adopted.
  state.client.onclose();
  let release!: () => void, started!: () => void;
  state.connectGate = new Promise<void>(resolve => { release = resolve; });
  const connecting = new Promise<void>(resolve => { started = resolve; }); state.connectStarted = started;
  const writing = service.androidGatewayTransport.invoke("device.setMasterVolume", { value: 20 });
  await connecting;
  observationB.accept();
  release();
  await writing;
  assert.deepEqual(state.invocations.at(-1).target, targetA);
});

test("concurrent Keychain loading never presents an empty saved account to cold connection", async t => {
  const { state, service } = await loadService(t);
  state.failConnect = false;
  let release!: () => void;
  state.secureReadGate = new Promise<void>(resolve => { release = resolve; });
  const saved = state.saved;
  const readingAccount = service.publicRelay.status();
  const connecting = service.refreshRemoteTargetObservation();
  await Promise.resolve(); await Promise.resolve();
  assert.deepEqual(state.calls, [], "SDK initialization must wait for the native saved credential read");
  assert.equal(state.secureReads, 1);
  release();
  const [account] = await Promise.all([readingAccount, connecting]);
  assert.equal(account.paired, true); assert.equal(state.saved, saved); assert.equal(state.secureReads, 1);
});


test("a native saved-sign-in read failure never becomes an empty account or automatic authorization", async t => {
  const { state, service } = await loadService(t);
  state.failConnect = false; state.secureReadError = true;
  const saved = state.saved;
  await assert.rejects(service.refreshRemoteTargetObservation(), /Keychain temporarily unavailable/);
  assert.deepEqual(state.calls, []); assert.equal(state.saved, saved);
  state.secureReadError = false;
  await service.refreshRemoteTargetObservation();
  assert.equal(state.secureReads, 2); assert.equal((await service.publicRelay.status()).paired, true); assert.equal(state.saved, saved);
});

test("background retirement cancels an old cold demand while a later explicit demand owns its own connection", async t => {
  const { state, service } = await loadService(t);
  state.failConnect = false;
  let release!: () => void, started!: () => void;
  state.connectGate = new Promise<void>(resolve => { release = resolve; });
  const connecting = new Promise<void>(resolve => { started = resolve; }); state.connectStarted = started;
  const first = service.refreshRemoteTargetObservation();
  const stale = assert.rejects(first, /connection changed before this request/);
  await connecting;
  state.appEvents.get("appStateChange")({ isActive: false });
  state.appEvents.get("appStateChange")({ isActive: true });
  assert.equal(state.calls.filter(name => name === "initialize").length, 1, "foreground must not retry a retired demand");
  state.connectGate = undefined;
  await service.refreshRemoteTargetObservation();
  const calls = state.calls.length;
  release(); await stale;
  assert.equal(state.calls.length, calls, "the old SDK connection must not issue tools after its replacement");
  assert.equal(state.calls.filter(name => name === "initialize").length, 2);
  assert.equal((await service.publicRelay.status()).paired, true);
});


test("switching away from paired-host mode cannot reuse that host's USB diagnostics", async t => {
  const { state, service, storage } = await loadService(t);
  state.failConnect = false;
  await service.refreshRemoteTargetObservation();
  assert.equal((await service.androidUsbRuntimeDiagnostics()).connected, true);
  storage.set("qc.remote.ios.connection-mode-v1", "auto");
  const before = state.calls.length;
  const diagnostics = await service.androidUsbRuntimeDiagnostics();
  assert.equal(diagnostics.connected, false);
  assert.match(diagnostics.detail, /No direct Quad Cortex USB session/);
  assert.equal(state.calls.length, before);
});
