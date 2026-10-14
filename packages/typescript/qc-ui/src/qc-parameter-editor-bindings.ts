import type { PresetSnapshot } from "@qc-remote/client";
import { parseLaneControlSelectionId } from "@qc-remote/core";
import { QC_COLORS } from "@qc-remote/theme";
import type { CorOsParameterEditorProps } from "./parameter-editor";
import { officialBlockVisual } from "./block-visuals";
import type { BlockEditorSessionController } from "./use-block-editor-session";
import type { useGridWorkflow } from "./use-grid-workflow";
import type { useParameterWorkflow } from "./use-parameter-workflow";
import type { usePerformanceWorkflow } from "./use-performance-workflow";

export interface QcParameterEditorBindingOptions {
  snapshot: PresetSnapshot;
  selectedBlockId: string;
  editor: BlockEditorSessionController;
  grid: ReturnType<typeof useGridWorkflow>;
  parameter: ReturnType<typeof useParameterWorkflow>;
  performance: ReturnType<typeof usePerformanceWorkflow>;
  connected: boolean;
  pending: boolean;
  initialMenuOpen?: boolean;
  titlePresetMark?: boolean;
  editorContextKey?: unknown;
  notice(message: string): void;
  openExpression(): void;
}

const unavailableLabels = {
  "save-device-preset": "Save Current Parameters as...",
  "change-device": "Change device",
  "reset-defaults": "Reset to defaults",
  "assign-looper-actions": "Reassign Looper X Actions",
  "mute-bypass": "Mute/bypass",
  "model-update": "Model update available",
  "legacy-version": "Change to legacy version"
} as const;

/** Build the identical parameter-screen behavior used by desktop and mobile. */
export function qcParameterEditorBindings(options: QcParameterEditorBindingOptions): CorOsParameterEditorProps | undefined {
  const { snapshot, selectedBlockId, editor, grid, parameter, performance, connected, pending, initialMenuOpen, notice, openExpression } = options;
  const details = editor.details;
  if (!details) return undefined;
  // A lane control (Input Gate/Lane Output) shares this same editor session -
  // `editor.load` is generic over what `BlockDetails` describes - but renders
  // through CorOsLaneControlPanel instead, via qcLaneControlPanelBindings; it
  // is never a routingNode either, so this generic editor must stand down.
  if (parseLaneControlSelectionId(selectedBlockId)) return undefined;
  const selected = snapshot.blocks.find((block) => block.id === selectedBlockId)
    ?? snapshot.blocks.find((block) => block.row === details.row && block.column === details.column);
  const routingNode = details.column === 8 ? "splitter" : details.column === 9 ? "mixer" : undefined;
  const accent = routingNode === "splitter"
    ? QC_COLORS.category.equalizer
    : routingNode === "mixer"
      ? QC_COLORS.category.synth
      : officialBlockVisual(selected ?? { id: "editor", name: details.name, kind: "utility", category: details.category, row: details.row, column: details.column }).color;

  return {
    editorContextKey: options.editorContextKey,
    titlePresetMark: options.titlePresetMark,
    details,
    drafts: editor.drafts,
    accent,
    activeScene: snapshot.activeScene,
    scenes: snapshot.scenes,
    bypassed: Boolean(selected?.bypassed),
    footswitch: selected?.footswitch,
    routingNode,
    disabled: pending,
    page: editor.page,
    onPageChange: editor.setPage,
    onDraftChange: parameter.draft,
    onCommit: parameter.commit,
    onToggleSceneMode: (parameterItem) => void parameter.toggleSceneMode(parameterItem),
    onCommitBatch: (changes) => void parameter.commitBatch(changes),
    onToggleBypass: () => { if (selected) void performance.toggleBlockBypass(selected); },
    onSceneSelect: (scene) => void performance.selectScene(scene),
    footswitchAssignmentPending: grid.footswitchAssignmentPending,
    onFootswitchAssignmentStart: (assignmentPending) => {
      grid.setFootswitchAssignmentPending(assignmentPending);
      notice(assignmentPending
        ? "Press footswitch A–H to assign it; press the currently assigned switch to remove the assignment."
        : "Footswitch assignment cancelled.");
    },
    onUnassignFootswitch: () => void grid.assignFootswitch(null),
    clipboardModelId: grid.clipboard?.modelId,
    contextActionEnabled: {
      "save-device-preset": connected && !pending,
      "change-device": connected && !pending,
      "copy-device": true,
      "paste-device": Boolean(grid.clipboard && grid.clipboard.modelId === details.modelId && connected && !pending),
      "reset-defaults": connected && !pending,
      expression: true,
      "assign-looper-actions": connected && !pending,
      "mute-bypass": connected && !pending,
      "model-update": false,
      "legacy-version": false,
      remove: connected && !pending
    },
    initialMenuOpen,
    onSaveDevicePreset: grid.saveDevicePreset,
    onLoadLooperAssignments: grid.loadLooperAssignments,
    onSaveLooperAssignments: grid.saveLooperAssignments,
    onContextAction: (action) => {
      if (action === "copy-device") { void grid.copy(); return; }
      if (action === "paste-device") { void grid.paste(); return; }
      if (action === "reset-defaults") { void parameter.resetDefaults(); return; }
      if (action === "remove") { void grid.remove(); return; }
      if (action === "expression") { openExpression(); return; }
      if (action === "change-device") { void grid.openReplace(); return; }
      notice(`${unavailableLabels[action]} is present in its CorOS position, but this write is not exposed by the current USB gateway.`);
    },
    onClose: () => {
      parameter.cancel();
      void grid.closeOnDevice();
    }
  };
}
