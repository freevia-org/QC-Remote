import { brokerTargetKey, selectedBrokerTarget, validateGatewayResult, type ConnectionState, type PresetSnapshot, type RuntimeStatus } from "@qc-remote/client";

/** MCP connectivity and a usable selected target are different observations.
 * brokerTarget is published only for a usable selected provider. USB diagnostics
 * stay physical-only even when the selected provider is the host's demo. */
export function pairedHostConnection(status: RuntimeStatus): ConnectionState {
  const target = selectedBrokerTarget(status);
  const usb = status.usbDiagnostics;
  const available = status.gatewayAvailable !== false;
  const ready = available && (status.targetReady ?? (Boolean(target) || Boolean(usb?.connected && (usb.synchronized || usb.phase === "ready"))));
  const phase = ready ? "ready" : status.targetReady === false || !available || !usb?.connected ? "disconnected" : usb.phase === "syncing" ? "syncing" : "opening";
  return {
    phase, source: "device", demo: !ready,
    ...(ready && target ? { brokerTarget: { ...target } } : {}),
    detail: ready && target?.source === "demo" ? "Connected to the paired host’s demo device."
      : ready ? "Connected to the Quad Cortex through the paired host."
        : usb?.detail ?? "The paired host has no available device. Check the host and its USB connection."
  };
}

export interface PairedHostObservation { connection: ConnectionState; snapshot?: PresetSnapshot; }

/** Explicit demand only. A metadata-bearing target is checked on both sides
 * of the snapshot read so a host selection change cannot pair a new snapshot
 * with an old broker generation. This is a finite read, never a polling loop. */
export async function readPairedHostTarget(options: {
  callTool(name: string, args: Record<string, unknown>): Promise<any>;
  onStatus?(status: RuntimeStatus): void;
}): Promise<PairedHostObservation> {
  const status = await options.callTool("get_qc_status", {}) as RuntimeStatus;
  const connection = pairedHostConnection(status);
  options.onStatus?.(status);
  if (connection.phase !== "ready") return { connection };
  const snapshot = validateGatewayResult("device.snapshot", await options.callTool("get_current_preset", {})) as PresetSnapshot;
  if (!status.brokerTarget) return { connection, snapshot }; // Legacy physical-only host.
  const after = await options.callTool("get_qc_status", {}) as RuntimeStatus;
  const latest = pairedHostConnection(after);
  options.onStatus?.(after);
  if (latest.phase !== "ready" || brokerTargetKey(status.brokerTarget) !== brokerTargetKey(after.brokerTarget)) {
    throw new Error("The paired host target changed during refresh. Refresh again before editing.");
  }
  return { connection: latest, snapshot };
}
