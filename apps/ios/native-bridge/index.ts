import { registerPlugin, type PluginListenerHandle } from "@capacitor/core";

export interface QCIosNativeBridgePlugin {
  secureGet(options: { key: string }): Promise<{ value: string | null }>;
  secureSet(options: { key: string; value: string }): Promise<void>;
  secureRemove(options: { key: string }): Promise<void>;
  setScreenAwake(options: { enabled: boolean }): Promise<{ enabled: boolean }>;
  getMidiInputChannel(): Promise<{ channel: number }>;
  setMidiInputChannel(options: { channel: number }): Promise<{ channel: number }>;
  platformCapabilities(): Promise<{ deviceFamily: "iphone" | "ipad"; supportsUsbMidi: boolean; sharedProtocolRuntimeBundled: boolean; driverHardwareSupported: boolean; driverOsSupported: boolean; fullUsbDriverBundled: boolean; fullUsbReady: boolean; fullUsbRequirement: string; driverCompatibilityNote: string; driverEntitlementRequired: boolean }>;
  scanUsbMidi(): Promise<{ devices: Array<{ deviceId: number; name: string; manufacturer: string; connected: boolean }> }>;
  connectUsbMidi(options: { deviceId: number }): Promise<{ connected: boolean; deviceId: number; name: string }>;
  disconnectUsbMidi(): Promise<void>;
  sendUsbMidiControlChange(options: { channel: number; controller: number; value: number }): Promise<{ accepted: boolean; verified: boolean; transport: "usb-midi" }>;
  sendUsbMidiProgramChange(options: { channel: number; bankMsb: number; bankLsb: number; program: number }): Promise<{ accepted: boolean; verified: boolean; transport: "usb-midi" }>;
  scanUsbDriver(): Promise<{ available: boolean; connected?: boolean; synchronized?: boolean; reason?: string }>;
  connectUsbDriver(): Promise<{ connected: boolean; synchronized: boolean; deviceId: number; name: string; diagnostics: Record<string, number> }>;
  disconnectUsbDriver(): Promise<void>;
  usbDriverDiagnostics(): Promise<Record<string, number>>;
  gatewayReadUsb(options: { method: string; params?: Record<string, unknown> }): Promise<unknown>;
  gatewayWriteUsb(options: { method: string; params?: Record<string, unknown> }): Promise<unknown>;
  driverCurrentPreset(): Promise<{ snapshot: Record<string, unknown> | null }>;
  addListener(eventName: "qcStateBatch", listener: (frame: { sequence: number; observedAt: number; states: unknown[]; tempoClock?: Record<string, unknown> }) => void): Promise<PluginListenerHandle>;
  addListener(eventName: "qcConnection", listener: (status: { state: "available" | "disconnected"; name?: string; synchronized?: boolean; detail?: string }) => void): Promise<PluginListenerHandle>;
}

export const QCIosNativeBridge = registerPlugin<QCIosNativeBridgePlugin>("QCIosNativeBridge");
