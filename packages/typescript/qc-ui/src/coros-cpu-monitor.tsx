import { closeUserOpenedTarget, escapeUserOpenedTarget } from "./user-open-focus";
import type { CpuColumnLoad, PresetSnapshot } from "@qc-remote/client";
import { QcDeviceCategoryGlyph as DeviceCategoryGlyph } from "./device-category-glyph";
import { QcUiIcon } from "./theme-icons";
import { CorOsOfficialGrid } from "./coros-official-grid";

export interface CorOsCpuMonitorPanelProps {
  onClose: () => void;
  returnFocusTarget?: "grid-menu";
  /** Fraction of the CPU in use, 0..1. */
  cpuLoad?: number;
}

/**
 * The CPU Monitor's own element: the small readout the unit lays over the top
 * right of the Grid. It draws nothing but that panel, so it can sit on top of
 * any Grid (the live one in QuadCortexSurface, or the fixture one below) and
 * leave that Grid exactly as it was. Its frame is transparent to pointer input;
 * only the close control takes it.
 *
 * Geometry is authored on the unit's 800x480 canvas (see .coros-cpu-overlay-canvas)
 * so it lands in the same place whether the host is a fixed canvas or a bezel
 * that scales the Grid to its own size.
 */
export function CorOsCpuMonitorPanel({ onClose, returnFocusTarget, cpuLoad }: CorOsCpuMonitorPanelProps) {
  return (
    <div className="coros-cpu-overlay-frame">
      <div className="coros-cpu-overlay-canvas">
        <aside className="coros-cpu-overlay" aria-label="CPU Monitor" data-qc-open-target="cpu-monitor" tabIndex={-1} onKeyDown={(event) => escapeUserOpenedTarget(event, onClose, returnFocusTarget)}>
          <button aria-label="Close CPU Monitor" onClick={(event) => closeUserOpenedTarget(event.currentTarget, onClose, returnFocusTarget)}>
            <QcUiIcon kind="close" />
          </button>
          <span className="cpu-overlay-eq">
            <DeviceCategoryGlyph label="Equalizer" />
          </span>
          <span className="cpu-overlay-input">In</span>
          <small>CPU</small>
          <strong>{cpuLoad === undefined ? "—" : `${Math.round(Math.max(0, Math.min(1, cpuLoad)) * 100)}%`}</strong>
        </aside>
      </div>
    </div>
  );
}

/**
 * The CPU Monitor on the simplified vector Grid the visual fixtures use. The
 * live app does not render this: it lays CorOsCpuMonitorPanel over its own Grid.
 */
export function CorOsCpuMonitor({ snapshot, onClose, returnFocusTarget, cpuLoad = 0, cpuColumns = [] }: { snapshot: PresetSnapshot; onClose: () => void; returnFocusTarget?: "grid-menu"; cpuLoad?: number; cpuColumns?: readonly CpuColumnLoad[] }) {
  return (
    <CorOsOfficialGrid referenceOnly snapshot={snapshot} cpuColumns={cpuColumns} screenClassName="coros-cpu-screen">
      <CorOsCpuMonitorPanel cpuLoad={cpuLoad} onClose={onClose} returnFocusTarget={returnFocusTarget} />
    </CorOsOfficialGrid>
  );
}
