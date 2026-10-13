import type { LibraryEntries, LibraryEntry, LibraryFolder, PresetEntry, PresetFolder, PresetList } from "@qc-remote/client";

/**
 * The five things the unit's Directory category dropdown offers on firmware
 * 4.1.0, in the order it draws them (corpus `directory-categories`). The
 * firmware's Setlists tab is absent here because the unit under test does not
 * offer it.
 */
export type DirectoryCategory = "favorites" | "presets" | "captures" | "irs" | "plugins";

export const DIRECTORY_CATEGORIES: readonly { id: DirectoryCategory; label: string }[] = [
  { id: "favorites", label: "Favorites and Recent" },
  { id: "presets", label: "Presets" },
  { id: "captures", label: "Neural Captures" },
  { id: "irs", label: "Impulse Responses" },
  { id: "plugins", label: "Plugin Presets" }
];

/** The device library reads the gateway can serve. Plugin presets have none. */
export type DirectoryLibraryKind = "favorites" | "recents" | "captures" | "irs";

export interface DirectoryLibraryState {
  favorites?: LibraryEntries;
  recents?: LibraryEntries;
  captures?: LibraryEntries;
  irs?: LibraryEntries;
  /** Reads in flight. */
  loading: readonly DirectoryLibraryKind[];
  /** Why a read failed, by kind; absent while it has not failed. */
  errors: Partial<Record<DirectoryLibraryKind, string>>;
}

/** Stateful Directory data and actions supplied by the preset workflow. */
export interface PresetDirectoryState {
  /** Physical transport owner; changing it retires local row actions. */
  interactionSessionKey?: unknown;
  open: boolean;
  list?: PresetList;
  loading: boolean;
  /** The device has not finished sending the catalog the user asked for. */
  pending?: boolean;
  disabled: boolean;
  onClose: () => void;
  /** Reports the search field opening and closing (and closing when the Directory goes away), so the unit's can follow. Must be stable. */
  onSearchOpenChange?: (open: boolean) => void;
  onRefresh: () => void;
  onRecall: (entry: PresetEntry) => void;
  onSelectSetlist: (setlistKey: string) => void;
  onNewSetlist: () => void;
  onPresetAction: (action: "upload" | "edit" | "copy" | "cut" | "paste" | "delete", entry: PresetEntry) => void;
  onPresetBatchAction: (action: "copy" | "delete", entries: PresetEntry[]) => void;
  onArrangePreset: (source: PresetEntry, destination: PresetEntry) => void;
  /** Device library reads behind the Favorites, Neural Captures and Impulse Responses categories. */
  library?: DirectoryLibraryState;
  onLoadLibrary?: (kind: DirectoryLibraryKind) => void;
  /** Preset dates by product key from setlists read only to date Recent rows. */
  setlistDates?: ReadonlyMap<string, string | number>;
  /** Read these setlists (once each) for the dates of Recent rows that have none. */
  onLoadSetlistDates?: (folderKeys: readonly string[]) => void;
  /**
   * Whether the unit is signed in to a Neural DSP cloud account. The host is
   * not told, so it is left unset, and an unset state draws what the unit draws
   * for a signed-out account: the person-with-a-red-cross glyph, not the
   * upload cloud (walk of firmware 4.1.0, 2026-10-03).
   */
  cloudSignedIn?: boolean;
}

export const EMPTY_DIRECTORY_LIBRARY: DirectoryLibraryState = { loading: [], errors: {} };

/** The reads a category needs before it can draw. */
export function libraryKindsForCategory(category: DirectoryCategory, favoritesTab: "favorites" | "recents" = "favorites"): readonly DirectoryLibraryKind[] {
  if (category === "captures") return ["captures"];
  if (category === "irs") return ["irs"];
  if (category === "favorites") return [favoritesTab === "recents" ? "recents" : "favorites"];
  return [];
}

const MY_PRESETS = "my presets";

/**
 * The unit numbers its user folders by row, from 1, with the factory folder
 * fixed at 0 and the default "My Presets" always at 1 (corpus
 * `directory-categories` and a walk of the live unit, where "My Presets 2" is
 * the seventh row and carries 7). The number is therefore the display index,
 * so the order has to be the one the rows are drawn in.
 *
 * The gateway projects the setlists alphabetically, because that is how the
 * desktop application lists them; the unit's own order beyond "My Presets" is
 * not something the broker is told, so everything after the pinned folder
 * keeps the order it arrived in.
 */
export function orderUserFolders<T extends Pick<PresetFolder, "name">>(folders: readonly T[]): T[] {
  const pinned = folders.filter((folder) => folder.name.trim().toLowerCase() === MY_PRESETS);
  return [...pinned, ...folders.filter((folder) => !pinned.includes(folder))];
}

export interface LibraryEntryGroup {
  key: string;
  name: string;
  count: number;
}

/** Child folders of a library, in the order the device published them. */
export function libraryChildFolders(result: LibraryEntries | undefined): LibraryFolder[] {
  return (result?.folders ?? []).filter((folder) => !folder.isLibrary && !folder.isDownloads);
}

/**
 * The capture folder the unit stamps with its downloaded badge: My Captures,
 * key `1_q` in the device's own static key map, the only folder with the badge
 * in the Neural Captures frame. Factory packs, the library root and the
 * Downloads provider never carry it.
 */
export function isDownloadFolder(folder: Pick<LibraryFolder, "key" | "name" | "isFactory" | "isLibrary" | "isDownloads">): boolean {
  if (folder.isFactory || folder.isLibrary || folder.isDownloads) return false;
  return folder.key === "1_q" || folder.name.trim().toLowerCase() === "my captures";
}

export interface LibraryFolderRow {
  folder: LibraryFolder;
  /** 1 for a folder directly under the library root. */
  depth: number;
}

/**
 * The folder column under the library row, as the unit draws it: only the
 * folders directly under the library root, with a folder's own children shown
 * beneath it once it (or something inside it) is the selected folder. The
 * device publishes the whole tree flat - the factory capture tree alone is
 * 179 folders - so drawing every folder as a sibling buried the three real
 * top-level folders under their grandchildren.
 */
export function libraryFolderRows(result: LibraryEntries | undefined, selectedKey: string | undefined): LibraryFolderRow[] {
  const folders = result?.folders ?? [];
  const known = new Set(folders.map((folder) => folder.key));
  const library = folders.find((folder) => folder.isLibrary);
  const children = new Map<string | undefined, LibraryFolder[]>();
  for (const folder of folders) {
    if (folder.isLibrary || folder.isDownloads) continue;
    const parent = folder.parentKey && known.has(folder.parentKey) && folder.parentKey !== library?.key ? folder.parentKey : undefined;
    const siblings = children.get(parent) ?? [];
    siblings.push(folder);
    children.set(parent, siblings);
  }
  const open = new Set<string>();
  const parentOf = new Map(folders.map((folder) => [folder.key, folder.parentKey]));
  for (let key = selectedKey, guard = 0; key && guard < 64; key = parentOf.get(key), guard += 1) open.add(key);
  const rows: LibraryFolderRow[] = [];
  const walk = (parent: string | undefined, depth: number, trail: ReadonlySet<string>) => {
    for (const folder of children.get(parent) ?? []) {
      if (trail.has(folder.key)) continue;
      rows.push({ folder, depth });
      if (open.has(folder.key)) walk(folder.key, depth + 1, new Set([...trail, folder.key]));
    }
  };
  walk(undefined, 1, new Set());
  return rows;
}

/** The keys of one folder and everything beneath it. */
export function libraryFolderSubtree(result: LibraryEntries | undefined, key: string): Set<string> {
  const folders = result?.folders ?? [];
  const subtree = new Set([key]);
  for (let grew = true; grew;) {
    grew = false;
    for (const folder of folders) {
      if (folder.parentKey && subtree.has(folder.parentKey) && !subtree.has(folder.key)) { subtree.add(folder.key); grew = true; }
    }
  }
  return subtree;
}

/** The entries one library folder holds, and those of its sub-folders; no folder selected means the whole library. */
export function libraryEntriesIn(result: LibraryEntries | undefined, folderKey: string | undefined): LibraryEntry[] {
  const entries = result?.entries ?? [];
  if (!folderKey) return entries;
  const subtree = libraryFolderSubtree(result, folderKey);
  return entries.filter((entry) => entry.folderKey !== undefined && subtree.has(entry.folderKey));
}

/**
 * The rows the unit draws for a library (or one folder of it).
 *
 * The device publishes the capture tree as a folder per factory pack plus the
 * flat library, so one capture can arrive as several records and some of them
 * carry only a key and a folder: no name. Drawing every record gave a count of
 * 8378 against the unit's 2127 and a column of nameless rows. So a nameless
 * record borrows the name, author and date a named record of the same key
 * carries, a record still without a name is not a row at all (the unit has
 * nothing to draw for it), and one key is one row however many folders list it.
 */
export function displayLibraryEntries(result: LibraryEntries | undefined, folderKey: string | undefined): LibraryEntry[] {
  const named = new Map<string, LibraryEntry>();
  for (const entry of result?.entries ?? []) if (entry.name.trim() && !named.has(entry.key)) named.set(entry.key, entry);
  const rows = new Map<string, LibraryEntry>();
  for (const entry of libraryEntriesIn(result, folderKey)) {
    if (rows.has(entry.key)) continue;
    const donor = entry.name.trim() ? entry : named.get(entry.key);
    if (!donor) continue;
    rows.set(entry.key, entry.name.trim() ? entry : { ...entry, name: donor.name, author: entry.author ?? donor.author, date: entry.date ?? donor.date, description: entry.description ?? donor.description, version: entry.version ?? donor.version, instrument: entry.instrument ?? donor.instrument });
  }
  return [...rows.values()];
}

/** The count beside the library row: the rows the unit would draw with no folder selected. */
export function libraryRowCount(result: LibraryEntries | undefined): number {
  return displayLibraryEntries(result, undefined).length;
}

/** The labels of the A-Z scrubber down the unit's capture list, and the letter each one jumps to. */
export const DIRECTORY_ALPHA_INDEX: readonly { label: string; target: string }[] = [
  { label: "#", target: "#" }, { label: "•", target: "A" }, { label: "A", target: "A" }, { label: "•", target: "E" },
  { label: "I", target: "I" }, { label: "•", target: "M" }, { label: "R", target: "R" }, { label: "•", target: "V" }, { label: "Z", target: "Z" }
];

/** The first entry at or after a scrubber letter, or the last entry when the letter is past them all. */
export function alphaIndexTarget(entries: readonly { name: string }[], letter: string): number {
  if (!entries.length) return -1;
  const bucket = (name: string) => { const first = name.trim().charAt(0).toLocaleUpperCase(); return first >= "A" && first <= "Z" ? first : "#"; };
  const index = entries.findIndex((entry) => { const b = bucket(entry.name); return letter === "#" ? b === "#" : b !== "#" && b >= letter; });
  return index < 0 ? entries.length - 1 : index;
}

/** Case-insensitive filter shared by every library list's search field. */
export function filterLibraryEntries(entries: readonly LibraryEntry[], query: string): LibraryEntry[] {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return [...entries];
  return entries.filter((entry) => `${entry.name} ${entry.author ?? ""}`.toLocaleLowerCase().includes(needle));
}

/** The unit lists captures and impulse responses by name, digits and symbols first, so the A-Z scrubber has an order to jump through. */
export function sortLibraryEntries<T extends { name: string }>(entries: readonly T[]): T[] {
  const bucket = (name: string) => { const first = name.trim().charAt(0).toLocaleUpperCase(); return first >= "A" && first <= "Z" ? first : "#"; };
  return [...entries].sort((left, right) => {
    const a = bucket(left.name), b = bucket(right.name);
    if (a !== b) return a === "#" ? -1 : b === "#" ? 1 : a < b ? -1 : 1;
    return left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: "base" });
  });
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"] as const;

/**
 * The unit's date line under a Recent or Favorite row: "03 Oct 2026". Takes the
 * date string the device publishes or its epoch milliseconds; anything that is
 * not a real date draws nothing rather than an invented line.
 */
export function formatDirectoryDate(date: string | number | undefined): string | undefined {
  if (date === undefined || date === "") return undefined;
  const parsed = new Date(date);
  if (!Number.isFinite(parsed.getTime())) return undefined;
  // Month names are spelled out here: the locale's own "short" form is "Sept" in
  // some engines and "Sep" in others, and the unit always draws three letters.
  const day = String(parsed.getDate()).padStart(2, "0");
  return `${day} ${MONTHS[parsed.getMonth()]} ${parsed.getFullYear()}`;
}

/**
 * The line under a row's name. The unit draws a date there on the Recent tab
 * only; a Favorites row is the name and its two buttons and nothing else (the
 * device frame carries no date on Favorites, and no author either), a library
 * list shows the author, and nothing is drawn that the unit was not told.
 */
export function entryDetail(entry: LibraryEntry, detail: "author" | "date" | "none", dates?: ReadonlyMap<string, string | number>): string | undefined {
  if (detail === "none") return undefined;
  if (detail === "date") return formatDirectoryDate((entry.productKey ? dates?.get(entry.productKey) : undefined) ?? entry.date);
  return entry.author || undefined;
}

/** What the signed-in state of the unit's cloud account is, in one place: the header glyph and the Cloud rows both read it. */
export function isCloudSignedOut(cloudSignedIn: boolean | undefined): boolean {
  return cloudSignedIn !== true;
}

/** The unit draws the multi-select, sort and search tools on the Favorites tab only; the Recent tab is the tabs and the tick. */
export function favoritesShowsTools(tab: "favorites" | "recents"): boolean {
  return tab === "favorites";
}
