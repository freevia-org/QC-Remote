import { useCallback, useState } from "react";
import type { CpuColumnLoad } from "@qc-remote/client";
import type { QcStateUpdate } from "@qc-remote/core";

/** What the surface needs to lay the CPU Monitor over the live Grid. */
export interface QcCpuMonitorProps {
  cpuLoad: number;
  cpuColumns: readonly CpuColumnLoad[];
  returnFocusTarget?: "grid-menu";
  onClose: () => void;
}

export interface QcCpuTelemetry {
  cpuLoad: number;
  cpuColumns: readonly CpuColumnLoad[];
  /** Feed every native state batch; only `cpuLoad` updates are used. */
  consume(states: readonly QcStateUpdate[]): void;
  /** The monitor overlay while it is open, otherwise undefined. */
  monitor(open: boolean, returnFocusTarget: "grid-menu" | undefined, onClose: () => void): QcCpuMonitorProps | undefined;
}

/**
 * The one owner of the CPU readings the display shows: the aggregate load and
 * the per-block columns. A partial update (load without columns, or columns
 * without load) keeps the other reading, so neither host can drop one.
 */
export function useQcCpuTelemetry(): QcCpuTelemetry {
  const [cpuLoad, setCpuLoad] = useState(0);
  const [cpuColumns, setCpuColumns] = useState<readonly CpuColumnLoad[]>([]);
  const consume = useCallback((states: readonly QcStateUpdate[]) => {
    for (const state of states) {
      if (state.kind !== "cpuLoad") continue;
      if (state.cpuTotalLoad !== undefined) setCpuLoad(state.cpuTotalLoad);
      if (state.cpuColumns) setCpuColumns(state.cpuColumns);
    }
  }, []);
  const monitor = useCallback((open: boolean, returnFocusTarget: "grid-menu" | undefined, onClose: () => void) => (
    open ? { cpuLoad, cpuColumns, returnFocusTarget, onClose } : undefined
  ), [cpuLoad, cpuColumns]);
  return { cpuLoad, cpuColumns, consume, monitor };
}
