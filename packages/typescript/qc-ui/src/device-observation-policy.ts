import { createContext, useContext } from "react";
import type { ConnectionState, PresetSnapshot } from "@qc-remote/client";

export interface DeviceObservationPolicy {
  mode: "live" | "on-demand";
  lastObservedAt?: number;
  refresh?(): Promise<PresetSnapshot>;
  /** Explicit atomic host observation; unavailable targets carry no snapshot. */
  readTarget?(): Promise<{ connection: ConnectionState; snapshot?: PresetSnapshot }>;
  connection?(): ConnectionState | undefined;
}

/** Local USB keeps its normal passive readers. Paid remote transports opt out
 * at the UI read sources; ordinary gateway/guard reads remain fresh. */
export const DeviceObservationContext = createContext<DeviceObservationPolicy>({ mode: "live" });
export const DeviceObservationProvider = DeviceObservationContext.Provider;
export function useDeviceObservationPolicy(): DeviceObservationPolicy {
  return useContext(DeviceObservationContext);
}
