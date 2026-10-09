import type { LaneControl, PresetSnapshot } from "@qc-remote/client";
import { parseLaneControlSelectionId } from "@qc-remote/core";
import type { LaneControlPanelState } from "./lane-control-panel";
import type { BlockEditorSessionController } from "./use-block-editor-session";
import type { useGridWorkflow } from "./use-grid-workflow";
import type { usePerformanceWorkflow } from "./use-performance-workflow";

export interface QcLaneControlPanelBindingOptions {
  snapshot: PresetSnapshot;
  selectedBlockId: string;
  editor: BlockEditorSessionController;
  grid: ReturnType<typeof useGridWorkflow>;
  performance: ReturnType<typeof usePerformanceWorkflow>;
  pending: boolean;
  notice(message: string): void;
  openExpression(control: LaneControl): void;
}

/**
 * Build `CorOsLaneControlPanel`'s props the same way `qcParameterEditorBindings`
 * builds the generic editor's - reusing the same `editor`/`grid.closeOnDevice` the
 * regular block and routing-node panels already use, `selectedBlockId` (not
 * `details.column`, which is not a real Grid column for a lane control) being
 * the one thing that tells this function it should render at all.
 */
export function qcLaneControlPanelBindings(options: QcLaneControlPanelBindingOptions): LaneControlPanelState | undefined {
  const { snapshot, selectedBlockId, editor, grid, performance, pending, notice, openExpression } = options;
  const lane = parseLaneControlSelectionId(selectedBlockId);
  const details = editor.details;
  if (!lane || !details) return undefined;
  return {
    details,
    control: lane.control,
    activeScene: snapshot.activeScene,
    scenes: snapshot.scenes,
    pending,
    onSceneSelect: (scene) => void performance.selectScene(scene),
    onContextAction: (action) => {
      if (action === "expression") { openExpression(lane.control); return; }
      notice("This action is not available on a lane control.");
    },
    onClose: () => { void grid.closeOnDevice(); }
  };
}
