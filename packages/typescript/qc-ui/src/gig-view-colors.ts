import { QC_PRESET_SLOT_COLORS, QC_SCENE_COLORS } from "@qc-remote/core";

/** Preset Gig View lights the loaded slot in its fixed A–H screen color. */
export function gigPresetTileColor(position: number): string {
  return QC_PRESET_SLOT_COLORS[((position % 8) + 8) % 8];
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
