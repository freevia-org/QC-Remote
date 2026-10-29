import { registerPlugin, type PluginListenerHandle } from "@capacitor/core";
import { createGatewayClientTransport, withDeviceConnectionSource, type GatewayTransport, type NativeStateFrame, type PresetSnapshot, type QcDeviceSuspension, type RuntimeStatus } from "@qc-remote/client";
import { createQcGatewayTransport, type ControlAccessMode, type PublicRelayPort, type PublicRelayState, type PublicRelayStatus, type PublicRelayUsage, type QcDeviceTransport, type QcStateUpdate } from "@qc-remote/core";
import { QC_BRAND } from "@qc-remote/theme";
import type { VirtualDeviceStorage } from "@qc-remote/ui";
import { readAndroidPhysicalTarget, type AndroidGatewayRuntimeStatus } from "./android-gateway-target.ts";
import { createAndroidGatewayContext } from "./android-gateway-context.ts";

export type { QcStateUpdate } from "@qc-remote/core";

export type QcUsbDevice = {
  deviceId: number;
  name: string;
  manufacturer?: string;
  permission: boolean;
  interfaces: number;
};

export type AndroidUsbConnectionProgress = {
  requestId: string;
  attempt?: number;
  phase: "searching" | "permission" | "opening" | "claiming" | "reader" | "handshake" | "synchronizing" | "ready" | "error" | "disconnected";
  message: string;
  errorCode?: string;
};

export type AndroidUsbDiagnostics = {
  connected: boolean; phase: string; synchronized: boolean; device: string;
  messagesReceived: number; messagesSent: number; messagesReceivedByType?: Record<string, number>;
  messagesSentByType?: Record<string, number>; decodeErrors: number; expectedWriteStalls: number;
  lastMessageType: number; connectedAt: number; setlistKnown: boolean; presetPosition: number;
  modelCount: number; readAttempts: number; negativeReads: number; interfaceId: number;
  inputEndpointAddress: number; inputMaxPacketSize: number; reportBytes: number; midiAvailable: boolean;
  midiInterfaceId: number; midiOutputEndpointAddress: number; lastMidiQueueDelayMs: number;
  maxMidiQueueDelayMs: number; lastHidWriteDurationMs: number; maxHidWriteDurationMs: number;
  lastStateAt: number; writesQuarantined: boolean; writeQuarantineReason?: string;
  deviceSuspended?: boolean; suspension?: QcDeviceSuspension;
  flightRecorder?: { version: number; entries: Array<{ atUnixMs: number; processId: number; event: string; messageType?: number; reportCount?: number }> };
  lastError?: string;
  lastStartupFailure?: string;
  lastStartupFailureSessionEpoch?: number;
};

interface QcUsbNativePlugin {
    selectBrokerTarget(options: { source: "demo" | "physical" }): Promise<{ source: "demo" | "physical"; revision: number }>;
    brokerTargetSelection(): Promise<{ source: "demo" | "physical"; revision: number }>;
  publishDemoProvider(options: { sessionId: string | null; generation: number }): Promise<void>;
  admitDemoRequest(options: { id: string; sessionId: string; generation: number; brokerTarget?: AndroidDemoRequest["brokerTarget"] }): Promise<{ admitted: boolean }>;
  completeDemoRequest(options: { id: string; sessionId: string; generation: number; result?: Record<string, unknown>; error?: { code: string; message: string } }): Promise<void>;
  addListener(eventName: "qcDemoRequest", listener: (request: AndroidDemoRequest) => void): Promise<PluginListenerHandle>;
  internetForwarding(): Promise<{ enabled: boolean }>;
  setInternetForwarding(options: { enabled: boolean }): Promise<{ enabled: boolean }>;
  scan(): Promise<{ devices: QcUsbDevice[]; connected: boolean; synchronized: boolean }>;
  connect(options?: { allowPermissionPrompt?: boolean; requestId?: string }): Promise<{ connected: boolean; synchronized: boolean; name: string; deviceId: number }>;
  disconnect(): Promise<void>;
  diagnostics(): Promise<AndroidUsbDiagnostics>;
  gatewayInvoke<T>(options: { method: string; params?: Record<string, unknown>; expectedState?: Record<string, unknown>; expectedBrokerTarget?: AndroidDemoRequest["brokerTarget"]; requireAcceptedBrokerTarget?: boolean }): Promise<T>;
  addListener(eventName: "qcStateBatch", listener: (frame: NativeStateFrame<QcStateUpdate>) => void): Promise<PluginListenerHandle>;
  addListener(eventName: "qcConnection", listener: (status: { state: "available" | "disconnected"; name?: string }) => void): Promise<PluginListenerHandle>;
  addListener(eventName: "qcConnectionProgress", listener: (progress: AndroidUsbConnectionProgress) => void): Promise<PluginListenerHandle>;
  addListener(eventName: "qcStartupFailure", listener: (failure: { sessionEpoch: number; detail: string }) => void): Promise<PluginListenerHandle>;
}

export interface AndroidDemoRequest {
  id: string;
  sessionId: string;
  generation: number;
  sequence: number;
  brokerTarget: { source: "physical" | "demo"; sessionId: string; generation: number };
  method: string;
  params: Record<string, unknown>;
  expectedState: Record<string, unknown>;
}

interface ScreenWakeNativePlugin {
  setEnabled(options: { enabled: boolean }): Promise<{ enabled: boolean }>;
}

interface DemoDeviceStorageNativePlugin {
  read(): Promise<{ text: string | null }>;
  write(options: { text: string }): Promise<void>;
}

interface QcRelayNativePlugin {
  openAccountDeletionPage(options: { endpoint: string }): Promise<{ opened: boolean }>;
  status(): Promise<PublicRelayStatus>;
  usage(): Promise<PublicRelayUsage>;
  pair(options: { endpoint: string; pairingCode: string; deviceName?: string }): Promise<{ paired: boolean; endpoint: string }>;
  beginBrowserPairing(options: { endpoint: string }): Promise<{ opened: boolean; expiresAt: number }>;
  pollBrowserPairing(): Promise<{ state: "waiting" | "pairing" | "paired"; expiresAt?: number; paired?: boolean; endpoint?: string }>;
  start(): Promise<void>;
  setAccessMode(options: { mode: ControlAccessMode }): Promise<{ accessMode: ControlAccessMode }>;
  unpair(): Promise<{ warning?: string }>;
  addListener(eventName: "relayState", listener: (result: { state: PublicRelayState }) => void): Promise<PluginListenerHandle>;
}

export const QcUsbNative = registerPlugin<QcUsbNativePlugin>("QcUsb");

/** Adapt Android's native diagnostics to the runtime contract shared with Windows. */
export async function androidUsbRuntimeDiagnostics(): Promise<NonNullable<RuntimeStatus["usbDiagnostics"]> & { android: AndroidUsbDiagnostics }> {
  const android = await QcUsbNative.diagnostics();
  const phase = android.phase;
  const detail = android.lastError ?? (android.writesQuarantined
    ? android.writeQuarantineReason ?? "Writes are paused until the USB session is reset."
    : phase === "ready" ? "Quad Cortex is synchronized."
      : phase === "syncing" ? "Quad Cortex is synchronizing."
        : phase === "connecting" ? "Quad Cortex is connecting."
          : "Quad Cortex is disconnected.");
  return {
    phase, detail, connected: android.connected, synchronized: android.synchronized,
    writesQuarantined: android.writesQuarantined, writeQuarantineReason: android.writeQuarantineReason,
    deviceSuspended: android.deviceSuspended, suspension: android.suspension, android
  };
}
export const ScreenWakeNative = registerPlugin<ScreenWakeNativePlugin>("ScreenWake");
export const DemoDeviceStorageNative = registerPlugin<DemoDeviceStorageNativePlugin>("DemoDeviceStorage");
export const QcRelayNative = registerPlugin<QcRelayNativePlugin>("QcRelay");
export const subscribeRelayState = (listener: (state: PublicRelayState) => void) =>
  QcRelayNative.addListener("relayState", ({ state }) => listener(state));

export const publicRelay: PublicRelayPort = {
  status: () => QcRelayNative.status(),
  usage: () => QcRelayNative.usage(),
  async pair(endpoint, pairingCode, deviceName = "QC Remote on Android") {
    await QcRelayNative.pair({ endpoint, pairingCode, deviceName });
    return QcRelayNative.status();
  },
  start: () => QcRelayNative.start(),
  async unpair() {
    const { warning } = await QcRelayNative.unpair();
    const status = await QcRelayNative.status();
    return warning ? { ...status, detail: warning } : status;
  },
  async setAccessMode(mode) {
    await QcRelayNative.setAccessMode({ mode });
    return QcRelayNative.status();
  },
  browserPairing: {
    async begin() {
      return QcRelayNative.beginBrowserPairing({ endpoint: QC_BRAND.publicRelayEndpoint });
    },
    async poll() {
      const result = await QcRelayNative.pollBrowserPairing();
      const paired = result.state === "paired" || result.paired === true;
      return {
        state: paired ? "paired" as const : "waiting" as const,
        ...(paired ? { status: await QcRelayNative.status() } : {})
      };
    }
  },
};

const androidGatewayContext = createAndroidGatewayContext(<T,>(request: Parameters<QcUsbNativePlugin["gatewayInvoke"]>[0]) => QcUsbNative.gatewayInvoke<T>(request));

/** Commit only a UI-adopted physical context; status discovery never changes it. */
export function acceptAndroidGatewayBrokerTarget(target: AndroidDemoRequest["brokerTarget"] | undefined): void {
  androidGatewayContext.accept(target);
}

export function readAndroidGatewayTarget() {
  return readAndroidPhysicalTarget({
    status: () => QcUsbNative.gatewayInvoke<AndroidGatewayRuntimeStatus>({ method: "system.status" }),
    snapshot: (target) => createGatewayClientTransport<GatewayTransport>(
      <T,>(method: string, params?: Record<string, unknown>) => QcUsbNative.gatewayInvoke<T>({ method, params, expectedBrokerTarget: target }),
      "rpc"
    ).currentSnapshot(),
    accept: acceptAndroidGatewayBrokerTarget
  });
}

export const androidGatewayTransport = withDeviceConnectionSource(createGatewayClientTransport<GatewayTransport>(
  androidGatewayContext.invoke,
  "rpc"
));

export const virtualDeviceStorage: VirtualDeviceStorage = {
  async read() { return (await DemoDeviceStorageNative.read()).text; },
  write(text) { return DemoDeviceStorageNative.write({ text }); }
};

/** Android and Windows deliberately share the exact UI transport adapter. */
export function createAndroidQcTransport(gateway: GatewayTransport, currentSnapshot: () => PresetSnapshot): QcDeviceTransport {
  return createQcGatewayTransport(gateway, currentSnapshot);
}

/** Browser navigation only; no credential mutation or deletion request. */
export async function openRemoteAccountDeletionPage(): Promise<void> {
  await QcRelayNative.openAccountDeletionPage({ endpoint: QC_BRAND.publicRelayEndpoint });
}
