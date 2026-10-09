import type { PublicRelayConnector, PublicRelayDevice, PublicRelayPort, PublicRelayStatus, PublicRelayUsage } from "@qc-remote/core";
import { QC_BRAND } from "@qc-remote/theme";
import { callTauri } from "./tauri-invoke.ts";

export const publicRelay: PublicRelayPort = {
  status(): Promise<PublicRelayStatus> {
    return callTauri<PublicRelayStatus>("relay_status");
  },
  usage(): Promise<PublicRelayUsage> {
    return callTauri<PublicRelayUsage>("relay_usage");
  },
  pair(endpoint: string, pairingCode: string, deviceName = "QC Remote on Windows"): Promise<PublicRelayStatus> {
    return callTauri<PublicRelayStatus>("pair_public_relay", { endpoint, pairingCode, deviceName });
  },
  start(): Promise<void> {
    return callTauri<void>("start_public_relay");
  },
  unpair(): Promise<PublicRelayStatus> {
    return callTauri<PublicRelayStatus>("unpair_public_relay");
  },
  setAccessMode(mode): Promise<PublicRelayStatus> {
    return callTauri<PublicRelayStatus>("set_public_relay_access_mode", { mode });
  },
  browserPairing: {
    begin() {
      return callTauri<{ pairUrl: string; expiresAt: number }>("begin_public_relay_browser_pairing", { endpoint: QC_BRAND.publicRelayEndpoint });
    },
    poll(deviceName = "QC Remote on Windows") {
      return callTauri<{ state: "waiting" | "paired"; status?: PublicRelayStatus }>("poll_public_relay_browser_pairing", { deviceName });
    },
    cancel() {
      return callTauri<void>("cancel_public_relay_browser_pairing");
    },
    openPairUrl(pairUrl: string) {
      return callTauri<void>("open_public_relay_pair_url", { endpoint: QC_BRAND.publicRelayEndpoint, pairUrl });
    }
  },
  devices(): Promise<PublicRelayDevice[]> {
    return callTauri<PublicRelayDevice[]>("relay_devices");
  },
  unpairDevice(deviceId: string): Promise<PublicRelayDevice[]> {
    return callTauri<PublicRelayDevice[]>("unpair_public_relay_device", { deviceId });
  },
  connectors(): Promise<PublicRelayConnector[]> {
    return callTauri<PublicRelayConnector[]>("relay_connectors");
  },
  unlinkConnector(grantId: string): Promise<PublicRelayConnector[]> {
    return callTauri<PublicRelayConnector[]>("unlink_public_relay_connector", { grantId });
  }
};

/** Backward-compatible alias; new consumers use `publicRelay.browserPairing`. */
export const publicRelayBrowserPairing = publicRelay.browserPairing!;
