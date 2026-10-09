import { qcCategoryKey, type GridBlock, type QcCategoryKey } from "@qc-remote/client";
import { QC_COLORS } from "@qc-remote/theme";

export type OfficialBlockVisualKey =
  | "plugin" | "amp" | "capture" | "cab" | "overdrive" | "delay" | "reverb"
  | "compressor" | "pitch" | "modulation" | "morph" | "synth" | "filter"
  | "equalizer" | "ir-loader" | "wah" | "fx-loop" | "looper" | "utility";

export interface OfficialBlockVisual {
  key: OfficialBlockVisualKey;
  color: string;
}

export interface OfficialBlockCategory extends OfficialBlockVisual {
  label: string;
  meaning: string;
}

// CorOS 4.1 manual order. Coordinates match the device's own glyphs, and Grid
// outlines use the device's own palette.
export const OFFICIAL_BLOCK_CATEGORIES: readonly OfficialBlockCategory[] = [
  { key: "plugin", label: "Plugins", color: QC_COLORS.browserCategory.plugin, meaning: "Compatible Neural DSP X plugin devices." },
  { key: "amp", label: "Amp", color: QC_COLORS.browserCategory.amp, meaning: "Amplifier devices for guitar and bass." },
  { key: "capture", label: "Neural Capture", color: QC_COLORS.browserCategory.capture, meaning: "Neural Capture devices." },
  { key: "cab", label: "Cab", color: QC_COLORS.browserCategory.cab, meaning: "Mono and stereo cabinet simulations with selectable microphones." },
  { key: "overdrive", label: "Overdrive", color: QC_COLORS.browserCategory.overdrive, meaning: "Boost, distortion, fuzz, and overdrive pedal devices." },
  { key: "delay", label: "Delay", color: QC_COLORS.browserCategory.delay, meaning: "Mono and stereo digital, analog, and tape delays." },
  { key: "reverb", label: "Reverb", color: QC_COLORS.browserCategory.reverb, meaning: "Digital and analog reverbs." },
  { key: "compressor", label: "Compressor", color: QC_COLORS.browserCategory.compressor, meaning: "Mono, stereo, and side-chain dynamics processors." },
  { key: "pitch", label: "Pitch", color: QC_COLORS.browserCategory.pitch, meaning: "Pitch shifter devices." },
  { key: "modulation", label: "Modulation", color: QC_COLORS.browserCategory.modulation, meaning: "Chorus, flanger, phaser, tremolo, and other modulation devices." },
  { key: "morph", label: "Morph", color: QC_COLORS.browserCategory.morph, meaning: "Complex audio processor devices." },
  { key: "synth", label: "Synth", color: QC_COLORS.browserCategory.synth, meaning: "Devices that generate sounds by shaping and manipulating waveforms." },
  { key: "filter", label: "Filter", color: QC_COLORS.browserCategory.filter, meaning: "Dynamic and fixed filter devices." },
  { key: "equalizer", label: "Equalizer", color: QC_COLORS.browserCategory.equalizer, meaning: "Graphic and parametric equalizers." },
  { key: "ir-loader", label: "IR Loader", color: QC_COLORS.browserCategory.irLoader, meaning: "Third-party impulse-response loaders." },
  { key: "wah", label: "Wah", color: QC_COLORS.browserCategory.wah, meaning: "Wah pedal devices." },
  { key: "fx-loop", label: "FX Loop", color: QC_COLORS.browserCategory.fxLoop, meaning: "External-device integration through Send and Return ports." },
  { key: "looper", label: "Looper", color: QC_COLORS.browserCategory.looper, meaning: "Real-time audio recording and layering." },
  { key: "utility", label: "Utility", color: QC_COLORS.browserCategory.utility, meaning: "Routing, mixing, gain, and other audio tools." }
];

export const OFFICIAL_BLOCK_VISUALS = Object.fromEntries(
  OFFICIAL_BLOCK_CATEGORIES.map(({ key, color }) => [key, { key, color }])
) as Record<OfficialBlockVisualKey, OfficialBlockVisual>;

export const OFFICIAL_BLOCK_CATEGORY_BY_KEY = Object.fromEntries(
  OFFICIAL_BLOCK_CATEGORIES.map((category) => [category.key, category])
) as Record<OfficialBlockVisualKey, OfficialBlockCategory>;

/**
 * The palette keys are camelCase because they index `QC_COLORS`; the visual
 * keys are the CorOS names used in markup and fixtures. Only these two differ.
 */
const VISUAL_KEY_BY_CATEGORY: Record<QcCategoryKey, OfficialBlockVisualKey> = {
  plugin: "plugin", capture: "capture", amp: "amp", looper: "looper", irLoader: "ir-loader",
  cab: "cab", overdrive: "overdrive", delay: "delay", reverb: "reverb", compressor: "compressor",
  pitch: "pitch", modulation: "modulation", morph: "morph", synth: "synth", filter: "filter",
  equalizer: "equalizer", wah: "wah", fxLoop: "fx-loop", utility: "utility"
};

/**
 * Which category a block is, is decided once for the whole repository by
 * `qcCategoryKey`, generated from `contracts/public/qc-domain.v1.json`. The footswitch
 * lamps in `@qc-remote/core` and the gateway's `_block_color` classify with the
 * same rules; what differs is only the palette each surface then indexes - the
 * screen paints the RGB565 `browserCategory` ink, the lamps light the 24-bit
 * `category` source it is quantised from.
 */
export function officialBlockVisual(block: GridBlock): OfficialBlockVisual {
  const key = qcCategoryKey(`${block.category ?? ""} ${block.kind ?? ""}`, block.name);
  return OFFICIAL_BLOCK_VISUALS[VISUAL_KEY_BY_CATEGORY[key]];
}

/** *NormalPlugin block icons use a 30% category-color interior fill. */
export function blockUsesActiveFill(block: GridBlock): boolean {
  const category = `${block.category ?? ""}`.toLowerCase();
  return block.bypassed !== true && (block.plugin === true || category.includes("plugin"));
}

export { pluginBadge, PUBLISHED_PLUGIN_BADGES } from "./plugin-badges.ts";
export type { PublishedPluginBadge } from "./plugin-badges.ts";
