import { closeUserOpenedTarget, escapeUserOpenedTarget } from "./user-open-focus";
import type { CpuColumnLoad, PresetSnapshot } from "@qc-remote/client";
import { QcDeviceCategoryGlyph as DeviceCategoryGlyph } from "./device-category-glyph";
import { QcUiIcon } from "./theme-icons";
import { CorOsOfficialGrid } from "./coros-official-grid";

export function CorOsCpuMonitor({ snapshot, onClose, returnFocusTarget, cpuLoad = 0, cpuColumns = [] }: { snapshot: PresetSnapshot; onClose: () => void; returnFocusTarget?: "grid-menu"; cpuLoad?: number; cpuColumns?: readonly CpuColumnLoad[] }) {
  return (
    <CorOsOfficialGrid snapshot={snapshot} cpuColumns={cpuColumns} screenClassName="coros-cpu-screen">
      <aside className="coros-cpu-overlay" aria-label="CPU Monitor" data-qc-open-target="cpu-monitor" tabIndex={-1} onKeyDown={(event) => escapeUserOpenedTarget(event, onClose, returnFocusTarget)}>
        <button aria-label="Close CPU Monitor" onClick={(event) => closeUserOpenedTarget(event.currentTarget, onClose, returnFocusTarget)}>
          <QcUiIcon kind="close" />
        </button>
        <span className="cpu-overlay-eq">
          <DeviceCategoryGlyph label="Equalizer" />
        </span>
        <span className="cpu-overlay-input">In</span>
        <small>CPU</small>
        <strong>{Math.round(Math.max(0, Math.min(1, cpuLoad)) * 100)}%</strong>
      </aside>
    </CorOsOfficialGrid>
  );
}
