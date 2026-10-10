import type { LaneControl } from "@qc-remote/client";

export type QcSurfaceAction =
  | { kind: "switch"; role: string; phase: "press" | "release" }
  | { kind: "rotate"; role: string; delta: number }
  | { kind: "select-scene"; scene: number }
  | { kind: "select-mode-slot"; slot: 0 | 1 | 2 }
  | { kind: "select-block"; blockId: string }
  | { kind: "select-routing-node"; row: number; node: "splitter" | "mixer" }
  | { kind: "select-lane-control"; row: number; control: LaneControl }
  | { kind: "place-split"; row: number }
  | { kind: "add-block-cell"; row: number; column: number }
  | { kind: "toggle-bypass"; blockId: string }
  | { kind: "move-block-to"; blockId: string; fromRow: number; fromColumn: number; toRow: number; toColumn: number }
  | { kind: "remove-block-by-drag"; blockId: string };

export type QcSurfaceCommand =
  | { kind: "select-scene"; scene: number }
  | { kind: "select-mode-slot"; slot: 0 | 1 | 2 }
  | { kind: "toggle-block-editor"; blockId: string }
  | { kind: "open-routing-node"; row: number; node: "splitter" | "mixer" }
  | { kind: "open-lane-control"; row: number; control: LaneControl }
  | { kind: "open-split-placement"; row: number }
  | { kind: "open-add-block"; row: number; column: number }
  | { kind: "toggle-block-bypass"; blockId: string }
  | { kind: "move-block"; blockId: string; fromRow: number; fromColumn: number; toRow: number; toColumn: number }
  | { kind: "remove-block"; blockId: string }
  | { kind: "press-footswitch"; index: number }
  | { kind: "move-preset"; delta: -1 | 1 }
  | { kind: "tap-tempo" }
  | { kind: "rotate"; role: string; delta: number }
  | { kind: "none" };

export function surfaceCommand(action: QcSurfaceAction): QcSurfaceCommand {
  if (action.kind === "select-scene") return action;
  if (action.kind === "select-mode-slot") return action;
  if (action.kind === "select-block") return { kind: "toggle-block-editor", blockId: action.blockId };
  if (action.kind === "select-routing-node") return { kind: "open-routing-node", row: action.row, node: action.node };
  if (action.kind === "select-lane-control") return { kind: "open-lane-control", row: action.row, control: action.control };
  if (action.kind === "place-split") return { kind: "open-split-placement", row: action.row };
  if (action.kind === "add-block-cell") return { kind: "open-add-block", row: action.row, column: action.column };
  if (action.kind === "toggle-bypass") return { kind: "toggle-block-bypass", blockId: action.blockId };
  if (action.kind === "move-block-to") return { kind: "move-block", blockId: action.blockId, fromRow: action.fromRow, fromColumn: action.fromColumn, toRow: action.toRow, toColumn: action.toColumn };
  if (action.kind === "remove-block-by-drag") return { kind: "remove-block", blockId: action.blockId };
  if (action.kind === "rotate") return action;
  if (action.phase !== "release") return { kind: "none" };
  const footswitch = /^footswitch:([A-H])$/i.exec(action.role);
  if (footswitch) return { kind: "press-footswitch", index: footswitch[1].toUpperCase().charCodeAt(0) - 65 };
  if (action.role === "bank:up") return { kind: "move-preset", delta: -1 };
  if (action.role === "bank:down") return { kind: "move-preset", delta: 1 };
  if (action.role === "tempo") return { kind: "tap-tempo" };
  return { kind: "none" };
}

export interface QcSurfaceCommandHandlers {
  selectScene?(scene: number): void;
  selectModeSlot?(slot: 0 | 1 | 2): void;
  toggleBlockEditor?(blockId: string): void;
  openRoutingNode?(row: number, node: "splitter" | "mixer"): void;
  openLaneControl?(row: number, control: LaneControl): void;
  openSplitPlacement?(row: number): void;
  openAddBlock?(row: number, column: number): void;
  toggleBlockBypass?(blockId: string): void;
  moveBlock?(blockId: string, from: { row: number; column: number }, to: { row: number; column: number }): void;
  removeBlock?(blockId: string): void;
  pressFootswitch?(index: number): void;
  movePreset?(delta: -1 | 1): void;
  tapTempo?(): void;
  rotate?(role: string, delta: number): void;
}

/** Dispatch the portable part of a hardware action without platform-specific switch statements. */
export function dispatchSurfaceCommand(command: QcSurfaceCommand, handlers: QcSurfaceCommandHandlers): boolean {
  if (command.kind === "select-scene" && handlers.selectScene) handlers.selectScene(command.scene);
  else if (command.kind === "select-mode-slot" && handlers.selectModeSlot) handlers.selectModeSlot(command.slot);
  else if (command.kind === "toggle-block-editor" && handlers.toggleBlockEditor) handlers.toggleBlockEditor(command.blockId);
  else if (command.kind === "open-routing-node" && handlers.openRoutingNode) handlers.openRoutingNode(command.row, command.node);
  else if (command.kind === "open-lane-control" && handlers.openLaneControl) handlers.openLaneControl(command.row, command.control);
  else if (command.kind === "open-split-placement" && handlers.openSplitPlacement) handlers.openSplitPlacement(command.row);
  else if (command.kind === "open-add-block" && handlers.openAddBlock) handlers.openAddBlock(command.row, command.column);
  else if (command.kind === "toggle-block-bypass" && handlers.toggleBlockBypass) handlers.toggleBlockBypass(command.blockId);
  else if (command.kind === "move-block" && handlers.moveBlock) handlers.moveBlock(command.blockId, { row: command.fromRow, column: command.fromColumn }, { row: command.toRow, column: command.toColumn });
  else if (command.kind === "remove-block" && handlers.removeBlock) handlers.removeBlock(command.blockId);
  else if (command.kind === "press-footswitch" && handlers.pressFootswitch) handlers.pressFootswitch(command.index);
  else if (command.kind === "move-preset" && handlers.movePreset) handlers.movePreset(command.delta);
  else if (command.kind === "tap-tempo" && handlers.tapTempo) handlers.tapTempo();
  else if (command.kind === "rotate" && handlers.rotate) handlers.rotate(command.role, command.delta);
  else return false;
  return true;
}
