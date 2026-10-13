import type { ControlAccessMode } from "./control-access.ts";

export type PublicRelayState =
  | "stopped"
  | "connecting"
  | "connected"
  | "reconnecting"
  | "pairing_required"
  | "invalid_endpoint";

/** The authenticated browser identity captured at the time this host was paired. */
export interface PublicRelayPairing {
  email: string;
  device: string;
  pairedAt: number;
}

export interface PublicRelayStatus {
  paired: boolean;
    /** This installation's persisted outbound relay-session switch. */
    remoteEnabled?: boolean;
  /** A previous identity system paired this host. It cannot access a Google-owned relay until paired again. */
  needsRepair?: boolean;
  state: PublicRelayState;
  accessMode: ControlAccessMode;
  endpoint?: string;
  deviceId?: string;
  pairing?: PublicRelayPairing;
  /** Why the last operation could not be completed as asked. Set when an
   * unpair cleared this host locally but could not reach the relay to withdraw
   * the credential, so the person can retry rather than assume it is gone. */
  detail?: string;
}

/** One device paired to the same account as this host.
 *
 * Display-only. It carries nothing the person did not already see when they
 * paired that device, and never a credential. A host knows its own pairing
 * without asking anyone; every other entry comes from the relay, so this is
 * absent entirely on a deployment that cannot list them. */
export interface PublicRelayDevice {
  deviceId: string;
  name: string;
  pairedAt: number;
  expiresAt: number;
  /** The credential lapsed. Reported rather than hidden: it is the reason
   * remote access stopped, and removing the row answers that with nothing. */
  expired?: boolean;
  connected?: boolean;
  /** This host. The one row the panel must not offer a remote unpair for. */
  current?: boolean;
  email?: string;
  signedInFrom?: string;
}

/** One durable OAuth authorization granted to a remote MCP client. */
export interface PublicRelayConnector {
  grantId: string;
  clientId: string;
  name: string;
  scopes: string[];
  authorizedAt: number;
  expiresAt?: number;
  authorizedFrom?: string;
  device?: string;
  browser?: string;
  ipAddress?: string;
  country?: string;
  city?: string;
  clientUri?: string;
  /** A live MCP lease exists at the instant the roster is read. */
  active?: boolean;
  /** Most recent authenticated MCP request observed for this grant. */
  lastActiveAt?: number;
  appVersion?: string;
  /** Activity-route metadata describes the provider connection reaching the
   * relay. It may be provider infrastructure rather than the user's phone. */
  activityDevice?: string;
  activityBrowser?: string;
  activityIpAddress?: string;
  activityCountry?: string;
  activityCity?: string;
}

export interface PublicRelayUsage {
  day: string;
  commands: number;
  lastCommandAt?: number;
  platformUsage?: string;
}

export interface PublicRelayBrowserPairingOffer {
  /** Present on Windows, where the person may copy or share the pairing URL. */
  pairUrl?: string;
  expiresAt: number;
}

export interface PublicRelayBrowserPairingProgress {
  state: "waiting" | "paired";
  status?: PublicRelayStatus;
}

export interface PublicRelayBrowserPairingPort {
  begin(): Promise<PublicRelayBrowserPairingOffer>;
  poll(deviceName?: string): Promise<PublicRelayBrowserPairingProgress>;
  cancel?(): Promise<void>;
  /** Available on desktop, whose native bridge validates the URL before opening it. */
  openPairUrl?(pairUrl: string): Promise<void>;
}

/** Platform-neutral control plane for the authenticated outbound relay. */
export interface PublicRelayPort {
  status(): Promise<PublicRelayStatus>;
  usage(): Promise<PublicRelayUsage>;
  pair(endpoint: string, pairingCode: string, deviceName?: string): Promise<PublicRelayStatus>;
  start(): Promise<void>;
  unpair(): Promise<PublicRelayStatus>;
  setAccessMode(mode: ControlAccessMode): Promise<PublicRelayStatus>;
  browserPairing?: PublicRelayBrowserPairingPort;
  /** Optional for adapters that target a relay host predating this preference. */
  /** Every device on this account. Optional: a host that cannot ask - the
   * mobile host, or one pointed at a relay older than the roster - simply does
   * not have it, and its panel falls back to the pairing it holds locally. */
  devices?(): Promise<PublicRelayDevice[]>;
  /** Retire one of the *other* devices, returning what is left. Unpairing this
   * host stays `unpair`, which also clears the local credential store. */
  unpairDevice?(deviceId: string): Promise<PublicRelayDevice[]>;
  connectors?(): Promise<PublicRelayConnector[]>;
  unlinkConnector?(grantId: string): Promise<PublicRelayConnector[]>;
}
