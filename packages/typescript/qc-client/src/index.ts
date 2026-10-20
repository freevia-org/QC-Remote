import type { BlockDetails, DeviceActionResult, GeneralSettings, GlobalEqSettings, IoSettings, LooperStatus, MidiOutMessage, ModeCycle, PresetSnapshot, TempoSettings, TunerSettings } from "./generated-payloads.ts";
export * from "./generated-domain.ts";
export * from "./generated-gateway-methods.ts";
export * from "./generated-payloads.ts";

import { QC_CONNECTION_PHASES, QC_BROKER_CONNECTION_PHASE_ALIASES, type ConnectionPhase } from "./generated-domain.ts";

/** Normalize both hosts' native phase vocabulary without hiding a fault as connecting. */
export function normalizeBrokerConnection(state: ConnectionState): ConnectionState {
  const phase = state.phase as string;
  const aliases: Readonly<Record<string, ConnectionPhase>> = QC_BROKER_CONNECTION_PHASE_ALIASES;
  const normalized = Object.hasOwn(aliases, phase) ? aliases[phase]
    : (QC_CONNECTION_PHASES as readonly string[]).includes(phase) ? phase as ConnectionPhase : "needs-attention";
  if (normalized !== phase || normalized === "searching") return { ...state, phase: normalized, demo: true };
  return state;
}

/**
 * The client gateway route: a native/paired-host gateway (`device`) or this
 * client's local virtual runtime (`virtual`). A paired host can itself serve
 * a demo without changing the client route; read brokerTarget for its actual
 * physical/demo target before describing the selected target as hardware.
 *
 * `demo` is not a source marker: editable virtual workflows use demo:false.
 */
export type ConnectionSource = "device" | "virtual";

/** The broker-served target, independent of the client's transport route.
 * A remote demo still uses the host/MCP (`source: "device"`) transport;
 * `source: "virtual"` continues to select this client's local demo runtime.
 * Older hosts omit this metadata; absence must not imply a demo target.
 */
export interface BrokerTarget {
  source: "physical" | "demo";
  /** Unique live provider session, including a broker/registration nonce. */
  sessionId: string;
  /** Changes on every effective target switch, including return to a prior session. */
  generation: number;
}

export interface ConnectionState {
  brokerTarget?: BrokerTarget;
  phase: ConnectionPhase;
  detail: string;
  lastSync?: string;
  demo: boolean;
  source: ConnectionSource;
  /**
   * Set by `reconnect` and `resetSession`: whether this call actually tore the
   * session down. The native broker deliberately leaves a healthy session
   * alone on an unforced reconnect and reports `false` rather than implying a
   * handshake it never performed. Absent on transports that always restart,
   * so treat anything other than an explicit `false` as a restart.
   */
  restarted?: boolean;
}

export interface TempoClockState {
  available: boolean;
  sequence?: number;
  receivedAtUnixMs?: number;
  enabled?: boolean;
  prerollEnabled?: boolean;
  currentBeat?: number;
  currentBar?: number;
  currentTick?: number;
  countInBeatsRemaining?: number;
  countInBarsRemaining?: number;
}

export interface NativeStateFrame<TState = unknown> {
  /** Producer identity, stable across its USB reconnects. Legacy hosts omit it. */
  streamId?: string;
  sequence: number;
  observedAt: number;
  states: TState[];
  tempoClock?: Omit<TempoClockState, "available" | "sequence" | "receivedAtUnixMs">;
}

export interface NativeStateFrames<TState = unknown> {
  native: boolean;
  streamId?: string;
  latestSequence?: number;
  frames: NativeStateFrame<TState>[];
}

/**
 * The device broker's update relay: the part of the host that carries the Quad
 * Cortex's own update requests to Neural DSP over this computer's internet
 * connection while the unit is connected over USB. Every field is optional
 * because an older broker omits the whole object. `system.status` is readable
 * by the AI apps attached to the broker, so this carries counts, the switch and
 * the outcome word of the last request, and no host, path, query, domain list
 * or timestamp.
 */
export interface UpdaterRelayDiagnostics {
  enabled?: boolean;
  requestsSeen?: number;
  /** Requests the unit made while the relay was off; they were not carried. */
  requestsWhileDisabled?: number;
  requestsCompleted?: number;
  requestsFailed?: number;
  requestsRefused?: number;
  /** Requests for a domain that is not allowed (outside neuraldsp.com and the extra domains). */
  refusedDomainRequests?: number;
  bytesFromServer?: number;
  bytesToDevice?: number;
  inFlight?: boolean;
  /** How the latest finished request ended: "completed", "timeout", "refused:domain-not-allowed", ... */
  lastOutcome?: string | null;
}

export interface RuntimeStatus {
  /** Effective broker target availability; USB diagnostics stay physical-only. */
  targetReady?: boolean;
  brokerTarget?: BrokerTarget;
  platform: string;
  gatewayAvailable: boolean;
  gatewayApiVersion?: number;
  capabilities?: string[];
  message: string;
  usbDiagnostics?: {
    phase: string;
    detail: string;
    connected: boolean;
    synchronized: boolean;
    activePresetName?: string;
    activeScene?: number;
    connectedAtUnixMs?: number;
    handshakeMs?: number;
    messagesReceived?: number;
    messagesSent?: number;
    messagesReceivedByType?: Record<string, number>;
    messagesSentByType?: Record<string, number>;
    lastMessageType?: number;
    updaterRelay?: UpdaterRelayDiagnostics;
    /** Inbound frames this connection received and could not act on, keyed by
     * reason and then by message type, plus the per-type tally of the two
     * device trailer bytes nothing on the host interprets.
     * `messagesReceivedByType` counts what the host understood; this counts
     * what it did not. */
    inboundTelemetry?: {
      frames?: number;
      /** Frames no consumer anywhere in the host could have used. The
       * handler-less count is reported separately, because a type the state
       * decoder ignores may still be read by another lane. */
      unactedFrames?: number;
      noStateHandlerFrames?: number;
      /** Trailer message types the protocol does not declare, keyed by the
       * raw 32-bit trailer value. */
      undeclaredTypes?: Record<string, number>;
      undeclaredTypeKeysDropped?: number;
      /** Declared types whose protobuf body did not decode. */
      decodeFailures?: Record<string, number>;
      /** Declared types that decoded and that no state handler claims. */
      noStateHandlerTypes?: Record<string, number>;
      encryptedRejections?: Record<string, number>;
      compressed?: Record<string, number>;
      inflateFailures?: Record<string, number>;
      deviceTrailerBytes?: {
        messageType: number;
        deviceBytes: [number, number];
        count: number;
      }[];
      deviceTrailerKeysDropped?: number;
      /** Messages lost part-way through assembly (a new FIRST report or an
       * ingress epoch change cut them off); never assembled, so not in
       * `frames`. */
      abandonedPartialFrames?: number;
      /** Continuation reports that arrived with no message open. */
      orphanedContinuationReports?: number;
      /** Reports the framing layer rejected. */
      framingErrors?: number;
      lastUnacted?: string | null;
    };
    /** Set after a mutation reached the wire but its outcome could not be
     * established. Reads remain available; later writes fail closed until a
     * session reset clears it. */
    writesQuarantined?: boolean;
    writeQuarantineReason?: string;
    /** The device asked the host to stop driving it while a backup or a reset
     * runs on the unit. Requests that would reach it are refused with
     * `DEVICE_SUSPENDED` until it resumes or the session ends. */
    deviceSuspended?: boolean;
    suspension?: QcDeviceSuspension;
  };
}

/** The session owner's record of the device's suspension of host control. */
export interface QcDeviceSuspension {
  /** Null unless the device currently holds the session suspended. */
  active: {
    /** BACKUP, FACTORY_RESET, SETTINGS_RESET, UNSPECIFIED or UNKNOWN_<n>. */
    reason: string;
    sinceUnixMs: number;
    refusedRequests: number;
  } | null;
  suspensionsObserved: number;
  refusedRequests: number;
  resyncsRequested: number;
  unmatchedResumes: number;
  lastEnded: {
    reason: string;
    sinceUnixMs: number;
    endedUnixMs: number;
    endedBy: "deviceResumed" | "screenUnlocked" | "sessionEnded";
    refusedRequests: number;
    resyncRequested: boolean;
    /** Reason on the device's resume frame; null until it has arrived. */
    resumeReason: string | null;
  } | null;
}

export interface ModelEntry {
  id: number;
  name: string;
  category: string;
  basedOn: string;
  pluginId?: number | null;
  sku?: string | null;
  available?: boolean;
}

export interface ModelCatalogAuditException {
  modelId: number;
  modelName: string;
  parameterIndex: number;
  issue: string;
}

export interface ModelCatalogAudit {
  modelCount: number;
  parameterCount: number;
  categoryCount: number;
  exceptions: ModelCatalogAuditException[];
}

export interface ModelList {
  models: ModelEntry[];
  audit: ModelCatalogAudit;
}

/**
 * The unit's version reply. `appFwVersion` is the Zenjack application firmware
 * build (the Device Information screen's "Zenjack FW app", e.g. `d14e`), NOT
 * the CorOS version number. The other firmware fields are what the reply
 * carries beyond what the gateway has so far been taught to pass on; each is
 * optional because a gateway that does not pass it on leaves it out.
 */
export interface DeviceIdentity {
  serial: string;
  appFwVersion?: string;
  customName?: string;
  deviceType?: number;
  macAddress?: string;
  linuxKernelVersion?: string;
  ubootVersion?: string;
  bootloaderFwVersion?: string;
  zencoderFwApp?: string;
  zencoderFwBootloader?: string;
  zenwirelessFwVersion?: string;
  /**
   * The CorOS version number. The version reply has no field for it: the host reads it from
   * the update request the unit sends (the `X-Cloud-Api-Hardware-Source-Version` header of
   * its type-61 message), so it is present only once the unit has sent one this session.
   */
  corOsVersion?: string;
}

export interface InhibitedModules {
  globalGate: boolean;
  globalEq: boolean;
}

export interface DeviceImage {
  pngBase64: string;
  width: number;
  height: number;
}

export interface PresetEntry {
  position: number;
  location: string;
  name: string;
  instrument: number;
  /** False when this directory row represents an empty preset slot. */
  occupied?: boolean;
  /** The preset's product key; the same key a Recent or Favorites row carries. */
  productKey?: string | null;
  /** Author metadata published by CorOS for this stored preset. */
  author?: string;
  /** Original date text published by CorOS. */
  date?: string;
  /** Preferred sortable device timestamp, in milliseconds since Unix epoch. */
  dateMsSinceEpoch?: number;
}

export interface PresetFolder {
  key: string;
  name: string;
  isFactory: boolean;
  /**
   * Position in which the Quad Cortex first published this folder in the current
   * session, counted from zero. The unit lists its user folders in that order.
   * Absent from brokers that predate it, and for folders whose arrival was not seen.
   */
  order?: number;
}

export interface PresetFolderList {
  folders: PresetFolder[];
  loading?: boolean;
}

export interface PresetList {
  setlistKey: string;
  setlistName: string;
  currentPosition: number;
  presets: PresetEntry[];
  folders: PresetFolder[];
  loading?: boolean;
}

export interface ParameterActionResult extends DeviceActionResult {
  block: BlockDetails;
}

export interface WorkspaceDocument {
  version: 1;
  savedAt: string;
  source: {
    deviceName: string;
    setlistKey: string;
    setlistName: string;
    presetPosition: number;
    presetLocation: string;
    presetName: string;
  };
  snapshot: PresetSnapshot;
  selectedBlock?: BlockDetails;
  ui: {
    selectedBlockId: string;
    formFactorId: string;
  };
}

export interface WorkspaceFileResult {
  cancelled: boolean;
  path?: string;
  name?: string;
  document?: WorkspaceDocument;
}

export interface DiagnosticsReport {
  generatedAt: string;
  appVersion: string;
  runtime: { platform: string; gatewayAvailable: boolean };
  connection: { phase: ConnectionPhase; demo: boolean; source: ConnectionSource };
  device: {
    presetLocation: string;
    presetPosition: number;
    mode: PresetSnapshot["mode"];
    activeScene: number;
    tempo: number;
    dirty: boolean;
    blockCount: number;
  };
  events: Array<{ at: string; event: string }>;
}

export interface PresetSlot extends PresetEntry {
  occupied: boolean;
}

export interface PresetSlotList {
  setlistKey: string;
  setlistName: string;
  currentPosition: number;
  slots: PresetSlot[];
}

export interface SavePresetResult extends DeviceActionResult {
  savedName: string;
}

export interface DuplicateSetlistResult extends DeviceActionResult {
  setlistKey: string;
}

export interface MasterVolumeState {
  value: number;
}

export interface LibraryEntry {
  name: string;
  key: string;
  author?: string;
  description?: string;
  date?: string;
  productKey?: string;
  folderKey?: string;
  folderName?: string;
  position?: number;
  instrument?: number;
  /** Neural Capture file version; version 2 maps to the CorOS Neural Capture v2 model. */
  /** The native file protocol publishes this as a string ("1" or "2"). */
  version?: number | string;
  isFactory: boolean;
  isPlugin: boolean;
}

export interface LibraryFolder {
  key: string;
  name: string;
  parentKey?: string;
  isFactory: boolean;
  isDownloads: boolean;
  isLibrary: boolean;
}

export interface LibraryEntries { entries: LibraryEntry[]; folders?: LibraryFolder[]; }
export interface PinnedModels { models: number[]; captures: string[]; }
export interface GraphicsTree { tree: string; }

export type LaneControl = "inputGate" | "laneOutput";

export type { GatewayTransport } from "./generated-gateway-transport.ts";
import type { GatewayTransport } from "./generated-gateway-transport.ts";

/**
 * Preserve an explicit connection source; legacy native device brokers omit
 * it and need the physical gateway default. Never derive source from `demo`:
 * virtual workflows also use demo:false. Host-selected target metadata, when
 * present, is separate from this gateway/provider source.
 */
export function withDeviceConnectionSource<T extends GatewayTransport>(transport: T): T {
  const stamp = async (state: Promise<ConnectionState>): Promise<ConnectionState> => {
    const connection = normalizeBrokerConnection(await state);
    if (connection.source !== undefined && connection.source !== "device" && connection.source !== "virtual") {
      throw new Error("The device connection source is unsupported. Refresh before editing.");
    }
    return { ...connection, source: connection.source ?? "device" };
  };
  return {
    ...transport,
    reconnect: () => stamp(transport.reconnect()),
    resetSession: () => stamp(transport.resetSession()),
    disconnect: () => stamp(transport.disconnect())
  };
}
/** Validate selected-target metadata before it becomes an operation fence.
 * Missing metadata is a legacy/unavailable host, never an inferred demo. */
export function brokerTargetKey(target: BrokerTarget | undefined): string | undefined {
  if (target === undefined) return undefined;
  if (!target || (target.source !== "physical" && target.source !== "demo")
      || typeof target.sessionId !== "string" || !target.sessionId.trim()
      || !Number.isSafeInteger(target.generation) || target.generation < 1) {
    throw new Error("The selected broker target identity is unsupported. Refresh before editing.");
  }
  return JSON.stringify([target.source, target.sessionId, target.generation]);
}

/** Require the model identifier observed with a parameter or Grid target. */
export function requireModelId(target: { modelId?: number | null } | undefined): number {
  const modelId = target?.modelId;
  if (!Number.isInteger(modelId) || modelId! < 1 || modelId! > 0x7fffffff) {
    throw new Error("The block model is unavailable. Refresh the device before editing it.");
  }
  return modelId!;
}

/** Use the identity captured with the caller's operands, never a later refresh. */
export function requirePresetIdentity(snapshot: { presetIdentity?: string }): string {
  if (!snapshot.presetIdentity) throw new Error("Refresh the Quad Cortex state before editing this preset; its device identity is unavailable.");
  return snapshot.presetIdentity;
}
