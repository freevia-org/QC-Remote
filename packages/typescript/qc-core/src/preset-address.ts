import type { PresetSnapshot } from "@qc-remote/client";

export type PresetAddress = Pick<PresetSnapshot, "setlistKey" | "presetPosition">;

/**
 * `key` without its trailing slashes, in linear time. A `replace(/\/+$/, "")`
 * is quadratic on a long run of slashes followed by another character.
 */
export function trimTrailingSlashes(key: string): string {
  let end = key.length;
  while (end > 0 && key.charCodeAt(end - 1) === 0x2f) end -= 1;
  return key.slice(0, end);
}

/** A slot number is only unique within its setlist. */
export function samePresetAddress(left: PresetAddress, right: PresetAddress): boolean {
  return left.presetPosition === right.presetPosition
    && trimTrailingSlashes(left.setlistKey) === trimTrailingSlashes(right.setlistKey);
}
