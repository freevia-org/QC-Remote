/**
 * Dim a flat ink the way the unit's 5/6/5 framebuffer does, without alpha.
 *
 * The unit never composites a fade: a dimmed element is drawn with each channel
 * substituted by a lower rung of that channel's own ladder (red and blue at
 * `(v << 3) | (v >> 2)`, green at `(v << 2) | (v >> 4)`). A bypassed Grid tile
 * is the measured case: Rage Booster's orange ring (255,113,0) reads 49,20,0 and its
 * white mark 49,48,49 on the live unit, which is the 5/6-bit code of each
 * channel taken to 6/31 of its value (red 31 -> 6, green 28 -> 5 and 63 -> 12).
 * An 8-bit `opacity` over black lands off those ladders for most inks (a
 * neutral 148 composites to 28, where its rung is 24), so the rung is picked
 * here per channel and painted as a flat colour.
 */

/** A bypassed Grid tile keeps 6 of every 31 steps of each channel's ladder. */
export const QC_BYPASSED_TILE_RUNG_SCALE = 6 / 31;

const hexChannel = (hex: string, offset: number) => Number.parseInt(hex.slice(1 + offset * 2, 3 + offset * 2), 16);

const expand5 = (code: number) => (code << 3) | (code >> 2);
const expand6 = (code: number) => (code << 2) | (code >> 4);
const toHex = (value: number) => value.toString(16).padStart(2, "0");

/**
 * `hex` (a six-digit hex value) with each channel's 5/6-bit code scaled by `scale`
 * and expanded back through the unit's ladders. Anything that is not a plain
 * 6-digit hex is returned untouched.
 */
export function qcDimToRungs(hex: string, scale: number = QC_BYPASSED_TILE_RUNG_SCALE): string {
  if (!/^#[0-9a-fA-F]{6}$/.test(hex)) return hex;
  const red = expand5(Math.round((hexChannel(hex, 0) >> 3) * scale));
  const green = expand6(Math.round((hexChannel(hex, 1) >> 2) * scale));
  const blue = expand5(Math.round((hexChannel(hex, 2) >> 3) * scale));
  return `#${toHex(red)}${toHex(green)}${toHex(blue)}`;
}
