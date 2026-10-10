import type { PresetSnapshot } from "@qc-remote/client";

export type PresetAddress = Pick<PresetSnapshot, "setlistKey" | "presetPosition">;

/** A slot number is only unique within its setlist. */
export function samePresetAddress(left: PresetAddress, right: PresetAddress): boolean {
  return left.presetPosition === right.presetPosition
    && left.setlistKey.replace(/\/+$/, "") === right.setlistKey.replace(/\/+$/, "");
}
