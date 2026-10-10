import type { GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import type { DeviceHistoryEntry } from "./use-device-history";

export interface WorkflowPrompts {
  confirm(message: string): boolean | Promise<boolean>;
  prompt(message: string, initialValue: string): string | null | Promise<string | null>;
}

/**
 * The app-wide device-command flag and the host's reporting callbacks: what
 * `useGuardedMutation` needs to admit, run and report one device write.
 */
export interface DeviceCommandOptions {
  connected: boolean;
  pending: boolean;
  setPending(pending: boolean): void;
  notice(message: string): void;
  fail(error: unknown): void;
}

/** Common dependencies for a device-wide workflow that writes through the gateway. */
export interface DeviceGatewayWorkflowOptions extends DeviceCommandOptions {
  gateway: GatewayTransport;
}

/** Common dependencies for verified live-device mutation workflows. */
export interface DeviceMutationWorkflowOptions extends DeviceGatewayWorkflowOptions {
  snapshot: PresetSnapshot;
  reconcile(snapshot: PresetSnapshot): void;
  recordHistory(entry: DeviceHistoryEntry): void;
}
