import type { BlockDetails, LaneControl } from "@qc-remote/client";

export interface BlockEditorSession {
  details?: BlockDetails;
  drafts: Record<number, number>;
  page: number;
}

export type BlockEditorSessionAction =
  | { type: "close" }
  | { type: "details"; details?: BlockDetails; resetPage?: boolean }
  | { type: "draft"; parameterIndex: number; value: number }
  | { type: "drafts"; drafts: Record<number, number> }
  | { type: "page"; page: number }
  | { type: "parameters"; values: Readonly<Record<number, number>> }
  | { type: "scene"; scene: number };

export const emptyBlockEditorSession: BlockEditorSession = Object.freeze({
  details: undefined,
  drafts: Object.freeze({}),
  page: 0
});

export function parameterDrafts(details: BlockDetails): Record<number, number> {
  return Object.fromEntries(
    details.parameters
      .filter((parameter) => parameter.normalizedValue !== null)
      .map((parameter) => [parameter.index, parameter.normalizedValue as number])
  );
}

export function updateBlockParameter(details: BlockDetails, parameterIndex: number, value: number): BlockDetails {
  return {
    ...details,
    parameters: details.parameters.map((parameter) => parameter.index === parameterIndex
      ? { ...parameter, normalizedValue: value }
      : parameter)
  };
}

export function updateBlockParameters(details: BlockDetails, values: Readonly<Record<number, number>>): BlockDetails {
  return {
    ...details,
    parameters: details.parameters.map((parameter) => Object.hasOwn(values, parameter.index)
      ? { ...parameter, normalizedValue: values[parameter.index] }
      : parameter)
  };
}

/**
 * Owns the invariant that the editor's metadata and draft controls describe the
 * same block. React shells use this reducer instead of coordinating three pieces
 * of state independently.
 */
export function reduceBlockEditorSession(
  state: BlockEditorSession,
  action: BlockEditorSessionAction
): BlockEditorSession {
  if (action.type === "close") return { details: undefined, drafts: {}, page: 0 };
  if (action.type === "page") return { ...state, page: Math.max(0, Math.floor(action.page)) };
  if (action.type === "draft") return { ...state, drafts: { ...state.drafts, [action.parameterIndex]: action.value } };
  if (action.type === "drafts") return { ...state, drafts: { ...action.drafts } };
  if (action.type === "scene") return state.details
    ? { ...state, details: { ...state.details, scene: action.scene } }
    : state;
  if (action.type === "parameters") {
    if (!state.details) return state;
    const details = updateBlockParameters(state.details, action.values);
    return { ...state, details, drafts: { ...state.drafts, ...action.values } };
  }
  if (!action.details) return { ...state, details: undefined, drafts: {}, page: action.resetPage ? 0 : state.page };
  return {
    details: action.details,
    drafts: parameterDrafts(action.details),
    page: action.resetPage ? 0 : state.page
  };
}

export function blockSelectionIntent(selectedBlockId: string, requestedBlockId: string): "open" | "close" {
  return selectedBlockId !== "" && selectedBlockId === requestedBlockId ? "close" : "open";
}

/**
 * `selectedBlockId` for a lane control panel, matching the existing
 * `routing-${row}-${node}` convention for the Splitter/Mixer virtual columns:
 * a lane control is likewise never a real `(row, column)` Grid block, so it
 * gets its own id shape rather than borrowing the block one.
 */
export function laneControlSelectionId(row: number, control: LaneControl): string {
  return `lane-${row}-${control}`;
}

/** Inverse of {@link laneControlSelectionId}; undefined for anything else. */
export function parseLaneControlSelectionId(selectedBlockId: string): { row: number; control: LaneControl } | undefined {
  const match = /^lane-(\d+)-(inputGate|laneOutput)$/.exec(selectedBlockId);
  return match ? { row: Number(match[1]), control: match[2] as LaneControl } : undefined;
}
