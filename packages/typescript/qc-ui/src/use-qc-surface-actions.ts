import { useCallback } from "react";
import type { BlockDetails, GridBlock, LaneControl, PresetSnapshot } from "@qc-remote/client";
import {
  blockSelectionIntent, dispatchSurfaceCommand, surfaceCommand,
  type QcSurfaceAction
} from "@qc-remote/core";
import type { useGridWorkflow } from "./use-grid-workflow";
import type { usePerformanceWorkflow } from "./use-performance-workflow";

export interface QcSurfaceActionOptions {
  snapshot: PresetSnapshot;
  selectedBlockId: string;
  blockDetails?: BlockDetails;
  grid: ReturnType<typeof useGridWorkflow>;
  performance: ReturnType<typeof usePerformanceWorkflow>;
  openBlock(block: GridBlock): void;
  closeBlock(): void;
  openRoutingNode?(row: number, node: "splitter" | "mixer"): void;
  openLaneControl?(row: number, control: LaneControl): void;
  openSplitPlacement?(row: number): void;
  rotate?(role: string, delta: number): void;
  editorUnhandled?(action: Extract<QcSurfaceAction, { kind: "switch" }>): void;
  unhandled?(action: QcSurfaceAction): void;
}

/** Route the portable QC surface once; hosts supply only screen-specific behavior. */
export function useQcSurfaceActions(options: QcSurfaceActionOptions) {
  const {
    snapshot, selectedBlockId, blockDetails, grid, performance,
    openBlock, closeBlock, openRoutingNode, openLaneControl, openSplitPlacement, rotate, editorUnhandled, unhandled
  } = options;

  return useCallback((action: QcSurfaceAction) => {
    const command = surfaceCommand(action);
    if (dispatchSurfaceCommand(command, {
      selectScene: (scene) => void performance.selectScene(scene),
      toggleBlockEditor: (blockId) => {
        if (blockSelectionIntent(selectedBlockId, blockId) === "close") closeBlock();
        else {
          const block = snapshot.blocks.find((candidate) => candidate.id === blockId);
          if (block) openBlock(block);
        }
      },
      openRoutingNode,
      openLaneControl,
      openSplitPlacement,
      openAddBlock: (row, column) => void grid.openAdd({ row, column }),
      // Grid gestures: double-click / long-press toggles bypass exactly as the
      // B key does, a drag moves the block, a drag off the Grid removes it.
      toggleBlockBypass: (blockId) => {
        const block = snapshot.blocks.find((candidate) => candidate.id === blockId);
        if (block) void performance.toggleBlockBypass(block);
      },
      moveBlock: (blockId, from, to) => {
        const block = snapshot.blocks.find((candidate) => candidate.id === blockId);
        if (block && block.row === from.row && block.column === from.column) void grid.moveBlockTo(block, to.row, to.column);
      },
      removeBlock: (blockId) => void grid.removeBlock(snapshot.blocks.find((candidate) => candidate.id === blockId)),
      selectModeSlot: (slot) => void performance.selectModeSlot(slot),
      rotate
    })) return;

    if (action.kind !== "switch") {
      unhandled?.(action);
      return;
    }
    if (blockDetails && grid.footswitchAssignmentPending && action.phase === "release" && action.role.startsWith("footswitch:")) {
      const index = action.role.charCodeAt(action.role.length - 1) - 65;
      const assigned = snapshot.blocks.find((block) => block.id === selectedBlockId)?.footswitch;
      grid.setFootswitchAssignmentPending(false);
      void grid.assignFootswitch(assigned === index ? null : index);
      return;
    }
    if (blockDetails && action.phase === "release") {
      editorUnhandled?.(action);
      return;
    }
    if (dispatchSurfaceCommand(command, {
      pressFootswitch: (index) => void performance.pressFootswitch(index),
      movePreset: (delta) => void performance.movePreset(delta),
      tapTempo: () => void performance.tapTempo()
    })) return;
    unhandled?.(action);
  }, [blockDetails, closeBlock, editorUnhandled, grid, openBlock, openLaneControl, openRoutingNode, openSplitPlacement, performance, rotate, selectedBlockId, snapshot.blocks, unhandled]);
}
