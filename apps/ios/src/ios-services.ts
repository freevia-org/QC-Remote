import { App } from "@capacitor/app";
import { Browser } from "@capacitor/browser";
import { Capacitor, registerPlugin, type PluginListenerHandle } from "@capacitor/core";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { UnauthorizedError } from "@modelcontextprotocol/sdk/client/auth.js";
import type { OAuthClientInformationMixed, OAuthClientMetadata, OAuthTokens } from "@modelcontextprotocol/sdk/shared/auth.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { createGatewayClientTransport, GATEWAY_CLIENT_METHODS, withDeviceConnectionSource, type GatewayTransport, type NativeStateFrame, type PresetSnapshot, type RuntimeStatus } from "@qc-remote/client";
import { controlAccessPermitsAction, createQcGatewayTransport, parseControlAccessMode, SHARED_QC_ACTIONS, type ControlAccessMode, type PublicRelayPort, type PublicRelayState, type PublicRelayStatus, type PublicRelayUsage, type QcDeviceTransport, type QcStateUpdate } from "@qc-remote/core";
import { QC_BRAND } from "@qc-remote/theme";
import type { VirtualDeviceStorage } from "@qc-remote/ui";
import { QCIosNativeBridge } from "../native-bridge";
import { confirmIosWorkflow } from "./workflow-confirmations";

const endpoint = QC_BRAND.publicRelayEndpoint.replace(/\/$/, "");
const serverUrl = `${endpoint}/mcp`;
const callbackUrl = "qcremote://oauth/callback";
const storageKey = "qcr-ios-oauth-v1";
type Stored = { client?: OAuthClientInformationMixed; tokens?: OAuthTokens; verifier?: string; state?: string; discovery?: unknown };
const listeners = new Map<string, Set<(value: any) => void>>();
const directStateFrames: Array<{ streamId?: string; sequence: number; observedAt: number; states: unknown[]; tempoClock?: Record<string, unknown> }> = [];
let stored: Stored = {};
let loaded = false;
let client: Client | undefined;
let transport: StreamableHTTPClientTransport | undefined;
let authPending = false;
let connecting = false;
let mode: ControlAccessMode = parseControlAccessMode(localStorage.getItem("qc-control.device-access-mode-v1"));
let lastStatus: any;
let cursor = 0;
let statePoll: ReturnType<typeof setInterval> | undefined;
let directIpadSession = false;

async function load(): Promise<void> {
  if (loaded) return;
  loaded = true;
  try { const raw = (await QCIosNativeBridge.secureGet({ key: storageKey })).value; if (raw) stored = JSON.parse(raw); } catch { stored = {}; }
}
async function save(): Promise<void> { await QCIosNativeBridge.secureSet({ key: storageKey, value: JSON.stringify(stored) }); }
function emit(name: string, value: unknown): void {
  if (name === "qcStateBatch" && directIpadSession) {
    directStateFrames.push(value as (typeof directStateFrames)[number]);
    if (directStateFrames.length > 512) directStateFrames.splice(0, directStateFrames.length - 512);
  }
  for (const listener of listeners.get(name) ?? []) listener(value);
}
function addListener<T>(eventName: string, listener: (value: T) => void): Promise<PluginListenerHandle> {
  const group = listeners.get(eventName) ?? new Set(); group.add(listener as (value: any) => void); listeners.set(eventName, group);
  return Promise.resolve({ remove: async () => { group.delete(listener as (value: any) => void); } } as PluginListenerHandle);
}
if (Capacitor.isNativePlatform()) {
  void QCIosNativeBridge.addListener("qcStateBatch", (frame) => emit("qcStateBatch", frame)).catch(() => undefined);
  void QCIosNativeBridge.addListener("qcConnection", (status) => emit("qcConnection", status)).catch(() => undefined);
}
function parseResult(result: any): any {
  if (result?.isError) throw new Error(result.content?.map((c: any) => c.text ?? "").join("\n") || "QC Remote request failed.");
  const text = result?.content?.find((c: any) => c.type === "text")?.text;
  if (!text) return result;
  try { return JSON.parse(text); } catch { return text; }
}
const snake = (name: string) => name.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`);
const provider = {
  get redirectUrl() { return callbackUrl; },
  get clientMetadata(): OAuthClientMetadata {
    return { client_name: "QC Remote for iOS", redirect_uris: [callbackUrl], grant_types: ["authorization_code", "refresh_token"], response_types: ["code"], token_endpoint_auth_method: "none", scope: "qc:read qc:control offline_access" };
  },
  state: async () => { stored.state = crypto.randomUUID(); await save(); return stored.state; },
  clientInformation: async () => { await load(); return stored.client; },
  saveClientInformation: async (value: OAuthClientInformationMixed) => { stored.client = value; await save(); },
  tokens: async () => { await load(); return stored.tokens; },
  saveTokens: async (value: OAuthTokens) => { stored.tokens = value; await save(); },
  saveCodeVerifier: async (value: string) => { stored.verifier = value; await save(); },
  codeVerifier: async () => { await load(); if (!stored.verifier) throw new Error("The sign-in session expired. Start sign-in again."); return stored.verifier; },
  redirectToAuthorization: async (url: URL) => { authPending = true; await Browser.open({ url: url.toString(), presentationStyle: "popover" }); },
  saveDiscoveryState: async (value: unknown) => { stored.discovery = value; await save(); },
  discoveryState: async () => { await load(); return stored.discovery as any; },
  invalidateCredentials: async (scope: "all" | "client" | "tokens" | "verifier" | "discovery") => {
    if (scope === "all") stored = {};
    else if (scope === "client") delete stored.client;
    else if (scope === "tokens") delete stored.tokens;
    else if (scope === "verifier") delete stored.verifier;
    else delete stored.discovery;
    await save();
  }
};

async function connectMcp(): Promise<void> {
  if (connecting || client) return;
  connecting = true;
  await load();
  const nextClient = new Client({ name: "QC Remote for iOS", version: "0.1.0" });
  const nextTransport = new StreamableHTTPClientTransport(new URL(serverUrl), { authProvider: provider });
  nextTransport.onclose = () => {
    if (transport !== nextTransport) return;
    if (statePoll) clearInterval(statePoll);
    statePoll = undefined;
    client = undefined; transport = undefined;
    emit("qcConnection", { state: "disconnected" });
    emit("relayState", { state: "reconnecting" });
  };
  client = nextClient; transport = nextTransport;
  try {
    await nextClient.connect(nextTransport);
    authPending = false;
    lastStatus = await callTool("get_qc_status", {});
    emit("qcConnection", { state: "available", name: lastStatus?.deviceName ?? "Quad Cortex via QC Remote" });
    if (!directIpadSession) {
      cursor = 0;
      startStatePolling();
    }
  } catch (error) {
    if (error instanceof UnauthorizedError) { authPending = true; emit("qcConnection", { state: "disconnected" }); return; }
    client = undefined; transport = undefined;
    emit("qcConnection", { state: "disconnected" });
    throw error;
  } finally { connecting = false; }
}
async function callTool(name: string, args: Record<string, unknown>): Promise<any> {
  if (!client) throw new Error("Sign in to QC Remote Cloud to connect to the paired host.");
  return parseResult(await client.callTool({ name, arguments: args }));
}
function startStatePolling(): void {
  if (directIpadSession) return;
  if (statePoll) clearInterval(statePoll);
  statePoll = setInterval(() => { void (async () => {
    try {
      const page = await invokeRemote("device.stateEvents", { afterSequence: cursor, limit: 256 });
      if (directIpadSession) return;
      for (const frame of page.frames ?? []) {
        cursor = Math.max(cursor, frame.sequence ?? cursor);
        emit("qcStateBatch", frame as NativeStateFrame<QcStateUpdate>);
      }
    } catch { /* State refreshes recover transient network gaps through currentPreset. */ }
  })(); }, 1_500);
}
async function invokeRemote(method: string, args: Record<string, unknown> = {}): Promise<any> {
  if (method === "system.status") return await callTool("get_qc_status", {});
  if (method === "device.snapshot" || method === "device.currentPreset") return await callTool("get_current_preset", {});
  const descriptor = GATEWAY_CLIENT_METHODS[Object.keys(GATEWAY_CLIENT_METHODS).find((key) => (GATEWAY_CLIENT_METHODS as any)[key].rpc === method) as keyof typeof GATEWAY_CLIENT_METHODS];
  if (!descriptor) throw new Error(`This iOS remote session does not provide ${method}.`);
  const action = SHARED_QC_ACTIONS.find((candidate) => candidate.rpc === method);
  if (!action) throw new Error(`The cloud host does not publish the ${method} action.`);
  if (!controlAccessPermitsAction(mode, action.name)) throw new Error(`The current ${mode} access mode does not allow ${action.name}.`);
  if (["risky-write", "persistent-write"].includes(action.classification)
      || ["show_tuner", "show_gig_view", "copy_scene"].includes(action.name)) {
    const accepted = await confirmIosWorkflow(`${action.description}\n\nContinue with ${action.name.replaceAll("_", " ")}?`);
    if (!accepted) throw new Error("Cancelled.");
    const required = action.required as readonly string[];
    if (required.includes("confirm_risky_operation")) args.confirmRiskyOperation = true;
    if (required.includes("confirm_persistent_write")) args.confirmPersistentWrite = true;
  }
  const params = Object.fromEntries(Object.entries(args).map(([key, value]) => [snake(key), value]));
  if (action.name === "rename_current_preset" && "name" in args) { delete params.name; params.new_name = args.name; }
  const result = await callTool("execute_qc_action", { action: action.name, params });
  if (["device.reconnect", "device.resetSession", "device.disconnect"].includes(method)) {
    const runtime = await callTool("get_qc_status", {});
    return {
      phase: runtime.usbDiagnostics?.phase === "ready" ? "ready" : runtime.usbDiagnostics?.phase === "syncing" ? "syncing" : runtime.usbDiagnostics?.connected ? "opening" : "disconnected",
      detail: runtime.usbDiagnostics?.detail ?? "Remote host connection updated.",
      demo: false,
      source: "device",
      restarted: result?.changed !== false
    };
  }
  if (method === "device.stateEvents") {
    for (const frame of result.frames ?? []) cursor = Math.max(cursor, frame.sequence ?? cursor);
  }
  return result;
}

const nativePlugin = {
  async scan() {
    await load();
    if (Capacitor.isNativePlatform()) {
      const capabilities = await QCIosNativeBridge.platformCapabilities();
      if (capabilities.deviceFamily === "ipad") {
        const driver = await QCIosNativeBridge.scanUsbDriver();
        if (driver.available || driver.connected) {
          directIpadSession = Boolean(driver.connected);
          return {
            devices: [{ deviceId: 1, name: "Quad Cortex via iPad USB DriverKit", permission: true, interfaces: 1 }],
            connected: Boolean(driver.connected), synchronized: Boolean(driver.synchronized)
          };
        }
      }
    }
    if (!client && !authPending && stored.tokens) void connectMcp().catch(() => undefined);
    return { devices: client ? [{ deviceId: 1, name: lastStatus?.deviceName ?? "Quad Cortex via QC Remote", permission: true, interfaces: 1 }] : [], connected: Boolean(client), synchronized: Boolean(client) };
  },
  async connect() {
    if (Capacitor.isNativePlatform()) {
      const capabilities = await QCIosNativeBridge.platformCapabilities();
      if (capabilities.deviceFamily === "ipad") {
        const driver = await QCIosNativeBridge.scanUsbDriver();
        if (driver.available || directIpadSession) {
          const result = await QCIosNativeBridge.connectUsbDriver();
          directIpadSession = true;
          directStateFrames.splice(0);
          cursor = 0;
          if (statePoll) clearInterval(statePoll);
          statePoll = undefined;
          const previousClient = client;
          client = undefined;
          transport = undefined;
          void previousClient?.close().catch(() => undefined);
          return { connected: true, synchronized: result.synchronized, name: result.name, deviceId: result.deviceId };
        }
      }
    }
    try { await connectMcp(); } catch (error) { throw error; }
    if (authPending || !client) throw new Error("Finish QC Remote Cloud sign-in in the browser to connect the paired host.");
    return { connected: Boolean(client), synchronized: Boolean(client), name: lastStatus?.deviceName ?? "Quad Cortex via QC Remote", deviceId: 1 };
  },
  async disconnect() {
    if (statePoll) clearInterval(statePoll); statePoll = undefined;
    if (directIpadSession) {
      directIpadSession = false;
      directStateFrames.splice(0);
      await QCIosNativeBridge.disconnectUsbDriver();
    }
    try { await client?.close(); } finally { client = undefined; transport = undefined; emit("qcConnection", { state: "disconnected" }); }
  },
  async diagnostics() {
    if (directIpadSession) {
      const [usb, driver] = await Promise.all([QCIosNativeBridge.scanUsbDriver(), QCIosNativeBridge.usbDriverDiagnostics()]);
      return { connected: true, synchronized: Boolean(usb.synchronized), phase: usb.synchronized ? "ready" : "syncing", device: "Quad Cortex USB DriverKit", messagesReceived: 0, messagesSent: 0, decodeErrors: 0, expectedWriteStalls: 0, lastMessageType: 0, connectedAt: 0, setlistKnown: false, presetPosition: 0, modelCount: 0, readAttempts: 0, negativeReads: 0, interfaceId: driver.interfaceNumber ?? -1, inputEndpointAddress: driver.inputEndpointAddress ?? 0, inputMaxPacketSize: driver.inputPacketSize ?? 0, reportBytes: driver.reportLength ?? 0, midiAvailable: false, midiInterfaceId: -1, midiOutputEndpointAddress: 0, lastMidiQueueDelayMs: 0, maxMidiQueueDelayMs: 0, lastHidWriteDurationMs: 0, maxHidWriteDurationMs: 0, lastStateAt: Date.now(), writesQuarantined: driver.writesQuarantined === 1 };
    }
    return { connected: Boolean(client), synchronized: Boolean(client), phase: client ? "ready" : authPending ? "connecting" : "disconnected", device: "QC Remote Cloud", messagesReceived: 0, messagesSent: 0, decodeErrors: 0, expectedWriteStalls: 0, lastMessageType: 0, connectedAt: 0, setlistKnown: false, presetPosition: 0, modelCount: 0, readAttempts: 0, negativeReads: 0, interfaceId: -1, inputEndpointAddress: 0, inputMaxPacketSize: 0, reportBytes: 0, midiAvailable: false, midiInterfaceId: -1, midiOutputEndpointAddress: 0, lastMidiQueueDelayMs: 0, maxMidiQueueDelayMs: 0, lastHidWriteDurationMs: 0, maxHidWriteDurationMs: 0, lastStateAt: Date.now(), writesQuarantined: false };
  },
  async gatewayInvoke({ method, params }: { method: string; params?: Record<string, unknown> }) {
    if (directIpadSession) {
      if (method === "system.status") {
        const diagnostics = await nativePlugin.diagnostics();
        return { deviceName: "Quad Cortex", usbDiagnostics: diagnostics, connected: diagnostics.connected, synchronized: diagnostics.synchronized };
      }
      if (method === "device.snapshot" || method === "device.currentPreset") {
        const { snapshot } = await QCIosNativeBridge.driverCurrentPreset();
        if (!snapshot) throw new Error("The Quad Cortex has not published its current preset yet.");
        return snapshot;
      }
      if (method === "device.stateEvents") {
        const after = Number(params?.afterSequence ?? 0);
        const limit = Math.max(1, Math.min(512, Number(params?.limit ?? 256)));
        const frames = directStateFrames.filter((frame) => frame.sequence > after).slice(0, limit);
        return { native: true, streamId: frames.at(-1)?.streamId, latestSequence: frames.at(-1)?.sequence ?? after, frames };
      }
      const action = SHARED_QC_ACTIONS.find((candidate) => candidate.rpc === method);
      if (action) {
        if (!controlAccessPermitsAction(mode, action.name)) throw new Error(`The current ${mode} access mode does not allow ${action.name}.`);
        const actionParams = { ...(params ?? {}) };
        if (["risky-write", "persistent-write"].includes(action.classification)
            || ["show_tuner", "show_gig_view", "copy_scene"].includes(action.name)) {
          const accepted = await confirmIosWorkflow(`${action.description}\n\nContinue with ${action.name.replaceAll("_", " ")}?`);
          if (!accepted) throw new Error("Cancelled.");
          const required = action.required as readonly string[];
          if (required.includes("confirm_risky_operation")) actionParams.confirmRiskyOperation = true;
          if (required.includes("confirm_persistent_write")) actionParams.confirmPersistentWrite = true;
        }
        return QCIosNativeBridge.gatewayWriteUsb({ method, params: actionParams });
      }
      return QCIosNativeBridge.gatewayReadUsb({ method, params });
    }
    if (!client && stored.tokens && !authPending) await connectMcp();
    return invokeRemote(method, params);
  },
  addListener
};
export const QcUsbNative = nativePlugin as any;
export const androidGatewayTransport = withDeviceConnectionSource(createGatewayClientTransport<GatewayTransport>(<T,>(method: string, params?: Record<string, unknown>) => nativePlugin.gatewayInvoke({ method, params }) as Promise<T>, "rpc"));
export async function androidUsbRuntimeDiagnostics(): Promise<NonNullable<RuntimeStatus["usbDiagnostics"]> & { android: any }> {
  const android = await nativePlugin.diagnostics();
  return { phase: android.phase, detail: directIpadSession ? "Connected directly to the Quad Cortex through the iPad DriverKit prototype." : client ? "Connected through the QC Remote host." : "Sign in to QC Remote Cloud to reach a paired host.", connected: android.connected, synchronized: android.synchronized, android };
}
export const createAndroidQcTransport = (gateway: GatewayTransport, currentSnapshot: () => PresetSnapshot): QcDeviceTransport => createQcGatewayTransport(gateway, currentSnapshot);
export const ScreenWakeNative = { setEnabled: ({ enabled }: { enabled: boolean }) => QCIosNativeBridge.setScreenAwake({ enabled }) };
export const virtualDeviceStorage: VirtualDeviceStorage = { async read() { return localStorage.getItem("qc-demo-device-v1"); }, async write(text) { localStorage.setItem("qc-demo-device-v1", text); } };
export const openRemoteAccountDeletionPage = async () => { await Browser.open({ url: `${endpoint}/account/delete` }); };

const status = async (): Promise<PublicRelayStatus> => {
  await load();
  const paired = Boolean(stored.tokens);
  return { paired, state: client ? "connected" : authPending ? "connecting" : paired ? "reconnecting" : "pairing_required", accessMode: mode, endpoint, ...(paired ? { pairing: { email: "Signed in", device: "iOS controller", pairedAt: 0 } } : {}) };
};
export const subscribeRelayState = (listener: (state: PublicRelayState) => void) => addListener("relayState", listener);
export const publicRelay: PublicRelayPort = {
  status,
  async usage(): Promise<PublicRelayUsage> { const value = await callTool("get_remote_usage", {}); return { day: value.day ?? "", commands: value.commands ?? 0, lastCommandAt: value.lastCommandAt, platformUsage: value.platformUsage }; },
  async pair() { await connectMcp(); return status(); },
  async start() { await connectMcp(); },
  async unpair() { await nativePlugin.disconnect(); stored = {}; await save(); return status(); },
  async setAccessMode(value) { mode = value; localStorage.setItem("qc-control.device-access-mode-v1", mode); return status(); },
  browserPairing: {
    async begin() { await connectMcp(); return { expiresAt: Date.now() + 600_000 }; },
    async poll() { const current = await status(); return current.paired ? { state: "paired" as const, status: current } : { state: "waiting" as const }; }
  }
};

const appUrlListener = App.addListener("appUrlOpen", ({ url }) => {
  void (async () => {
    const callback = new URL(url);
    if (callback.protocol !== "qcremote:" || callback.hostname !== "oauth") return;
    const code = callback.searchParams.get("code");
    const state = callback.searchParams.get("state");
    if (!code || !state || !stored.state || state !== stored.state) throw new Error("The sign-in callback could not be verified. Start sign-in again.");
    await Browser.close().catch(() => undefined);
    await transport?.finishAuth(code);
    stored.state = undefined;
    await save();
    client = undefined; transport = undefined;
    await connectMcp();
  })().catch((error) => { authPending = false; emit("qcConnection", { state: "disconnected" }); console.error("QC Remote sign-in failed", error); });
});
void appUrlListener;

if (Capacitor.isNativePlatform()) void QCIosNativeBridge.setScreenAwake({ enabled: false });
