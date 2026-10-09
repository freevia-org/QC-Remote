export type CorOsContextAction = "create-new" | "edit-details" | "copy-scene" | "swap-scene" | "preset-midi-out" | "favorite" | "delete-preset" | "new-capture" | "modes-configuration" | "tempo" | "cpu-monitor" | "settings";

export const COROS_CONTEXT_ACTION_LABELS: Readonly<Record<CorOsContextAction, string>> = {
  "create-new": "Create New",
  "edit-details": "Edit Details",
  "copy-scene": "Copy Scene A",
  "swap-scene": "Swap Scene A",
  "preset-midi-out": "Preset MIDI Out",
  favorite: "Add to favorites",
  "delete-preset": "Delete preset",
  "new-capture": "New Neural Capture",
  "modes-configuration": "Modes Configuration",
  tempo: "Tempo",
  "cpu-monitor": "CPU monitor",
  settings: "Settings"
};

/** Keep unsupported CorOS menu feedback identical across native shells. */
export function corOsUnavailableContextActionMessage(action: CorOsContextAction): string {
  return `${COROS_CONTEXT_ACTION_LABELS[action]} is shown in the device-accurate Grid menu, but this command is not exposed by the current USB gateway.`;
}

/** Grid gaps before columns 0-7, followed by the gap after column 7. */
export const GRID_ROUTE_BOUNDARIES = [75, 141, 228.5, 317, 404.5, 488, 572, 659.5, 748] as const;

// The unit's long preset title runs from x 164 to x 565 on grid-base -
// 402px of ink, 38px tall. 596 stretched the same string to 434x41.
export const PRESET_TITLE_RIGHT_EDGE = 564;

/* The unit tracks the bank/slot block looser than this font's own advances.
   grid-base puts "32H" at ink 13, 57 and 101 where -1 tracking puts ours at
   13, 55 and 98 - the glyphs are the right widths, so only the gap between
   them is short, by 1.6px per glyph across the header's 1.012 x-scale. */
export const PRESET_LOCATION_TRACKING = 0.5;

export function presetTitlePresentation(name: string, dirty: boolean) {
  const normalizedName = name.trim() || "Unsaved";
  // CorOS can publish the dirty marker in the displayed name before the
  // dedicated dirty event reaches the host. The marker is presentation, not
  // preset identity: normalize it once and italicize the complete title when
  // either signal says the working buffer is modified.
  const nameCarriesDirtyMarker = normalizedName.endsWith("*");
  const cleanName = nameCarriesDirtyMarker
    ? normalizedName.slice(0, -1).trimEnd() || "Unsaved"
    : normalizedName;
  const effectiveDirty = dirty || nameCarriesDirtyMarker;
  const unsaved = cleanName.toLocaleLowerCase() === "unsaved";
  return {
    text: `${cleanName}${effectiveDirty ? "*" : ""}`,
    dimmed: unsaved && !effectiveDirty,
    italic: effectiveDirty
  };
}

export function presetTitleLayout(locationWidth: number, titleWidthAtFullSize: number) {
  // On grid-base the unit's "32H" ink ends at x 141 and the title's begins at
  // 164. locationWidth is the block's rendered advance, tracking included, so
  // the same gutter serves a one-digit and a two-digit bank; the two values
  // this once carried differed only by the tracking the caller now folds in.
  const gutter = 16;
  const start = 14 + Math.max(0, locationWidth) + gutter;
  const maxWidth = Math.max(180, PRESET_TITLE_RIGHT_EDGE - start);
  const fontSize = Math.max(22, Math.min(68, 68 * maxWidth / Math.max(1, titleWidthAtFullSize)));
  return {
    start,
    gutter,
    maxWidth,
    fontSize,
    squeeze: titleWidthAtFullSize * fontSize / 68 > maxWidth,
    baseline: 75 - (68 - fontSize) * .35
  };
}

export function splitAnchorX(splitColumn: number): number {
  return GRID_ROUTE_BOUNDARIES[Math.max(0, Math.min(7, splitColumn))];
}

/** mixColumn is the last cell in the branch, so its marker sits after it. */
export function mixAnchorX(mixColumn: number): number {
  return GRID_ROUTE_BOUNDARIES[Math.max(0, Math.min(8, mixColumn + 1))];
}

export function openSplitPath(splitX: number, fromY: number, toY: number, rowStartX = 52): string {
  const middleY = (fromY + toY) / 2;
  const radius = 9;
  return `M${splitX} ${fromY}V${middleY - radius}Q${splitX} ${middleY} ${splitX - radius} ${middleY}H${rowStartX + radius}Q${rowStartX} ${middleY} ${rowStartX} ${middleY + radius}V${toY - radius}Q${rowStartX} ${toY} ${rowStartX + radius} ${toY}`;
}

/** Mirror of openSplitPath for the Mixer return shown in the QC Grid. */
export function rejoinSplitPath(mixX: number, fromY: number, toY: number, rowEndX = 748): string {
  const radius = 9;
  // A mixer on the final boundary has no horizontal space for the ordinary
  // outward loop. Drawing that loop would reverse over itself and produce a
  // visible pulse, so descend directly and round into the lower rail.
  if (mixX === rowEndX) {
    return `M${mixX} ${fromY}V${toY - radius}Q${rowEndX} ${toY} ${rowEndX - radius} ${toY}`;
  }
  const middleY = (fromY + toY) / 2;
  return `M${mixX} ${fromY}V${middleY - radius}Q${mixX} ${middleY} ${mixX + radius} ${middleY}H${rowEndX - radius}Q${rowEndX} ${middleY} ${rowEndX} ${middleY + radius}V${toY - radius}Q${rowEndX} ${toY} ${rowEndX - radius} ${toY}`;
}

export function gridBlocksByRow<T extends { row: number; column: number }>(blocks: T[], rowCount = 4): T[][] {
  return Array.from({ length: rowCount }, (_, row) => blocks
    .filter((block) => block.row === row)
    .sort((left, right) => left.column - right.column));
}

export function rowHasVisibleSignalRail(blockCount: number, route?: { input?: string; output?: string; splitColumn?: number; mixColumn?: number }): boolean {
  return blockCount > 0
    || Boolean(route?.input && route.input !== "Internal")
    || Boolean(route?.output && route.output !== "Internal")
    || route?.splitColumn !== undefined
    || route?.mixColumn !== undefined;
}

export function routedPortIsPlugged(
  side: "input" | "output",
  routeId: number | undefined,
  ports: readonly { kind: string; id: number; plugged: boolean }[] | undefined
): boolean | undefined {
  if (routeId === undefined || routeId === 0 || !ports?.length) return undefined;
  const members = side === "input"
    ? ({ 3: [1, 2], 6: [4, 5] } as Record<number, number[]>)[routeId] ?? [routeId]
    : ({ 1: [1, 4, 5], 2: [2, 6, 7], 3: [3, 8, 9], 19: [1, 2, 3, 4, 5, 6, 7, 8, 9] } as Record<number, number[]>)[routeId] ?? [routeId];
  const kind = side;
  const relevant = ports.filter((port) => port.kind === kind && members.includes(port.id));
  // The mark follows the physical jacks only. The app is itself on USB, so
  // counting the USB link drew the mark on every connected unit over Multi Out
  // (side-by-side walk, grid0) and, on app 0.1.174, over a USB 3 output chip
  // too; the unit draws neither. Every corpus Grid frame was captured with the
  // host on USB and carries no mark on any port chip, so a USB-named route
  // (inputs 8-13, outputs 10-15, 20-22) has no jack to reflect and stays unmarked.
  return relevant.length ? relevant.some((port) => port.plugged) : undefined;
}

export const GRID_CONTEXT_MENU = [
  { label: "Create New", icon: "create-new", action: "create-new" },
  // The device draws three periods here, not a typographic ellipsis; the
  // CorOS graphics tree for `grid-context-menu` reads 'Save as...'.
  { label: "Save as...", icon: "save-as", action: "save-as" },
  { label: "Edit Details", icon: "edit", action: "edit-details" },
  { label: "Copy Scene A", icon: "file", action: "copy-scene" },
  { label: "Swap Scene A", icon: "next", action: "swap-scene" },
  { label: "Preset MIDI Out", icon: "midi", action: "preset-midi-out" },
  { label: "Add to favorites", icon: "favorite", action: "favorite" },
  { label: "Delete Preset", icon: "delete", action: "delete-preset", danger: true },
  { label: "New Neural Capture", icon: "capture", action: "new-capture" },
  { label: "Modes Configuration", icon: "modes", action: "modes-configuration" },
  { label: "Tempo", icon: "tempo", action: "tempo" },
  { label: "CPU Monitor", icon: "cpu", action: "cpu-monitor" },
  { label: "Settings", icon: "settings", action: "settings" }
] as const;

export const DIRECTORY_PRESET_CONTEXT_MENU = [
  { label: "Edit", action: "edit", requiresPreset: true },
  { label: "Copy", action: "copy", requiresPreset: true },
  { label: "Cut", action: "cut", requiresPreset: true },
  { label: "Paste", action: "paste" },
  { label: "Delete", action: "delete", danger: true, requiresPreset: true }
] as const;
