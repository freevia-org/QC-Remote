import type { ConnectionState, GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import { createVirtualGatewayTransport, VirtualDeviceStore, type VirtualDeviceStorage } from "./virtual-gateway.ts";
import type { MeasuredParameterDefaults, VirtualModelCatalogue } from "./virtual-device-defaults.ts";

/** Runtime resources bundled by every QC Remote host that offers the demo device. */
export const DEMO_PARAMETER_SCALES_URL = "./reference/qc-live-parameter-scales.json";
export const DEMO_PARAMETER_DEFAULTS_URL = "./reference/qc-live-parameter-defaults.json";

let catalogue: Promise<VirtualModelCatalogue> | undefined;
let measuredDefaults: Promise<MeasuredParameterDefaults> | undefined;

/** Lazy-load the large model catalogue once per app run. A failed read is retryable. */
export function loadDemoModelCatalogue(
  fetchDocument: (url: string) => Promise<Response> = (url) => fetch(url)
): Promise<VirtualModelCatalogue> {
  catalogue ??= (async () => {
    const response = await fetchDocument(DEMO_PARAMETER_SCALES_URL);
    if (!response.ok) throw new Error(`The demo device's model catalogue could not be read (${response.status}).`);
    const document = await response.json() as Partial<VirtualModelCatalogue>;
    if (!Array.isArray(document.models)) throw new Error("The demo device's model catalogue is malformed.");
    return document as VirtualModelCatalogue;
  })().catch((error) => {
    catalogue = undefined;
    throw error;
  });
  return catalogue;
}

/** Lazy-load the measured factory defaults beside the catalogue. */
export function loadDemoParameterDefaults(
  fetchDocument: (url: string) => Promise<Response> = (url) => fetch(url)
): Promise<MeasuredParameterDefaults> {
  measuredDefaults ??= (async () => {
    const response = await fetchDocument(DEMO_PARAMETER_DEFAULTS_URL);
    if (!response.ok) throw new Error(`The demo device's measured parameter defaults could not be read (${response.status}).`);
    const document = await response.json() as Partial<MeasuredParameterDefaults>;
    if (document.source !== "device-derived compatibility data" || document.models === null || typeof document.models !== "object" || Array.isArray(document.models)) {
      throw new Error("The demo device's measured parameter defaults are malformed.");
    }
    return document as MeasuredParameterDefaults;
  })().catch((error) => {
    measuredDefaults = undefined;
    throw error;
  });
  return measuredDefaults;
}

/** Open the same persisted virtual Quad Cortex on every supported host. */
export async function openDemoDevice(
  storage: VirtualDeviceStorage,
  loadCatalogue: () => Promise<VirtualModelCatalogue> = loadDemoModelCatalogue,
  loadDefaults: () => Promise<MeasuredParameterDefaults> = loadDemoParameterDefaults
): Promise<GatewayTransport> {
  const [catalogueDocument, defaultsDocument] = await Promise.all([loadCatalogue(), loadDefaults()]);
  const store = await VirtualDeviceStore.open({ storage, catalogue: catalogueDocument, measuredDefaults: defaultsDocument });
  return createVirtualGatewayTransport(store);
}

export type DemoDeviceSource = "device" | "virtual";

export interface DemoDeviceStartResult {
  transport: GatewayTransport;
  connection: ConnectionState;
  snapshot: PresetSnapshot;
}

/**
 * One stable gateway shared by the workflow hooks. Its target is selected at
 * call time, so switching hosts never rebuilds the editing surface or leaves
 * a callback pointing at the previous device.
 */
export interface DemoDeviceSession {
  readonly gateway: GatewayTransport;
  readonly source: DemoDeviceSource;
  readonly virtual: GatewayTransport | undefined;
  useDevice(): void;
  start(storage: VirtualDeviceStorage, shouldActivate?: () => boolean): Promise<DemoDeviceStartResult | undefined>;
}

export function createDemoDeviceSession(
  device: GatewayTransport,
  opener: (storage: VirtualDeviceStorage) => Promise<GatewayTransport> = openDemoDevice
): DemoDeviceSession {
  let source: DemoDeviceSource = "device";
  let virtualTransport: GatewayTransport | undefined;
  const active = (): GatewayTransport => {
    if (source === "device") return device;
    if (!virtualTransport) throw new Error("The demo device has not been opened.");
    return virtualTransport;
  };
  const methods = new Map<PropertyKey, (...args: unknown[]) => unknown>();
  const gateway = new Proxy({} as GatewayTransport, {
    get(_target, method) {
      if (typeof method === "symbol" || method === "then" || method === "toJSON") return undefined;
      let delegate = methods.get(method);
      if (!delegate) {
        delegate = (...args: unknown[]) => {
          const target = active() as unknown as Record<PropertyKey, unknown>;
          const implementation = target[method];
          if (typeof implementation !== "function") throw new TypeError(`The active gateway has no ${String(method)} method.`);
          return (implementation as (...values: unknown[]) => unknown).apply(target, args);
        };
        methods.set(method, delegate);
      }
      return delegate;
    }
  });
  return {
    gateway,
    get source() { return source; },
    get virtual() { return virtualTransport; },
    useDevice() { source = "device"; },
    async start(storage, shouldActivate = () => true) {
      // Do not switch routing until the virtual device has opened and answered
      // both readiness reads. A corrupt store therefore leaves a live QC alone.
      const transport = await opener(storage);
      const connection = await transport.reconnect();
      const snapshot = await transport.currentSnapshot();
      if (!shouldActivate()) return undefined;
      virtualTransport = transport;
      source = "virtual";
      return { transport, connection, snapshot };
    }
  };
}
