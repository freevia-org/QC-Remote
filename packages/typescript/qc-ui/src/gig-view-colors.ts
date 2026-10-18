import { QC_PRESET_SLOT_COLORS, QC_SCENE_COLORS } from "@qc-remote/core";

/** Preset Gig View lights the loaded slot in its fixed A–H screen color. */
export function gigPresetTileColor(position: number): string {
  return QC_PRESET_SLOT_COLORS[((position % 8) + 8) % 8];
}

/**
 * The Gig View scene tile's edit / swap / copy glyphs are the tile's own colour
 * under a 60% wash, sampled from the unit on 4.1.0 for all eight tiles: a lit
 * (selected) tile's glyphs are its colour at 40% (black washed over it: the
 * red tile's glyphs are 99,12,8 on 255,36,33), every other tile's are its
 * colour at 40% plus 150 (grey 250 washed over it: 156,162,165 on 16,28,33).
 * Each channel then lands on the panel's 5/6/5 ladder. `tile` is the tile's
 * fill, as a hex colour or a `var(--qc-palette-rrggbb)` reference.
 */
export function gigSceneToolColor(tile: string, lit: boolean): string {
  const hex = /([0-9a-f]{6})\)?$/i.exec(tile)?.[1];
  if (!hex) return "var(--qc-palette-a8aaa8)";
  const channel = (offset: number) => Number.parseInt(hex.slice(offset, offset + 2), 16);
  const wash = (value: number) => Math.floor(value * 0.4 + (lit ? 0 : 150));
  const fiveBit = (value: number) => { const reduced = value >> 3; return (reduced << 3) | (reduced >> 2); };
  const sixBit = (value: number) => { const reduced = value >> 2; return (reduced << 2) | (reduced >> 4); };
  return `#${[fiveBit(wash(channel(0))), sixBit(wash(channel(2))), fiveBit(wash(channel(4)))].map((value) => value.toString(16).padStart(2, "0")).join("")}`;
}

/** Scene colors arrive as 24-bit values; the QC framebuffer paints RGB565. */
export function gigSceneTileColor(color: string | undefined, scene: number): string {
  const fallback = QC_SCENE_COLORS[scene] ?? QC_SCENE_COLORS[0];
  const hex = /^#(?:[0-9a-f]{2})?([0-9a-f]{6})$/i.exec(color ?? fallback)?.[1];
  if (!hex) return fallback;
  const channel = (offset: number) => Number.parseInt(hex.slice(offset, offset + 2), 16);
  const fiveBit = (value: number) => { const reduced = value >> 3; return (reduced << 3) | (reduced >> 2); };
  const sixBit = (value: number) => { const reduced = value >> 2; return (reduced << 2) | (reduced >> 4); };
  return `#${[fiveBit(channel(0)), sixBit(channel(2)), fiveBit(channel(4))].map((value) => value.toString(16).padStart(2, "0")).join("")}`;
}
