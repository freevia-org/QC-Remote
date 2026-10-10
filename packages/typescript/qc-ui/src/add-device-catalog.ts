import type { ModelEntry } from "@qc-remote/client";
import { QC_COLORS } from "@qc-remote/theme";

/**
 * The nineteen categories of the unit's device browser, in the order its rail
 * lists them, with the colour each category's plate is painted. The live Add
 * device picker and the QA screen fixtures share this one list; it lives here
 * rather than in coros-screen-fixtures.tsx because that module is loaded lazily
 * and the live Grid must not pull it into the main bundle to reach it.
 */
export const COROS_DEVICE_CATEGORIES: ReadonlyArray<readonly [string, string]> = [
  ["Plugins", QC_COLORS.browserCategory.plugin],
  ["Amp", QC_COLORS.browserCategory.amp],
  ["Neural Capture", QC_COLORS.browserCategory.capture],
  ["Cab", QC_COLORS.browserCategory.cab],
  ["Overdrive", QC_COLORS.browserCategory.overdrive],
  ["Delay", QC_COLORS.browserCategory.delay],
  ["Reverb", QC_COLORS.browserCategory.reverb],
  ["Compressor", QC_COLORS.browserCategory.compressor],
  ["Pitch", QC_COLORS.browserCategory.pitch],
  ["Modulation", QC_COLORS.browserCategory.modulation],
  ["Morph", QC_COLORS.browserCategory.morph],
  ["Synth", QC_COLORS.browserCategory.synth],
  ["Filter", QC_COLORS.browserCategory.filter],
  ["Equalizer", QC_COLORS.browserCategory.equalizer],
  ["IR Loader", QC_COLORS.browserCategory.irLoader],
  ["Wah", QC_COLORS.browserCategory.wah],
  ["FX Loop", QC_COLORS.browserCategory.fxLoop],
  ["Looper", QC_COLORS.browserCategory.looper],
  ["Utility", QC_COLORS.browserCategory.utility],
];

export type DeviceBrowserInstrument = "guitar" | "bass";

/** Where one installed model sits in the device browser: its rail category and,
 *  for the three categories the unit splits, its GUITAR or BASS tab. */
export interface DeviceBrowserPlacement {
  category: string;
  instrument?: DeviceBrowserInstrument;
}

const categoryKey = (value: string) => value.toLowerCase().replace(/[^a-z0-9]/g, "");

/**
 * ModelEntry.category -> device browser placement.
 *
 * Every gateway publishes `listModels` categories in one curated display
 * vocabulary - quadcortex state.rs `display_category`, the virtual gateway's
 * `displayCategory` and the Python gateway's `_device_type_name` are the same
 * closed table - so the live values are "Guitar Amp", "Bass Amp", "Guitar
 * Overdrive", "Bass Overdrive", "Guitar Cab", "Bass Cab", "EQ", "IR Loader",
 * "Looper" and the single-word names the rail already uses. The raw ModelRepo
 * spellings those collapse ("Guitar Amplifier", "Cabsim Guitar (ST)",
 * "IRLoaders", "Loopers", "Equalizer") are listed too, keyed the same way, so a
 * catalogue that reached the UI unnormalised still lands in the right place.
 * Anything else (the ModelRepo's hidden routing and control categories) has no
 * rail entry and is reachable only from the all-categories list.
 */
const PLACEMENTS: Record<string, DeviceBrowserPlacement> = {
  guitaramp: { category: "Amp", instrument: "guitar" },
  guitaramplifier: { category: "Amp", instrument: "guitar" },
  amp: { category: "Amp", instrument: "guitar" },
  bassamp: { category: "Amp", instrument: "bass" },
  bassamplifier: { category: "Amp", instrument: "bass" },
  neuralcapture: { category: "Neural Capture" },
  guitarcab: { category: "Cab", instrument: "guitar" },
  guitarcabinet: { category: "Cab", instrument: "guitar" },
  cabsimguitarm: { category: "Cab", instrument: "guitar" },
  cabsimguitarst: { category: "Cab", instrument: "guitar" },
  basscab: { category: "Cab", instrument: "bass" },
  basscabinet: { category: "Cab", instrument: "bass" },
  cabsimbassm: { category: "Cab", instrument: "bass" },
  cabsimbassst: { category: "Cab", instrument: "bass" },
  guitaroverdrive: { category: "Overdrive", instrument: "guitar" },
  bassoverdrive: { category: "Overdrive", instrument: "bass" },
  delay: { category: "Delay" },
  reverb: { category: "Reverb" },
  compressor: { category: "Compressor" },
  pitch: { category: "Pitch" },
  modulation: { category: "Modulation" },
  morph: { category: "Morph" },
  synth: { category: "Synth" },
  filter: { category: "Filter" },
  eq: { category: "Equalizer" },
  equalizer: { category: "Equalizer" },
  irloader: { category: "IR Loader" },
  irloaders: { category: "IR Loader" },
  wah: { category: "Wah" },
  fxloop: { category: "FX Loop" },
  looper: { category: "Looper" },
  loopers: { category: "Looper" },
  utility: { category: "Utility" },
};

/** A plugin model belongs to Plugins whatever its effect category, as on the unit. */
export function deviceBrowserPlacement(model: Pick<ModelEntry, "category" | "pluginId">): DeviceBrowserPlacement | undefined {
  if (model.pluginId !== undefined && model.pluginId !== null) return { category: "Plugins" };
  return PLACEMENTS[categoryKey(model.category)];
}

/** The one search rule for installed models: name, category or based-on text. */
export function deviceBrowserMatches(model: Pick<ModelEntry, "name" | "category" | "basedOn">, query: string): boolean {
  const needle = query.trim().toLocaleLowerCase();
  return !needle || `${model.name} ${model.category} ${model.basedOn ?? ""}`.toLocaleLowerCase().includes(needle);
}

/** The instrument tabs a category offers: two only where it holds both. */
export function deviceBrowserInstruments(models: readonly ModelEntry[], category: string | undefined): DeviceBrowserInstrument[] {
  if (!category) return [];
  const present = new Set(models.map(deviceBrowserPlacement).filter((placement) => placement?.category === category).map((placement) => placement?.instrument));
  return present.has("guitar") && present.has("bass") ? ["guitar", "bass"] : [];
}

const CATEGORY_ORDER = new Map(COROS_DEVICE_CATEGORIES.map(([label], index) => [label, index]));
const byName = (left: ModelEntry, right: ModelEntry) => left.name.toLocaleLowerCase().localeCompare(right.name.toLocaleLowerCase()) || left.id - right.id;

/**
 * The rows the browser lists. With a category active, its models (on the
 * chosen instrument tab, when it has two) filtered by the search text; with none
 * active, every installed model in rail order, filtered the same way.
 */
export function deviceBrowserRows(models: readonly ModelEntry[], options: { category?: string; instrument?: DeviceBrowserInstrument; query?: string }): ModelEntry[] {
  const { category, instrument, query = "" } = options;
  const split = instrument !== undefined && deviceBrowserInstruments(models, category).length > 1;
  const rows = models.filter((model) => {
    const placement = deviceBrowserPlacement(model);
    if (category && placement?.category !== category) return false;
    if (split && placement?.instrument !== instrument) return false;
    return deviceBrowserMatches(model, query);
  });
  if (category) return rows.sort(byName);
  const rank = (model: ModelEntry) => CATEGORY_ORDER.get(deviceBrowserPlacement(model)?.category ?? "") ?? COROS_DEVICE_CATEGORIES.length;
  return rows.sort((left, right) => rank(left) - rank(right) || byName(left, right));
}

/**
 * Which half of the screen the browser takes. The unit's own frames
 * (device-browser-root and -models) open it over the right half with the target
 * slot lit in the washed left half; a target in the right half would sit under
 * the panel, so the panel moves to the left for those four columns.
 */
export function addDevicePickerSide(column: number): "left" | "right" {
  return column >= 4 ? "left" : "right";
}
