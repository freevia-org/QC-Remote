import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { CpuColumnLoad } from "@qc-remote/client";
import type { PresetLifetime, QcStateUpdate } from "@qc-remote/core";

/** What the surface needs to lay the CPU Monitor over the live Grid. */
export interface QcCpuMonitorProps {
  cpuLoad?: number;
  cpuColumns: readonly CpuColumnLoad[];
  returnFocusTarget?: "grid-menu";
  onClose: () => void;
}

export interface QcCpuTelemetry {
  cpuLoad?: number;
  cpuColumns: readonly CpuColumnLoad[];
  /** Feed native states belonging to this callback's physical session. */
  consume(states: readonly QcStateUpdate[]): void;
  monitor(open: boolean, returnFocusTarget: "grid-menu" | undefined, onClose: () => void): QcCpuMonitorProps | undefined;
}

/** Partial CPU updates retain their other reading, but never another device's. */
export function useQcCpuTelemetry(lifetime?: PresetLifetime): QcCpuTelemetry {
  const [cpuLoad, setCpuLoad] = useState<number>();
  const [cpuColumns, setCpuColumns] = useState<readonly CpuColumnLoad[]>([]);
  const [sessionRevision, setSessionRevision] = useState(0);
  const mounted = useRef(true);
  const mountEpoch = useRef(0);
  const renderedMount = mountEpoch.current;
  const session = lifetime?.checkpoint().session;
  const origin = useMemo(() => lifetime?.checkpoint(), [lifetime, session, sessionRevision]);
  useEffect(() => {
    mounted.current = true;
    // Re-arm callbacks after an effect cleanup/setup cycle, without reviving
    // any old callback that captured the preceding mounted epoch.
    setSessionRevision(revision => revision + 1);
    return () => { mounted.current = false; mountEpoch.current++; };
  }, []);
  useEffect(() => {
    const clear = (physical = true) => {
      if (physical) setCpuLoad(undefined);
      setCpuColumns([]);
      setSessionRevision(revision => revision + 1);
    };
    clear();
    let checkpoint = lifetime?.checkpoint();
    return lifetime?.subscribe(() => {
      if (checkpoint && !lifetime.current(checkpoint)) {
        const physical = !lifetime.currentSession(checkpoint);
        checkpoint = lifetime.checkpoint();
        clear(physical);
      }
    });
  }, [lifetime]);
  const consume = useCallback((states: readonly QcStateUpdate[]) => {
    if (!mounted.current || renderedMount !== mountEpoch.current
      || (origin && !lifetime!.currentSession(origin))) return;
    for (const state of states) {
      if (state.kind !== "cpuLoad") continue;
      if (state.cpuTotalLoad !== undefined) setCpuLoad(state.cpuTotalLoad);
      // Column indices name blocks in one preset layout, unlike the total.
      if (state.cpuColumns && (!origin || lifetime!.current(origin))) setCpuColumns(state.cpuColumns);
    }
  }, [lifetime, origin, renderedMount]);
  const monitor = useCallback((open: boolean, returnFocusTarget: "grid-menu" | undefined, onClose: () => void) => (
    open ? { cpuLoad, cpuColumns, returnFocusTarget, onClose } : undefined
  ), [cpuLoad, cpuColumns]);
  return { cpuLoad, cpuColumns, consume, monitor };
}
