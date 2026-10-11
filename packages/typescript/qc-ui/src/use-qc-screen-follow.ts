import { useCallback, useEffect, useRef } from "react";
import type { QcStateUpdate } from "@qc-remote/core";
import { useAdaptiveQcScreenPolling, type QcScreenObservation, type QcScreenPollingPhase } from "./use-adaptive-screen-polling.ts";
import type { QcCpuTelemetry } from "./use-qc-cpu-telemetry.ts";
import type { QcScreenDestination } from "./use-qc-screen-destination.ts";

/** The workflows following the unit's own screen opens and closes. */
export interface QcScreenFollowWorkflows {
  grid: { close(): void };
  preset: { closeDirectory(): void; openDirectory(): unknown };
  deviceSettings: { close(): void; openSettings(): unknown };
  tempo: { close(): void; openTempo(): unknown };
  tuner: { close(): void; openTuner(): unknown };
  ioEq: { refresh(): unknown };
}

export interface QcScreenFollowOptions {
  gateway: Parameters<typeof useAdaptiveQcScreenPolling>[0]["gateway"];
  /** The user's "follow the Quad Cortex screen" setting. */
  follow: boolean;
  /** Host gating that is not about the session: a demo device, a fixture run, a non-native build. */
  available: boolean;
  /** The device session can answer: connected, not suspended, not in a bulk transfer. */
  ready: boolean;
  setView: QcScreenDestination["setView"];
  workflows: QcScreenFollowWorkflows;
}

/**
 * Mirrors the unit's screen into the app: when the unit opens Tuner, Tempo,
 * Gig View, Settings, I/O, the Directory, the CPU Monitor or Looper X, the
 * matching app screen opens; when it returns to the Grid, the local overlays
 * close. Each kind is followed once until the unit shows a different one.
 * The "already followed" latch resets whenever following stops or the session
 * stops answering (a suspend, a bulk transfer, a disconnect), so the screen
 * showing on resume is followed again.
 */
export function useQcScreenFollow(options: QcScreenFollowOptions): { phase: QcScreenPollingPhase; observe(observation: QcScreenObservation): void } {
  const { gateway, follow, available, ready } = options;
  const followed = useRef<QcScreenObservation["kind"] | undefined>(undefined);
  useEffect(() => {
    if (!follow || !ready) followed.current = undefined;
  }, [follow, ready]);
  const latest = useRef(options);
  latest.current = options;
  const observe = useCallback((observation: QcScreenObservation) => {
    if (followed.current === observation.kind) return;
    followed.current = observation.kind;
    const { setView, workflows } = latest.current;
    if (observation.kind === "parameter-editor" || observation.kind === "unknown") return;
    workflows.grid.close();
    workflows.preset.closeDirectory();
    workflows.deviceSettings.close();
    workflows.tempo.close();
    workflows.tuner.close();
    setView(null);
    if (observation.kind === "tuner") { setView("tuner"); void workflows.tuner.openTuner(); }
    else if (observation.kind === "tempo") void workflows.tempo.openTempo();
    else if (observation.kind === "gig") setView("gig");
    else if (observation.kind === "settings") void workflows.deviceSettings.openSettings();
    else if (observation.kind === "io") { setView("io-overview"); void workflows.ioEq.refresh(); }
    else if (observation.kind === "directory") void workflows.preset.openDirectory();
    else if (observation.kind === "cpu-monitor") setView("cpu-monitor");
    else if (observation.kind === "looper") setView("looper-editor");
  }, []);
  const { phase } = useAdaptiveQcScreenPolling({ gateway, enabled: follow && available, connected: ready, onObservation: observe });
  return { phase, observe };
}

/** Where Gig View's on-unit state takes the app's screen: shown opens Gig, hidden closes a Gig screen, anything else is left alone. */
export function qcGigViewTransition(current: QcScreenDestination["view"], shown: boolean): QcScreenDestination["view"] {
  return shown ? "gig" : current?.startsWith("gig") ? null : current;
}

/**
 * The part of every native state batch that decides what the display shows:
 * Gig View shown or hidden on the unit, and the CPU readings. The hosts call
 * this first and keep only their transport-specific handling.
 */
export function consumeQcDisplayStates(states: readonly QcStateUpdate[], display: { setView: QcScreenDestination["setView"]; cpu: Pick<QcCpuTelemetry, "consume"> }): void {
  display.cpu.consume(states);
  for (const state of states) {
    if (state.kind === "gigView" && state.gigViewShown !== undefined) {
      const shown = state.gigViewShown;
      display.setView((current) => qcGigViewTransition(current, shown));
    }
  }
}
