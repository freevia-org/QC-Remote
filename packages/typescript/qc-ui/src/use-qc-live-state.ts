import { useCallback, useRef } from "react";
import type { QcStateUpdate } from "@qc-remote/core";
import type { BlockEditorSessionController } from "./use-block-editor-session";

export interface QcLiveStateOptions {
  reconcileFrame(states: readonly QcStateUpdate[], now?: number): {
    states: QcStateUpdate[];
  };
  editor: Pick<BlockEditorSessionController, "details" | "updateParameters">;
  onStates?(states: readonly QcStateUpdate[], frame?: {sequence: number; streamId?: string}): void;
}

/**
 * Shared native-frame consumer for desktop and Android. It applies the command
 * coordinator's timestamp policy first, then mirrors accepted parameter values
 * into an open block editor in one reducer update.
 */
export function useQcLiveState({ reconcileFrame, editor, onStates }: QcLiveStateOptions) {
  const openBlockRef = useRef<{ row: number; column: number } | undefined>(undefined);
  const onStatesRef = useRef(onStates);
  openBlockRef.current = editor.details
    ? { row: editor.details.row, column: editor.details.column }
    : undefined;
  onStatesRef.current = onStates;

  return useCallback((states: readonly QcStateUpdate[], observedAt?: number, frame?: {sequence: number; streamId?: string}) => {
    const reduced = reconcileFrame(states, observedAt);
    const openBlock = openBlockRef.current;
    if (openBlock) {
      const changes = reduced.states.flatMap((state) => (
        state.kind === "parameter"
        && state.parameterIndex !== undefined
        && state.normalizedValue !== undefined
        && state.row === openBlock.row
        && state.column === openBlock.column
          ? [{ parameter: { index: state.parameterIndex }, value: state.normalizedValue }]
          : []
      ));
      if (changes.length) editor.updateParameters(changes);
    }
    onStatesRef.current?.(reduced.states, frame);
    return reduced;
  }, [editor.updateParameters, reconcileFrame]);
}
