// Generated from public contract snapshots and private maintainer specifications; do not edit by hand. The generator is kept in the private repository.
export const QC_CONNECTION_PHASES = ["disconnected","discovering","searching","opening","handshaking","syncing","ready","degraded","needs-attention"] as const;
export type ConnectionPhase = typeof QC_CONNECTION_PHASES[number];
export const QC_BROKER_CONNECTION_PHASE_ALIASES = {"error":"needs-attention","connecting":"searching"} as const;

export const QC_GRID_ROWS = 4;
export const QC_GRID_COLUMNS = 8;
export const QC_SCENE_COUNT = 8;
export const QC_MINIMUM_TEMPO_BPM = 40;
export const QC_MAXIMUM_TEMPO_BPM = 240;
export const QC_STORED_PRESET_NAME_CHARACTERS = 20;
export const QC_PRESETS_PER_SETLIST = 256;
export const QC_IPC_MAX_FRAME_BYTES = 16777216;
export const QC_STATE_EVENT_DEFAULT_LIMIT = 256;
export const QC_STATE_EVENT_MAXIMUM_LIMIT = 4096;
export const QC_STATE_EVENT_PAGE_MAXIMUM_BYTES = 4194304;
export const QC_STATE_EVENT_HISTORY_MAXIMUM_BYTES = 8388608;
export const QC_INPUT_ROUTES = [
  { id: 1, label: "In 1", group: "MONO" },
  { id: 2, label: "In 2", group: "MONO" },
  { id: 4, label: "Return 1", group: "MONO" },
  { id: 5, label: "Return 2", group: "MONO" },
  { id: 8, label: "USB 5", group: "MONO" },
  { id: 9, label: "USB 6", group: "MONO" },
  { id: 10, label: "USB 7", group: "MONO" },
  { id: 11, label: "USB 8", group: "MONO" },
  { id: 3, label: "In 1/2", group: "STEREO" },
  { id: 6, label: "Return 1/2", group: "STEREO" },
  { id: 12, label: "USB 5/6", group: "STEREO" },
  { id: 13, label: "USB 7/8", group: "STEREO" },
  { id: 7, label: "Prev. Row", group: "" },
  { id: 0, label: "Not In Use", group: "" }
] as const;
export const QC_OUTPUT_ROUTES = [
  { id: 19, label: "Multi Out", group: "STEREO" },
  { id: 1, label: "Out 1/2", group: "STEREO" },
  { id: 2, label: "Out 3/4", group: "STEREO" },
  { id: 3, label: "Send 1/2", group: "STEREO" },
  { id: 22, label: "USB 3/4", group: "STEREO" },
  { id: 14, label: "USB 5/6", group: "STEREO" },
  { id: 15, label: "USB 7/8", group: "STEREO" },
  { id: 4, label: "Out 1", group: "MONO" },
  { id: 5, label: "Out 2", group: "MONO" },
  { id: 6, label: "Out 3", group: "MONO" },
  { id: 7, label: "Out 4", group: "MONO" },
  { id: 8, label: "Send 1", group: "MONO" },
  { id: 9, label: "Send 2", group: "MONO" },
  { id: 20, label: "USB 3", group: "MONO" },
  { id: 21, label: "USB 4", group: "MONO" },
  { id: 10, label: "USB 5", group: "MONO" },
  { id: 11, label: "USB 6", group: "MONO" },
  { id: 12, label: "USB 7", group: "MONO" },
  { id: 13, label: "USB 8", group: "MONO" },
  { id: 16, label: "Row 3", group: "OTHER" },
  { id: 17, label: "Row 4", group: "OTHER" },
  { id: 18, label: "Row 3/4", group: "OTHER" },
  { id: 0, label: "Internal", group: "OTHER" }
] as const;
export type QcCategoryKey = "plugin" | "capture" | "amp" | "looper" | "irLoader" | "cab" | "overdrive" | "delay" | "reverb" | "compressor" | "pitch" | "modulation" | "morph" | "synth" | "filter" | "equalizer" | "wah" | "fxLoop" | "utility";
export interface QcCategoryRule {
  key: QcCategoryKey;
  terms: readonly string[];
  words: readonly string[];
  nameTerms: readonly string[];
}
export const QC_CATEGORY_RULES: readonly QcCategoryRule[] = [
  { key: "plugin", terms: ["plugin"], words: [], nameTerms: [] },
  { key: "capture", terms: ["neural capture","capture"], words: [], nameTerms: [] },
  { key: "amp", terms: ["amplifier"], words: ["amp"], nameTerms: [] },
  { key: "looper", terms: ["looper"], words: [], nameTerms: [] },
  { key: "irLoader", terms: ["ir loader","irloader"], words: [], nameTerms: [] },
  { key: "cab", terms: ["cab","impulse response"], words: [], nameTerms: [] },
  { key: "overdrive", terms: ["overdrive","distortion","drive","boost","fuzz"], words: [], nameTerms: [] },
  { key: "delay", terms: ["delay"], words: [], nameTerms: [] },
  { key: "reverb", terms: ["reverb"], words: [], nameTerms: [] },
  { key: "compressor", terms: ["compressor"], words: [], nameTerms: [] },
  { key: "pitch", terms: ["pitch"], words: [], nameTerms: ["octav"] },
  { key: "modulation", terms: ["modulation","chorus","flanger","phaser","tremolo","vibrato"], words: ["mod"], nameTerms: [] },
  { key: "morph", terms: ["morph"], words: [], nameTerms: [] },
  { key: "synth", terms: ["synth"], words: [], nameTerms: [] },
  { key: "filter", terms: ["filter"], words: [], nameTerms: [] },
  { key: "equalizer", terms: ["equalizer"], words: ["eq"], nameTerms: [] },
  { key: "wah", terms: ["wah"], words: [], nameTerms: [] },
  { key: "fxLoop", terms: ["fx loop","effects loop"], words: [], nameTerms: [] },
  { key: "utility", terms: ["utility"], words: [], nameTerms: ["gate"] }
];
/**
 * Classify a block by the CorOS category string it reports, in one place.
 *
 * Both category palettes are keyed on the result: QC_COLORS.category is the
 * 24-bit colour CorOS stores and lights the footswitch lamp with, and
 * QC_COLORS.browserCategory is the RGB565 ink the screen paints. Pick the key
 * here, then index whichever palette the surface needs.
 */
export function qcCategoryKey(category: string, name = ""): QcCategoryKey {
  const haystack = category.toLowerCase();
  const model = name.toLowerCase();
  for (const rule of QC_CATEGORY_RULES) {
    if (rule.terms.some((term) => haystack.includes(term))) return rule.key;
    if (rule.words.some((word) => new RegExp(`(^|\\s)${word}(\\s|$)`).test(haystack))) return rule.key;
    if (rule.nameTerms.some((term) => model.includes(term))) return rule.key;
  }
  return "utility";
}
