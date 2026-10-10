import { useRef, type CSSProperties, type ReactNode } from "react";
import type { LibraryEntry } from "@qc-remote/client";
import { alphaIndexTarget, DIRECTORY_ALPHA_INDEX, DIRECTORY_CATEGORIES, displayLibraryEntries, entryDetail, filterLibraryEntries, isCloudSignedOut, isDownloadFolder, libraryFolderRows, libraryRowCount, sortLibraryEntries, type DirectoryCategory, type DirectoryLibraryKind, type DirectoryLibraryState } from "./directory-library-model";
import { QcDeviceCategoryGlyph } from "./device-category-glyph";
import { DirectoryDownloadBadge, QcDirectoryIcon, QcLibraryIcon, QcModeGlyph, QcScreenVectorGlyph, QcUiIcon } from "./theme-icons";

export function DirectoryCategoryGlyph({ category }: { category: DirectoryCategory }) {
  if (category === "favorites") return <QcLibraryIcon kind="heart" />;
  if (category === "presets") return <QcDirectoryIcon kind="grid" />;
  if (category === "captures") return <QcLibraryIcon kind="capture-header" />;
  if (category === "irs") return <QcLibraryIcon kind="impulse-response" />;
  return <QcDeviceCategoryGlyph label="Plugins" />;
}

function CategoryMenuGlyph({ category }: { category: DirectoryCategory }) {
  if (category === "presets") return <QcModeGlyph mode="PRESET" />;
  if (category === "plugins") return <QcScreenVectorGlyph icon="interface.plugin-presets" />;
  return <DirectoryCategoryGlyph category={category} />;
}

/** The black dropdown the Directory's category chip opens. */
export function DirectoryCategoryMenu({ category, onSelect, onClose }: { category: DirectoryCategory; onSelect: (category: DirectoryCategory) => void; onClose: () => void }) {
  return <>
    <button type="button" className="directory-context-scrim" aria-label="Close directory categories" onClick={onClose} />
    <aside className="directory-category-menu" role="menu" aria-label="Directory categories">
      {DIRECTORY_CATEGORIES.map((item) => <button key={item.id} type="button" role="menuitemradio" aria-checked={item.id === category} className={item.id === category ? "is-active" : undefined} onClick={() => onSelect(item.id)}>
        <span><CategoryMenuGlyph category={item.id} /></span>{item.label}{item.id === category && <b className="qc-selection-tick"><QcUiIcon kind="check" /></b>}
      </button>)}
    </aside>
  </>;
}

/**
 * The Downloads and Cloud rows at the top of a Directory folder column. The
 * unit greys the Cloud row while no cloud account is signed in - the same
 * state the header's person-with-a-red-cross glyph reports - and a greyed row
 * does nothing; Downloads is drawn as a normal row the gateway cannot read.
 */
export function DirectoryProviderRow({ kind, label, signedOut }: { kind: "download" | "cloud"; label: string; signedOut: boolean }) {
  const greyed = kind === "cloud" && signedOut;
  const title = greyed ? "Not signed in to a Neural DSP cloud account on the Quad Cortex." : kind === "cloud" ? `${label} are managed on the Quad Cortex.` : "Downloads are managed on the Quad Cortex.";
  return <button type="button" disabled aria-disabled="true" className={greyed ? "is-signed-out" : undefined} title={title}><span><QcDirectoryIcon kind={kind} /></span>{label}</button>;
}

/**
 * `unfavorite` draws the broken-heart button the unit puts after the binoculars on
 * every Favorites row; nothing in this app removes a favorite from the Directory
 * yet, so it is drawn disabled.
 *
 * `menu` draws the unit's three-dot button at the right of every row of the
 * Neural Captures list. Its actions are not published through the device API,
 * so it is drawn disabled - the mark is the unit's, the actions are not ours to
 * invent. The count the unit draws beside it has no field in the catalog and is
 * not drawn.
 */
function EntryList({ entries, empty, detail = "author", dates, onLocate, menu, unfavorite }: { entries: readonly LibraryEntry[]; empty: string; detail?: "author" | "date" | "none"; dates?: ReadonlyMap<string, string | number>; onLocate?: (entry: LibraryEntry) => void; menu?: boolean; unfavorite?: boolean }) {
  if (!entries.length) return <div className="directory-loading">{empty}</div>;
  return <>{entries.map((entry, index) => {
    const detailText = entryDetail(entry, detail, dates);
    return <div key={`${entry.folderKey ?? ""}/${entry.productKey ?? entry.key}/${index}`} role="option" aria-selected={false} className={`directory-library-row${onLocate ? " has-locate" : ""}${menu ? " has-more" : ""}${unfavorite ? " has-unfavorite" : ""}${detailText ? "" : " is-name-only"}`}>
    <strong>{entry.name}</strong>
    {detailText && <small>{detailText}</small>}
    {onLocate && <button type="button" className="directory-locate" aria-label={`Show ${entry.name} in Presets`} onClick={() => onLocate(entry)}><QcLibraryIcon kind="binoculars" /></button>}
    {unfavorite && <button type="button" className="directory-locate directory-unfavorite" disabled aria-label={`Remove ${entry.name} from Favorites on the Quad Cortex.`} title="Removing a favorite is not available from here yet; the heart in the Quad Cortex's own Directory does it."><QcLibraryIcon kind="broken-heart" /></button>}
    {menu && <button type="button" className="preset-row-more directory-library-more" disabled tabIndex={-1} aria-label={`Actions for ${entry.name} are managed on the Quad Cortex.`}><QcUiIcon kind="more" /></button>}
  </div>;
  })}</>;
}

function ReadStatus({ kind, library, label }: { kind: DirectoryLibraryKind; library: DirectoryLibraryState; label: string }) {
  if (library.loading.includes(kind) && !library[kind]) return <div className="directory-loading">{`READING ${label} FROM QUAD CORTEX…`}</div>;
  const error = library.errors[kind];
  if (error && !library[kind]) return <div className="directory-loading" role="alert">{error}</div>;
  return null;
}

/**
 * Neural Captures and Impulse Responses: the unit's two-pane library browser.
 * A disabled Downloads/Cloud row stands in for the providers the gateway cannot
 * read, exactly as the Presets browser does.
 */
function LibraryBrowser({ kind, library, folderKey, onSelectFolder, query, searchField, cloudSignedIn }: { kind: "captures" | "irs"; library: DirectoryLibraryState; folderKey?: string; onSelectFolder: (key: string | undefined) => void; query: string; searchField?: ReactNode; cloudSignedIn?: boolean }) {
  const result = library[kind];
  const isCapture = kind === "captures";
  const folderRows = libraryFolderRows(result, folderKey);
  const entries = sortLibraryEntries(filterLibraryEntries(displayLibraryEntries(result, folderKey), query));
  const status = ReadStatus({ kind, library, label: isCapture ? "NEURAL CAPTURES" : "IMPULSE RESPONSES" });
  const listRef = useRef<HTMLDivElement>(null);
  const jumpTo = (letter: string) => {
    const index = alphaIndexTarget(entries, letter);
    if (index >= 0) listRef.current?.querySelectorAll<HTMLElement>(".directory-library-row")[index]?.scrollIntoView({ block: "start" });
  };
  return <div className="coros-directory-body is-library">
    <nav className="directory-folders qc-scroll-rail" aria-label={isCapture ? "Capture folders" : "Impulse response folders"}>
      {isCapture && <DirectoryProviderRow kind="download" label="Downloads" signedOut={isCloudSignedOut(cloudSignedIn)} />}
      <DirectoryProviderRow kind="cloud" label={isCapture ? "Cloud Captures" : "Cloud IRs"} signedOut={isCloudSignedOut(cloudSignedIn)} />
      <button type="button" className={!folderKey ? "is-active" : undefined} onClick={() => onSelectFolder(undefined)}><span><QcLibraryIcon kind="capture-library" /></span>{isCapture ? "Captures Library" : "IRs Library"}{result && <b className="directory-count">{libraryRowCount(result)}</b>}</button>
      {folderRows.map(({ folder, depth }) => <button type="button" key={folder.key} style={{ "--qc-directory-depth": depth } as CSSProperties} className={`is-child${folder.key === folderKey ? " is-active" : ""}`} onClick={() => onSelectFolder(folder.key)}><span><QcDirectoryIcon kind="folder-outline" />{isCapture && isDownloadFolder(folder) && <DirectoryDownloadBadge />}</span>{folder.name}{!folder.isFactory && <b className="directory-folder-more" aria-hidden="true"><QcUiIcon kind="more" /></b>}</button>)}
    </nav>
    <div className="directory-library-pane">
      <div ref={listRef} className="directory-presets directory-library-list qc-scroll-rail" role="listbox" aria-label={isCapture ? "Neural Captures" : "Impulse Responses"}>
        {searchField}
        {status ?? <EntryList entries={entries} menu={isCapture} empty={query.trim() ? "NO MATCHING ITEMS" : isCapture ? "NO NEURAL CAPTURES" : "NO IMPULSE RESPONSES"} />}
      </div>
      {isCapture && entries.length > 0 && <nav className="directory-alpha-index" aria-label="Jump to letter">
        {DIRECTORY_ALPHA_INDEX.map((slot, index) => <button type="button" key={index} tabIndex={-1} data-dot={slot.label !== slot.target ? "" : undefined} aria-label={`Jump to ${slot.target}`} onClick={() => jumpTo(slot.target)}>{slot.label}</button>)}
      </nav>}
    </div>
  </div>;
}

/** The Favorites / Recent switch the unit draws in the Directory header, beside the category chip. */
export function DirectoryFavoritesTabs({ tab, onTab }: { tab: "favorites" | "recents"; onTab: (tab: "favorites" | "recents") => void }) {
  return <div className="directory-library-tabs" role="tablist" aria-label="Favorites or Recent">
    <button type="button" role="tab" aria-selected={tab === "favorites"} aria-label="Favorites" className={tab === "favorites" ? "is-active" : undefined} onClick={() => onTab("favorites")}><QcLibraryIcon kind="heart" /></button>
    <button type="button" role="tab" aria-selected={tab === "recents"} aria-label="Recent" className={tab === "recents" ? "is-active" : undefined} onClick={() => onTab("recents")}><QcLibraryIcon kind="clock" /></button>
  </div>;
}

/**
 * Favorites and Recent. The device reply for both reads is a list of presets
 * (the broker rejects any other item type), so only the Presets sub-category
 * has data; the two others are drawn, as the unit draws them, and say why they
 * are empty.
 */
function FavoritesBrowser({ library, tab, subcategory, onSubcategory, query, searchField, dates, onLocate }: { library: DirectoryLibraryState; tab: "favorites" | "recents"; subcategory: "presets" | "captures" | "irs"; onSubcategory: (value: "presets" | "captures" | "irs") => void; query: string; searchField?: ReactNode; dates?: ReadonlyMap<string, string | number>; onLocate?: (entry: LibraryEntry) => void }) {
  const kind: DirectoryLibraryKind = tab;
  const entries = filterLibraryEntries(library[kind]?.entries ?? [], query);
  const status = subcategory === "presets" ? ReadStatus({ kind, library, label: tab === "favorites" ? "FAVORITES" : "RECENTS" }) : null;
  return <div className="coros-directory-body is-library">
    <nav className="directory-folders qc-scroll-rail" aria-label="Favorites and Recent categories">
      <button type="button" className={subcategory === "presets" ? "is-active" : undefined} onClick={() => onSubcategory("presets")}><span><QcDirectoryIcon kind="grid" /></span>Presets</button>
      <button type="button" className={subcategory === "captures" ? "is-active" : undefined} onClick={() => onSubcategory("captures")}><span><QcLibraryIcon kind="capture-header" /></span>Neural Captures</button>
      <button type="button" className={subcategory === "irs" ? "is-active" : undefined} onClick={() => onSubcategory("irs")}><span><QcLibraryIcon kind="impulse-response" /></span>Impulse Responses</button>
    </nav>
    <div className="directory-presets directory-library-list qc-scroll-rail" role="listbox" aria-label={tab === "favorites" ? "Favorites" : "Recent"}>
      {searchField}
      {subcategory === "presets"
        ? status ?? <EntryList entries={entries} detail={tab === "recents" ? "date" : "none"} dates={dates ?? new Map()} onLocate={onLocate} unfavorite={tab === "favorites"} empty={query.trim() ? "NO MATCHING PRESETS" : tab === "favorites" ? "NO FAVORITE PRESETS" : "NO RECENT PRESETS"} />
        : <div className="directory-loading">{`${subcategory === "captures" ? "Neural Capture" : "Impulse Response"} ${tab === "favorites" ? "favorites" : "recents"} are not read by QC Remote yet; only preset ${tab === "favorites" ? "favorites" : "recents"} are.`}</div>}
    </div>
  </div>;
}

/** Plugin Presets: the unit's catalog of licensed plugin preset folders. */
function PluginPresetsBrowser() {
  return <div className="coros-directory-body is-library">
    <nav className="directory-folders qc-scroll-rail" aria-label="Plugin preset folders" />
    <div className="directory-presets directory-library-list qc-scroll-rail">
      <div className="directory-loading">The Quad Cortex's plugin preset catalog is not published through the device API. Plugin presets you have favorited or recently used appear under Favorites and Recent.</div>
    </div>
  </div>;
}

export interface DirectoryLibraryBodyProps {
  category: Exclude<DirectoryCategory, "presets">;
  library: DirectoryLibraryState;
  query: string;
  folderKey?: string;
  onSelectFolder: (key: string | undefined) => void;
  favoritesTab: "favorites" | "recents";
  favoritesSubcategory: "presets" | "captures" | "irs";
  onFavoritesSubcategory: (value: "presets" | "captures" | "irs") => void;
  /** The Directory's shared search field, drawn at the top of whichever list is showing. */
  searchField?: ReactNode;
  /** Whether the unit is signed in to a cloud account; unset is signed out, as in the header. */
  cloudSignedIn?: boolean;
  /** Preset dates by product key, from the preset lists already read: the Recent and Favorites reads carry no date of their own. */
  dates?: ReadonlyMap<string, string | number>;
  /** The binoculars on a Recent row: show that preset in the Presets category. */
  onLocate?: (entry: LibraryEntry) => void;
}

export function DirectoryLibraryBody({ category, library, query, folderKey, onSelectFolder, favoritesTab, favoritesSubcategory, onFavoritesSubcategory, searchField, cloudSignedIn, dates, onLocate }: DirectoryLibraryBodyProps) {
  if (category === "captures" || category === "irs") return <LibraryBrowser kind={category} library={library} folderKey={folderKey} onSelectFolder={onSelectFolder} query={query} searchField={searchField} cloudSignedIn={cloudSignedIn} />;
  if (category === "favorites") return <FavoritesBrowser library={library} tab={favoritesTab} subcategory={favoritesSubcategory} onSubcategory={onFavoritesSubcategory} query={query} searchField={searchField} dates={dates} onLocate={onLocate} />;
  return <PluginPresetsBrowser />;
}
