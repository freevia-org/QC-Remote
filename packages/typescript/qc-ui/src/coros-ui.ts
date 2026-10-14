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

/* The header's scene badge and mode label, measured on the SCENE unit frame and
   on grid-base (STOMP), which agree to the pixel. The badge is a 24px square at
   (656,12) - 24 yellow columns and rows, not 25 - holding a 23px letter centred
   on x 668 with its baseline at 33. The mode label is 22px IBM Plex Bold at
   x 692, baseline 80 (not 21.5: its caps are 16 rows tall on both frames). The
   SCENE glyph's ink starts at row 60 of its 26-unit box, so the ink-centred
   position is one row lower than the box-centred one STOMP uses. */
export const SCENE_BADGE = { x: 656, y: 12, size: 24, radius: 3, letterX: 668, letterBaseline: 33, letterSize: 23 } as const;
export const MODE_LABEL = { x: 692, baseline: 80, size: 22 } as const;
export const MODE_GLYPH_CENTRE = { x: 668, y: 72 } as const;
/** Rows the ink of a mode glyph sits below the centre of its box (the SCENE art leaves its last row and first column empty). */
export function modeGlyphInkRise(mode: string): number {
  return mode === "SCENE" ? 1 : 0;
}

/* A port pill's label, as the unit draws it. Measured on grid-base and nine
   more corpus frames: two lines of 11px caps (a 16px IBM Plex face) whose tops
   are 21px apart, the pair's ink centred 1.5px above the row centre
   ("Multi" spans 33px). The label is an SVG text whose first baseline is
   railLabelBaseline(row centre, line count). */
export const RAIL_LABEL_FONT_SIZE = 16;
export const RAIL_LABEL_LINE_STEP = 21;
export function railLabelBaseline(centreY: number, lineCount: number): number {
  return centreY + 4.5 - (lineCount - 1) * (RAIL_LABEL_LINE_STEP / 2);
}

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
  const text = `${cleanName}${effectiveDirty ? "*" : ""}`;
  // The unit sizes the title from the string it was sent, so a clean name that
  // ends in a space is fitted with that space even though the space draws
  // nothing. A dirty title is fitted as drawn: no frame shows where the marker
  // sits relative to such a space.
  const trailingSpace = effectiveDirty || !name.trim() ? "" : name.slice(name.trimEnd().length);
  return {
    text,
    fitText: `${text}${trailingSpace}`,
    dimmed: unsaved && !effectiveDirty,
    italic: effectiveDirty
  };
}

/* The Grid header, measured on two unit frames at 800x480 (a one-digit and a
   three-character bank) and the five others that share it. The bank and slot
   are one 70px run at x 12, baseline 77, with no tracking: "7F" puts its 7 at
   ink 16 and its F at 60, and "32H" puts its digits at 13, 57 and 101, which
   are IBM Plex Bold's own 42px advances. The name starts 15px after that run
   (110 after "7F", 162 after "32H"), and a render of the same string at the
   fitted size, origin and baseline differs from the unit's pixels by under
   three levels on average.

   The name's size is an integer, as in every unit frame: 50 and 50 and 45 on
   three names whose natural widths differ by 14%, none in between. It is the
   largest integer whose advance still ends by x 558.5 - three frames bracket
   that edge to within a pixel (the 45px name ends at 550.7..560.5, the first
   50px at 559.9..568.8, the 39px one at 558.5..568.7). The baseline follows
   the size down from the bank's 77 by 0.375 of the size lost - 69 at 50, 68
   at 45, 66 at 40 - and sits on whole pixels, a half going up. */
export const PRESET_BANK_SIZE = 70;
export const PRESET_BANK_X = 12;
export const PRESET_BANK_BASELINE = 77;
export const PRESET_TITLE_GUTTER = 15;
export const PRESET_TITLE_MIN_SIZE = 22;
export const GRID_TITLE_RIGHT_EDGE = 558.5;
export const PRESET_TITLE_BASELINE_RISE = 0.375;

export function presetTitleLayout(locationWidth: number, titleWidthAtFullSize: number) {
  // locationWidth is the bank run's advance at PRESET_BANK_SIZE, so one gutter
  // serves a one-digit and a two-digit bank alike.
  const gutter = PRESET_TITLE_GUTTER;
  const start = PRESET_BANK_X + Math.max(0, locationWidth) + gutter;
  const maxWidth = Math.max(180, GRID_TITLE_RIGHT_EDGE - start);
  // Advances scale linearly with size, so the largest integer that fits is a floor.
  const fitted = Math.floor(PRESET_BANK_SIZE * maxWidth / Math.max(1, titleWidthAtFullSize) + 1e-6);
  const fontSize = Math.max(PRESET_TITLE_MIN_SIZE, Math.min(PRESET_BANK_SIZE, fitted));
  return {
    start,
    gutter,
    maxWidth,
    fontSize,
    squeeze: titleWidthAtFullSize * fontSize / PRESET_BANK_SIZE > maxWidth,
    baseline: Math.ceil(PRESET_BANK_BASELINE - (PRESET_BANK_SIZE - fontSize) * PRESET_TITLE_BASELINE_RISE - 0.5)
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
