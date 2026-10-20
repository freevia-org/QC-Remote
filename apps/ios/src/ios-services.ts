import { App } from "@capacitor/app";
import { Browser } from "@capacitor/browser";
import { Capacitor, registerPlugin, type PluginListenerHandle } from "@capacitor/core";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { auth, UnauthorizedError } from "@modelcontextprotocol/sdk/client/auth.js";
import type { OAuthClientInformationMixed, OAuthClientMetadata, OAuthTokens } from "@modelcontextprotocol/sdk/shared/auth.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { createGatewayClientTransport, withDeviceConnectionSource, type GatewayTransport, type NativeStateFrame, type PresetSnapshot, type RuntimeStatus } from "@qc-remote/client";
import { controlAccessPermitsAction, createQcGatewayTransport, invokeAppMcp, parseAppMcpResult, parseControlAccessMode, SHARED_QC_ACTIONS, type ControlAccessMode, type PublicRelayPort, type PublicRelayState, type PublicRelayStatus, type PublicRelayUsage, type QcDeviceTransport, type QcStateUpdate } from "@qc-remote/core";
import { QC_BRAND } from "@qc-remote/theme";
import type { VirtualDeviceStorage } from "@qc-remote/ui";
import { QCIosNativeBridge } from "../native-bridge";
import { confirmIosWorkflow } from "./workflow-confirmations";
import { RemoteTraffic } from "./remote-traffic";

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
let mcpConnected = false;
let appActive = true;
const cloudActive = () => appActive && document.visibilityState !== "hidden";
const remoteTraffic = new RemoteTraffic(Date.now, cloudActive);
let connecting = false;
let mode: ControlAccessMode = parseControlAccessMode(localStorage.getItem("qc-control.device-access-mode-v1"));
let lastStatus: any;
let cursor = 0;
let directIpadSession = false;
const iosConnectionModeKey = "qc.remote.ios.connection-mode-v1";

export type IosConnectionMode = "midi" | "auto" | "remote";

export function getIosConnectionMode(): IosConnectionMode | undefined {
  const value = localStorage.getItem(iosConnectionModeKey);
  return value === "midi" || value === "auto" || value === "remote" ? value : undefined;
}

function prefersRemoteHost(): boolean {
  return getIosConnectionMode() === "remote";
}

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
  if (Capacitor.isNativePlatform() && !prefersRemoteHost()) throw new Error("Choose Remote mode before connecting to QC Remote Cloud.");
  if (connecting || client || !cloudActive()) return;
  await load();
  if (connecting || client || !cloudActive()) return;
  connecting = true;
  mcpConnected = false;
  const nextClient = new Client({ name: "QC Remote for iOS", version: "0.1.0" });
  const nextTransport = new StreamableHTTPClientTransport(new URL(serverUrl), { authProvider: provider, fetch: (input, init) => remoteTraffic.fetch(input, init), reconnectionOptions: { initialReconnectionDelay: 5_000, maxReconnectionDelay: 300_000, reconnectionDelayGrowFactor: 2, maxRetries: 0 } });
  nextClient.onclose = () => {
    if (transport !== nextTransport) return;
    client = undefined; transport = undefined;
    mcpConnected = false;
    emit("qcConnection", { state: "disconnected" });
    emit("relayState", { state: "reconnecting" });
  };
  client = nextClient; transport = nextTransport;
  try {
    await nextClient.connect(nextTransport);
    if (client !== nextClient || !cloudActive()) { await nextClient.close().catch(() => undefined); return; }
    authPending = false;
    lastStatus = await callTool("get_qc_status", {});
    if (client !== nextClient || !cloudActive()) return;
    mcpConnected = true;
    emit("qcConnection", { state: "available", name: lastStatus?.deviceName ?? "Quad Cortex via QC Remote" });
    cursor = 0;
  } catch (error) {
    if (client !== nextClient || transport !== nextTransport) return;
    mcpConnected = false;
    if (error instanceof UnauthorizedError) { authPending = true; emit("qcConnection", { state: "disconnected" }); return; }
    client = undefined; transport = undefined;
    await nextClient.close().catch(() => undefined);
    emit("qcConnection", { state: "disconnected" });
    throw error;
  } finally { connecting = false; }
}
let lastRemoteObservationAt: number | undefined;
export const remoteObservation = () => lastRemoteObservationAt;
export const subscribeRemoteObservation = (listener: (value: number) => void) => addListener("remoteObservation", listener);

async function callTool(name: string, args: Record<string, unknown>): Promise<any> {
  if (!client) throw new Error("Connect or refresh the paired host to continue. Your sign-in is saved.");
  const result = parseAppMcpResult(await client.callTool({ name, arguments: args }));
  if (name === "get_current_preset") {
    lastRemoteObservationAt = Date.now();
    emit("remoteObservation", lastRemoteObservationAt);
  }
  return result;
}

/** Explicit user refresh, never called by a polling timer. Ordinary gateway
 * reads also stay direct: last-observed UI state is not an authoritative cache. */
export async function refreshRemoteObservation(): Promise<PresetSnapshot> {
  if (!client) await connectMcp();
  lastStatus = await callTool("get_qc_status", {});
  return await callTool("get_current_preset", {});
}
async function invokeRemote(method: string, args: Record<string, unknown> = {}): Promise<any> {
  const result = await invokeAppMcp(method, args, {
    accessMode: mode,
    confirm: confirmIosWorkflow,
    onStatus: status => { lastStatus = status; },
    callTool
  });
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
      if (capabilities.deviceFamily === "ipad" && !prefersRemoteHost()) {
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
    return { devices: mcpConnected ? [{ deviceId: 1, name: lastStatus?.deviceName ?? "Quad Cortex via QC Remote", permission: true, interfaces: 1 }] : [], connected: mcpConnected, synchronized: mcpConnected };
  },
  async connect() {
    if (Capacitor.isNativePlatform()) {
      const capabilities = await QCIosNativeBridge.platformCapabilities();
      if (capabilities.deviceFamily === "ipad" && !prefersRemoteHost()) {
        const driver = await QCIosNativeBridge.scanUsbDriver();
        if (driver.available || directIpadSession) {
          const result = await QCIosNativeBridge.connectUsbDriver();
          directIpadSession = true;
          directStateFrames.splice(0);
          cursor = 0;
          const previousClient = client;
          client = undefined;
          transport = undefined;
          mcpConnected = false;
          void previousClient?.close().catch(() => undefined);
          return { connected: true, synchronized: result.synchronized, name: result.name, deviceId: result.deviceId };
        }
      }
    }
    try { await connectMcp(); } catch (error) { throw error; }
    if (authPending || !mcpConnected) throw new Error("Finish QC Remote Cloud sign-in in the browser to connect the paired host.");
    return { connected: mcpConnected, synchronized: mcpConnected, name: lastStatus?.deviceName ?? "Quad Cortex via QC Remote", deviceId: 1 };
  },
  async disconnect() {
    if (directIpadSession) {
      directIpadSession = false;
      directStateFrames.splice(0);
      await QCIosNativeBridge.disconnectUsbDriver();
    }
    try { await client?.close(); } finally { client = undefined; transport = undefined; mcpConnected = false; emit("qcConnection", { state: "disconnected" }); }
  },
  async diagnostics() {
    if (directIpadSession) {
      const [usb, driver] = await Promise.all([QCIosNativeBridge.scanUsbDriver(), QCIosNativeBridge.usbDriverDiagnostics()]);
      return { connected: true, synchronized: Boolean(usb.synchronized), phase: usb.synchronized ? "ready" : "syncing", device: "Quad Cortex USB DriverKit", messagesReceived: 0, messagesSent: 0, decodeErrors: 0, expectedWriteStalls: 0, lastMessageType: 0, connectedAt: 0, setlistKnown: false, presetPosition: 0, modelCount: 0, readAttempts: 0, negativeReads: 0, interfaceId: driver.interfaceNumber ?? -1, inputEndpointAddress: driver.inputEndpointAddress ?? 0, inputMaxPacketSize: driver.inputPacketSize ?? 0, reportBytes: driver.reportLength ?? 0, midiAvailable: false, midiInterfaceId: -1, midiOutputEndpointAddress: 0, lastMidiQueueDelayMs: 0, maxMidiQueueDelayMs: 0, lastHidWriteDurationMs: 0, maxHidWriteDurationMs: 0, lastStateAt: Date.now(), writesQuarantined: driver.writesQuarantined === 1 };
    }
    return { connected: mcpConnected, synchronized: mcpConnected, phase: mcpConnected ? "ready" : authPending ? "connecting" : "disconnected", device: "QC Remote Cloud", messagesReceived: 0, messagesSent: 0, decodeErrors: 0, expectedWriteStalls: 0, lastMessageType: 0, connectedAt: 0, setlistKnown: false, presetPosition: 0, modelCount: 0, readAttempts: 0, negativeReads: 0, interfaceId: -1, inputEndpointAddress: 0, inputMaxPacketSize: 0, reportBytes: 0, midiAvailable: false, midiInterfaceId: -1, midiOutputEndpointAddress: 0, lastMidiQueueDelayMs: 0, maxMidiQueueDelayMs: 0, lastHidWriteDurationMs: 0, maxHidWriteDurationMs: 0, lastStateAt: Date.now(), writesQuarantined: false };
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
  const hostRelay = lastStatus?.usbDiagnostics?.updaterRelay;
  const relayDetail = hostRelay && typeof hostRelay.enabled === "boolean"
    ? ` The paired host's QC internet forwarding is ${hostRelay.enabled ? "enabled" : "disabled"}.`
    : " Forwarding for QC account, cloud, and update requests depends on the paired host's QC Remote version and settings.";
  return {
    phase: android.phase,
    detail: directIpadSession
      ? "Connected directly through the iPad DriverKit prototype. This direct iOS path does not provide the QC internet request forwarder; use Paired host mode for QC account, cloud, and update requests."
      : mcpConnected
        ? `Connected through the QC Remote paired host.${relayDetail}`
        : "Sign in to QC Remote Cloud to reach a paired host.",
    connected: android.connected,
    synchronized: android.synchronized,
    android
  };
}
export const createAndroidQcTransport = (gateway: GatewayTransport, currentSnapshot: () => PresetSnapshot): QcDeviceTransport => createQcGatewayTransport(gateway, currentSnapshot);
export const ScreenWakeNative = { setEnabled: ({ enabled }: { enabled: boolean }) => QCIosNativeBridge.setScreenAwake({ enabled }) };
export const virtualDeviceStorage: VirtualDeviceStorage = { async read() { return localStorage.getItem("qc-demo-device-v1"); }, async write(text) { localStorage.setItem("qc-demo-device-v1", text); } };
export const openRemoteAccountDeletionPage = async () => { await Browser.open({ url: `${endpoint}/account/delete` }); };

const status = async (): Promise<PublicRelayStatus> => {
  await load();
  const paired = Boolean(stored.tokens);
  return { paired, state: mcpConnected ? "connected" : authPending ? "connecting" : paired ? "reconnecting" : "pairing_required", accessMode: mode, endpoint, ...(paired ? { pairing: { email: "Signed in", device: "iOS controller", pairedAt: 0 } } : {}) };
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

/** Switch the iOS UI's connection route after closing any current USB or cloud session. */
export async function setIosConnectionMode(mode: IosConnectionMode): Promise<void> {
  const previousMode = localStorage.getItem(iosConnectionModeKey);
  localStorage.setItem(iosConnectionModeKey, mode);
  try {
    await nativePlugin.disconnect();
    await QCIosNativeBridge.disconnectUsbMidi().catch(() => undefined);
  } catch (error) {
    if (previousMode === null) localStorage.removeItem(iosConnectionModeKey);
    else localStorage.setItem(iosConnectionModeKey, previousMode);
    throw error;
  }
}

type PendingOAuthCallback = { code: string; state: string };
let pendingOAuthCallback: PendingOAuthCallback | undefined;
let activeOAuthState: string | undefined;
const handledOAuthStates = new Set<string>();

function signInFailed(error: unknown): void {
  authPending = false;
  emit("qcConnection", { state: "disconnected" });
  console.error("QC Remote sign-in failed", error);
}

/** Native URL and foreground events may arrive in either order. Retain the
 * callback until HTTP is allowed, then exchange its one-time code once. */
async function drainOAuthCallback(): Promise<void> {
  if (!cloudActive() || activeOAuthState || !pendingOAuthCallback) return;
  const callback = pendingOAuthCallback;
  pendingOAuthCallback = undefined;
  activeOAuthState = callback.state;
  try {
    await load();
    if (!cloudActive()) { pendingOAuthCallback ??= callback; return; }
    if (!stored.state || callback.state !== stored.state) throw new Error("The sign-in callback could not be verified. Start sign-in again.");
    await Browser.close().catch(() => undefined);
    if (!cloudActive()) { pendingOAuthCallback ??= callback; return; }
    // Duplicated appUrlOpen events must not redeem a one-time code twice,
    // including after an ambiguous exchange failure. A new sign-in gets a
    // new state. Retain only bounded state identifiers, never consumed codes.
    handledOAuthStates.add(callback.state);
    if (handledOAuthStates.size > 16) handledOAuthStates.delete(handledOAuthStates.values().next().value!);
    if (transport) await transport.finishAuth(callback.code);
    else if (await auth(provider, { serverUrl, authorizationCode: callback.code, fetchFn: (input, init) => remoteTraffic.fetch(input, init) }) !== "AUTHORIZED") {
      throw new Error("QC Remote Cloud did not authorize this sign-in.");
    }
    stored.state = undefined;
    await save();
    authPending = false;
    client = undefined; transport = undefined; mcpConnected = false;
    // If the app went inactive during the exchange, keep the saved tokens;
    // connectMcp's foreground gate leaves reconnect to the next user demand.
    await connectMcp();
  } catch (error) { signInFailed(error); }
  finally {
    activeOAuthState = undefined;
    if (cloudActive() && pendingOAuthCallback) void drainOAuthCallback();
  }
}

function updateCloudActivity(): void {
  if (cloudActive()) { void drainOAuthCallback(); return; }
  const previous = client;
  client = undefined;
  transport = undefined;
  mcpConnected = false;
  // No background keepalive, SSE reconnect, or foreground-resume retry.
  // Only an already-received OAuth callback is drained on foreground events.
  void previous?.close().catch(() => undefined);
}
void App.addListener("appStateChange", ({ isActive }) => { appActive = isActive; updateCloudActivity(); });
document.addEventListener("visibilitychange", updateCloudActivity);

const appUrlListener = App.addListener("appUrlOpen", ({ url }) => {
  try {
    const callback = new URL(url);
    if (callback.protocol !== "qcremote:" || callback.hostname !== "oauth") return;
    const code = callback.searchParams.get("code");
    const state = callback.searchParams.get("state");
    if (!code || !state) throw new Error("The sign-in callback could not be verified. Start sign-in again.");
    if (handledOAuthStates.has(state) || activeOAuthState === state || pendingOAuthCallback?.state === state) return;
    pendingOAuthCallback = { code, state };
    authPending = true;
    void drainOAuthCallback();
  } catch (error) { signInFailed(error); }
});
void appUrlListener;

if (Capacitor.isNativePlatform()) void QCIosNativeBridge.setScreenAwake({ enabled: false });
