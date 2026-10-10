import { useCallback, useReducer } from "react";
import type { BlockDetails, BlockParameter } from "@qc-remote/client";
import { emptyBlockEditorSession, reduceBlockEditorSession } from "@qc-remote/core";

export interface BlockEditorSessionController {
  details?: BlockDetails;
  drafts: Record<number, number>;
  page: number;
  close(): void;
  load(details: BlockDetails, resetPage?: boolean): void;
  setPage(page: number): void;
  draft(parameter: Pick<BlockParameter, "index">, value: number): void;
  updateParameter(parameter: Pick<BlockParameter, "index">, value: number): void;
  updateParameters(changes: ReadonlyArray<{ parameter: Pick<BlockParameter, "index">; value: number }>): void;
  setScene(scene: number): void;
}

/** Shared state controller for the CorOS parameter screen on desktop and Android. */
export function useBlockEditorSession(): BlockEditorSessionController {
  const [session, dispatch] = useReducer(reduceBlockEditorSession, emptyBlockEditorSession);

  const close = useCallback(() => dispatch({ type: "close" }), []);
  const load = useCallback((details: BlockDetails, resetPage = false) => dispatch({ type: "details", details, resetPage }), []);
  const setPage = useCallback((page: number) => dispatch({ type: "page", page }), []);
  const draft = useCallback((parameter: Pick<BlockParameter, "index">, value: number) => dispatch({ type: "draft", parameterIndex: parameter.index, value }), []);
  const updateParameter = useCallback((parameter: Pick<BlockParameter, "index">, value: number) => dispatch({ type: "parameters", values: { [parameter.index]: value } }), []);
  const updateParameters = useCallback((changes: ReadonlyArray<{ parameter: Pick<BlockParameter, "index">; value: number }>) => dispatch({ type: "parameters", values: Object.fromEntries(changes.map(({ parameter, value }) => [parameter.index, value])) }), []);
  const setScene = useCallback((scene: number) => dispatch({ type: "scene", scene }), []);

  return { details: session.details, drafts: session.drafts, page: session.page, close, load, setPage, draft, updateParameter, updateParameters, setScene };
}
