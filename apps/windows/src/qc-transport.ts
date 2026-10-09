import type { GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import { createQcGatewayTransport, type QcDeviceTransport } from "@qc-remote/core";

/**
 * Adapts the stable gateway.v1 domain API to the shared app transport.
 * The native Rust broker remains below the gateway and is deliberately
 * invisible here.
 */
export function createWindowsQcTransport(
  gateway: GatewayTransport,
  currentSnapshot: () => PresetSnapshot
): QcDeviceTransport {
  return createQcGatewayTransport(gateway, currentSnapshot);
}
