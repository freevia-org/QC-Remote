import type { GridBlock } from "@qc-remote/client";

export interface PublishedPluginBadge {
  abbreviation: string;
  name: string;
  aliases: readonly string[];
}

/** Published PCOM badge vocabulary, independent from all block artwork. */
export const PUBLISHED_PLUGIN_BADGES: readonly PublishedPluginBadge[] = [
  { abbreviation: "PLI", name: "Archetype: Plini X", aliases: ["plini-x", "plinix", "plini"] },
  { abbreviation: "GOJ", name: "Archetype: Gojira X", aliases: ["gojira-x", "gojirax", "gojira"] },
  { abbreviation: "SLO", name: "Soldano SLO-100 X", aliases: ["slo100-x", "slo100x", "soldano", "slo-100"] },
  { abbreviation: "NAM", name: "Fortin Nameless Suite X", aliases: ["nameless-x", "namelessx", "nameless"] },
  { abbreviation: "WON", name: "Archetype: Cory Wong X", aliases: ["cory-x", "coryx", "cory-wong", "cory wong", "neural_dsp_cory_wong"] },
  { abbreviation: "NLY", name: "Archetype: Nolly X", aliases: ["nolly-x", "nollyx", "nolly"] },
  { abbreviation: "PLX", name: "Parallax X", aliases: ["parallax-x", "parallaxx", "parallax"] },
  { abbreviation: "MAY", name: "Archetype: John Mayer X", aliases: ["mayer-x", "mayerx", "john-mayer", "john mayer", "neural_dsp_mayer"] },
  { abbreviation: "PET", name: "Archetype: Petrucci X", aliases: ["petrucci-x", "petruccix", "petrucci"] },
  { abbreviation: "MSH", name: "Archetype: Misha Mansoor X", aliases: ["misha-x", "mishax", "misha-mansoor", "misha mansoor"] },
  { abbreviation: "RAB", name: "Archetype: Rabea X", aliases: ["rabea-x", "rabeax", "rabea"] },
  { abbreviation: "HEN", name: "Archetype: Tim Henson X", aliases: ["henson-x", "hensonx", "tim-henson", "tim henson"] }
] as const;

const normalizedPluginIdentity = (value: string): string => value.toLowerCase().replace(/[^a-z0-9]+/g, "");

/** Resolves badge text from device identity; it never alters the base tile. */
export function pluginBadge(block: GridBlock): string | undefined {
  if (block.plugin !== true && !`${block.category ?? ""}`.toLowerCase().includes("plugin")) return undefined;
  const identities = [block.pluginId, block.name].filter((value): value is string => Boolean(value)).map(normalizedPluginIdentity);
  for (const published of PUBLISHED_PLUGIN_BADGES) {
    if (published.aliases.some((alias) => {
      const normalizedAlias = normalizedPluginIdentity(alias);
      return identities.some((identity) => identity === normalizedAlias || identity.includes(normalizedAlias));
    })) return published.abbreviation;
  }
  const fallback = normalizedPluginIdentity(block.pluginId ?? "").replace(/(?:archetype|neuraldsp|suite|plugin|x)$/g, "");
  return fallback ? fallback.slice(0, 3).toUpperCase() : undefined;
}
