import { App } from "@capacitor/app";
import { Browser } from "@capacitor/browser";
import { Capacitor, registerPlugin, type PluginListenerHandle } from "@capacitor/core";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { auth, UnauthorizedError } from "@modelcontextprotocol/sdk/client/auth.js";
import type { OAuthClientInformationMixed, OAuthClientMetadata, OAuthTokens } from "@modelcontextprotocol/sdk/shared/auth.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { createGatewayClientTransport, withDeviceConnectionSource, type BrokerTarget, type GatewayTransport, type NativeStateFrame, type PresetSnapshot, type RuntimeStatus } from "@qc-remote/client";
import { controlAccessPermitsAction, createQcGatewayTransport, invokeAppMcp, parseAppMcpResult, parseControlAccessMode, SHARED_QC_ACTIONS, type ControlAccessMode, type PublicRelayPort, type PublicRelayState, type PublicRelayStatus, type PublicRelayUsage, type QcDeviceTransport, type QcStateUpdate } from "@qc-remote/core";
import { QC_BRAND } from "@qc-remote/theme";
import type { VirtualDeviceStorage } from "@qc-remote/ui";
import { QCIosNativeBridge } from "../native-bridge";
import { confirmIosWorkflow } from "./workflow-confirmations";
import { RemoteTraffic } from "./remote-traffic";
import { pairedHostConnection, readPairedHostTarget } from "./paired-host-target";

const endpoint = QC_BRAND.publicRelayEndpoint.replace(/\/$/, "");
const serverUrl = `${endpoint}/mcp`;
const callbackUrl = "qcremote://oauth/callback";
const storageKey = "qcr-ios-oauth-v1";
type Stored = { client?: OAuthClientInformationMixed; tokens?: OAuthTokens; verifier?: string; state?: string; discovery?: unknown };
const listeners = new Map<string, Set<(value: any) => void>>();
const directStateFrames: Array<{ streamId?: string; sequence: number; observedAt: number; states: unknown[]; tempoClock?: Record<string, unknown> }> = [];
let stored: Stored = {};
let loaded = false;
let loading: Promise<void> | undefined;
let client: Client | undefined;
let transport: StreamableHTTPClientTransport | undefined;
let authPending = false;
let mcpConnected = false;
let appActive = true;
const cloudActive = () => appActive && document.visibilityState !== "hidden";
const remoteTraffic = new RemoteTraffic(Date.now, cloudActive);
let connectionGeneration = 0;
let connectionTask: { generation: number; promise: Promise<void> } | undefined;
let mode: ControlAccessMode = parseControlAccessMode(localStorage.getItem("qc-control.device-access-mode-v1"));
let lastStatus: any;
let acceptedRemoteContext: { target?: BrokerTarget; guardSupported: boolean } | undefined;
export const clearRemoteTarget = () => { acceptedRemoteContext = undefined; };
let cursor = 0;
let directIpadSession = false;
const iosConnectionModeKey = "qc.remote.ios.connection-mode-v1";

export type IosConnectionMode = "midi" | "auto" | "remote";

export function getIosConnectionMode(): IosConnectionMode | undefined {
  // This App Store release supports paired hosts only. Ignore legacy direct
  // route preferences without changing credentials or the stored preference.
  return "remote";
}

function prefersRemoteHost(): boolean { return true; }

async function load(): Promise<void> {
  if (loaded) return;
  if (loading) return loading;
  const pending = (async () => {
    const raw = (await QCIosNativeBridge.secureGet({ key: storageKey })).value;
    const value: unknown = raw == null ? {} : JSON.parse(raw);
    if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("The saved sign-in data is invalid.");
    stored = value as Stored;
    loaded = true;
  })();
  loading = pending;
  try { await pending; } finally { if (loading === pending) loading = undefined; }
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

function retireMcpAttempt(): void {
  connectionGeneration++;
  connectionTask = undefined;
}
function connectMcp(): Promise<void> {
  if (connectionTask) return connectionTask.promise;
  if (mcpConnected && client) return Promise.resolve();
  const generation = ++connectionGeneration;
  const promise = establishMcp(generation);
  const task = { generation, promise };
  connectionTask = task;
  const finished = () => { if (connectionTask === task) connectionTask = undefined; };
  void promise.then(finished, finished);
  return promise;
}
async function awaitDemandConnection(): Promise<void> {
  const pending = connectMcp();
  const generation = connectionGeneration;
  await pending;
  if (generation !== connectionGeneration) throw new Error("The paired-host connection changed before this request completed. Refresh before editing.");
  if (!mcpConnected) throw new Error(authPending ? "Finish QC Remote Cloud sign-in in the browser to connect the paired host." : "Refresh the paired host to continue. Your sign-in is saved.");
}
async function establishMcp(generation: number): Promise<void> {
  if (Capacitor.isNativePlatform() && !prefersRemoteHost()) throw new Error("Choose Remote mode before connecting to QC Remote Cloud.");
  if (generation !== connectionGeneration || client || !cloudActive()) return;
  await load();
  if (generation !== connectionGeneration || client || !cloudActive()) return;
  mcpConnected = false;
  const nextClient = new Client({ name: "QC Remote for iOS", version: "0.1.0" });
  const nextTransport = new StreamableHTTPClientTransport(new URL(serverUrl), { authProvider: provider, fetch: (input, init) => remoteTraffic.fetch(input, init), reconnectionOptions: { initialReconnectionDelay: 5_000, maxReconnectionDelay: 300_000, reconnectionDelayGrowFactor: 2, maxRetries: 0 } });
  nextClient.onclose = () => {
    if (transport !== nextTransport) return;
    const established = mcpConnected;
    retireMcpAttempt();
    client = undefined; transport = undefined;
    mcpConnected = false;
    // A cold demand owns its failure feedback. Invalidating its native UI
    // attempt here would discard that error before the refresh can show it.
    if (established) emit("qcConnection", { state: "disconnected" });
    emit("relayState", { state: "reconnecting" });
  };
  client = nextClient; transport = nextTransport;
  try {
    await nextClient.connect(nextTransport);
    if (generation !== connectionGeneration || client !== nextClient || !cloudActive()) { await nextClient.close().catch(() => undefined); return; }
    authPending = false;
    const observedStatus = await callTool("get_qc_status", {});
    if (generation !== connectionGeneration || client !== nextClient || !cloudActive()) return;
    pairedHostConnection(observedStatus);
    lastStatus = observedStatus;
    mcpConnected = true;
    // Paired-host reads are explicit demand. Publishing availability here
    // starts a second mobile connection attempt during the initiating refresh.
    cursor = 0;
  } catch (error) {
    if (client !== nextClient || transport !== nextTransport) return;
    mcpConnected = false;
    if (error instanceof UnauthorizedError) { authPending = true; return; }
    client = undefined; transport = undefined;
    await nextClient.close().catch(() => undefined);
    throw error;
  }
}
let lastRemoteObservationAt: number | undefined;
export const remoteObservation = () => lastRemoteObservationAt;
export const subscribeRemoteObservation = (listener: (value: number) => void) => addListener("remoteObservation", listener);

async function callTool(name: string, args: Record<string, unknown>, observe = true): Promise<any> {
  if (!client) throw new Error("Connect or refresh the paired host to continue. Your sign-in is saved.");
  const result = parseAppMcpResult(await client.callTool({ name, arguments: args }));
  if (observe && name === "get_current_preset") {
    lastRemoteObservationAt = Date.now();
    emit("remoteObservation", lastRemoteObservationAt);
  }
  return result;
}

/** Explicit user refresh, never called by a polling timer. Ordinary gateway
 * reads also stay direct: last-observed UI state is not an authoritative cache. */
export const remoteConnection = () => lastStatus ? pairedHostConnection(lastStatus) : undefined;
export async function refreshRemoteTargetObservation() {
  if (!mcpConnected) await awaitDemandConnection();
  const result = await readPairedHostTarget({
    callTool: (name, args) => callTool(name, args, false),
    onStatus: status => { lastStatus = status; }
  });
  if (result.snapshot) {
    lastRemoteObservationAt = Date.now();
    emit("remoteObservation", lastRemoteObservationAt);
  }
  const context = {
    target: result.connection.brokerTarget ? Object.freeze({ ...result.connection.brokerTarget }) : undefined,
    guardSupported: Array.isArray(lastStatus?.capabilities) && lastStatus.capabilities.includes("mcpBrokerTargetGuard")
  };
  return { ...result, accept: () => { acceptedRemoteContext = result.connection.phase === "ready" ? context : undefined; } };
}
export async function refreshRemoteObservation(): Promise<PresetSnapshot> {
  const result = await refreshRemoteTargetObservation();
  if (!result.snapshot) throw new Error(result.connection.detail);
  return result.snapshot;
}
async function invokeRemote(method: string, args: Record<string, unknown>, context: typeof acceptedRemoteContext): Promise<any> {
  const action = SHARED_QC_ACTIONS.find(candidate => candidate.rpc === method);
  const guardSupported = Boolean(context?.guardSupported && Array.isArray(lastStatus?.capabilities) && lastStatus.capabilities.includes("mcpBrokerTargetGuard"));
  if (action && action.classification !== "read") {
    if (lastStatus?.brokerTarget && !context?.target) throw new Error("Refresh the paired host before editing; its selected target has not been accepted.");
    if (context?.target && !guardSupported) throw new Error("The cloud service cannot safely bind this write to the selected device. Update QC Remote Cloud before editing.");
  }
  const result = await invokeAppMcp(method, args, {
    accessMode: mode,
    expectedBrokerTarget: context?.target,
    brokerTargetGuardSupported: guardSupported,
    confirm: confirmIosWorkflow,
    onStatus: status => { pairedHostConnection(status); lastStatus = status; },
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
    const observed = mcpConnected ? remoteConnection() : undefined;
    const ready = observed?.phase === "ready";
    const name = observed?.brokerTarget?.source === "demo" ? "Demo device via paired host" : "Quad Cortex via paired host";
    return { devices: ready ? [{ deviceId: 1, name, permission: true, interfaces: 1 }] : [], connected: ready, synchronized: ready };

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
    const observed = remoteConnection();
    if (observed?.phase !== "ready") throw new Error(observed?.detail ?? "The paired host has no available device. Your sign-in is saved.");
    return { connected: true, synchronized: true, name: observed.brokerTarget?.source === "demo" ? "Demo device via paired host" : "Quad Cortex via paired host", deviceId: 1 };

  },
  async disconnect() {
    retireMcpAttempt();
    clearRemoteTarget();
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
    const observed = mcpConnected ? remoteConnection() : undefined;
    return { connected: observed?.phase === "ready", synchronized: observed?.phase === "ready", phase: observed?.phase ?? (authPending ? "connecting" : "disconnected"), device: observed?.brokerTarget?.source === "demo" ? "Paired host demo" : "QC Remote Cloud", messagesReceived: 0, messagesSent: 0, decodeErrors: 0, expectedWriteStalls: 0, lastMessageType: 0, connectedAt: 0, setlistKnown: false, presetPosition: 0, modelCount: 0, readAttempts: 0, negativeReads: 0, interfaceId: -1, inputEndpointAddress: 0, inputMaxPacketSize: 0, reportBytes: 0, midiAvailable: false, midiInterfaceId: -1, midiOutputEndpointAddress: 0, lastMidiQueueDelayMs: 0, maxMidiQueueDelayMs: 0, lastHidWriteDurationMs: 0, maxHidWriteDurationMs: 0, lastStateAt: Date.now(), writesQuarantined: false };
  },
  async gatewayInvoke({ method, params }: { method: string; params?: Record<string, unknown> }) {
    const capturedRemoteContext = acceptedRemoteContext;
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
    if (!mcpConnected && stored.tokens && !authPending) await awaitDemandConnection();
    if (!mcpConnected) throw new Error(authPending ? "Finish QC Remote Cloud sign-in in the browser to connect the paired host." : "Refresh the paired host to continue. Your sign-in is saved.");
    return invokeRemote(method, params ?? {}, capturedRemoteContext);
  },
  addListener
};
export const QcUsbNative = nativePlugin as any;
export const androidGatewayTransport = withDeviceConnectionSource(createGatewayClientTransport<GatewayTransport>(<T,>(method: string, params?: Record<string, unknown>) => nativePlugin.gatewayInvoke({ method, params }) as Promise<T>, "rpc"));
export async function androidUsbRuntimeDiagnostics(): Promise<NonNullable<RuntimeStatus["usbDiagnostics"]> & { android: any }> {
  const android = await nativePlugin.diagnostics();
  const physicalUsb = directIpadSession || !prefersRemoteHost() ? undefined : lastStatus?.usbDiagnostics;
  const hostRelay = physicalUsb?.updaterRelay;
  const relayDetail = hostRelay && typeof hostRelay.enabled === "boolean"
    ? ` The paired host's QC internet forwarding is ${hostRelay.enabled ? "enabled" : "disabled"}.`
    : " Forwarding for QC account, cloud, and update requests depends on the paired host's QC Remote version and settings.";
  return {
    ...(physicalUsb ?? {}),
    phase: directIpadSession ? android.phase : physicalUsb?.phase ?? "disconnected",
    detail: directIpadSession
      ? "Connected directly through the iPad DriverKit prototype. This direct iOS path does not provide the QC internet request forwarder; use Paired host mode for QC account, cloud, and update requests."
      : !prefersRemoteHost() ? "No direct Quad Cortex USB session is active."
        : mcpConnected ? `Connected through the QC Remote paired host.${relayDetail}`
          : "Sign in to QC Remote Cloud to reach a paired host.",
    connected: directIpadSession ? android.connected : Boolean(physicalUsb?.connected),
    synchronized: directIpadSession ? android.synchronized : Boolean(physicalUsb?.synchronized),
    android
  };
}
// Compatibility exports for the shared mobile shell. iOS never hosts the
// Android native gateway; its paired-host observation/acceptance is separate.
export function acceptAndroidGatewayBrokerTarget(_target: BrokerTarget | undefined): void {
  throw new Error("Android physical gateway acceptance is unavailable on iOS.");
}
export async function readAndroidGatewayTarget(): Promise<never> {
  throw new Error("Android physical gateway discovery is unavailable on iOS.");
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
  if (mode !== "remote") throw new Error("This release supports paired-host control only.");
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
  retireMcpAttempt();
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
