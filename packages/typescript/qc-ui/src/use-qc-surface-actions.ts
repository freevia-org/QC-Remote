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
  /** What an encoder turn adjusts: the open parameter editor first (returns whether it took the turn), then Tempo or Master Volume. */
  rotation: { editor(role: string, delta: number): boolean; tempo(delta: number): void; masterVolume(delta: number): void };
  /** A demo or virtual device: unhandled controls say so instead of claiming to be unverified. */
  demo: boolean;
  notice(message: string): void;
}

/** Route the portable QC surface once; hosts supply only screen-specific behavior. */
export function useQcSurfaceActions(options: QcSurfaceActionOptions) {
  const {
    snapshot, selectedBlockId, blockDetails, grid, performance,
    openBlock, closeBlock, openRoutingNode, openLaneControl, openSplitPlacement, rotation, demo, notice
  } = options;

  const unhandled = (action: QcSurfaceAction) => {
    if (action.kind !== "switch" || action.phase !== "release") return;
    if (action.role === "power") notice("Power and lock actions are intentionally available only on the physical Quad Cortex.");
    else notice(demo ? `Demo switch: ${action.role}. Hardware was not changed.` : `${action.role} has no verified action on the current screen.`);
  };

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
      rotate: (role, delta) => {
        if (rotation.editor(role, delta)) return;
        if (role === "tempo") rotation.tempo(delta);
        else if (role === "master-volume") rotation.masterVolume(delta);
        else notice(demo ? `Demo encoder: ${role} ${delta > 0 ? "+" : "−"}1.` : `${role} has no verified encoder action on the current screen.`);
      }
    })) return;

    if (action.kind !== "switch") {
      unhandled(action);
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
      if (action.role === "bank:up") notice("BANK UP cycles models on the hardware. Model replacement stays disabled until the gateway can perform it atomically.");
      else notice(`${action.role} is mapped to the on-screen parameter above it; drag or rotate it to adjust the value.`);
      return;
    }
    if (dispatchSurfaceCommand(command, {
      pressFootswitch: (index) => void performance.pressFootswitch(index),
      movePreset: (delta) => void performance.movePreset(delta),
      tapTempo: () => void performance.tapTempo()
    })) return;
    unhandled(action);
  }, [blockDetails, closeBlock, demo, grid, notice, openBlock, openLaneControl, openRoutingNode, openSplitPlacement, performance, rotation, selectedBlockId, snapshot.blocks]);
}
