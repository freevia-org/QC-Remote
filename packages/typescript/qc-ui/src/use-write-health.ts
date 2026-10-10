import { useCallback, useEffect, useRef } from "react";
import { transitionWriteQuarantine } from "@qc-remote/core";

export type WriteHealthOptions = {
  enabled: boolean;
  recoveryInstruction: string;
  onNotice: (message: string) => void;
  onQuarantineChange?: (quarantined: boolean) => void;
};

export function writeQuarantineNotice(reason: string | undefined, recoveryInstruction: string): string {
  return reason ?? `A previous write to the Quad Cortex could not be confirmed. Writes are paused until ${recoveryInstruction}.`;
}

/** Shares the transition-only notice policy while leaving each host's polling cadence intact. */
export function useWriteHealth({ enabled, recoveryInstruction, onNotice, onQuarantineChange }: WriteHealthOptions) {
  const wasQuarantined = useRef(false);

  useEffect(() => {
    if (enabled) return;
    wasQuarantined.current = false;
    onQuarantineChange?.(false);
  }, [enabled, onQuarantineChange]);

  const observe = useCallback((quarantined: boolean, reason?: string) => {
    if (!enabled) return;
    const transition = transitionWriteQuarantine(wasQuarantined.current, quarantined);
    onQuarantineChange?.(transition.quarantined);
    if (transition.began) {
      onNotice(writeQuarantineNotice(reason, recoveryInstruction));
    }
    wasQuarantined.current = transition.quarantined;
  }, [enabled, onNotice, onQuarantineChange, recoveryInstruction]);

  return { observe };
}
