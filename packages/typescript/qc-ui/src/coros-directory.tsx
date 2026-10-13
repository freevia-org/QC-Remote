import { useEffect, useMemo, useRef, useState } from "react";
import type { LibraryEntry, PresetEntry, PresetSnapshot } from "@qc-remote/client";
import { QcAccountSignedOutGlyph, QcDirectoryIcon, QcEditorIcon, QcUiIcon } from "./theme-icons";
import { DirectoryCategoryGlyph, DirectoryCategoryMenu, DirectoryFavoritesTabs, DirectoryLibraryBody, DirectoryProviderRow } from "./directory-library";
import { DIRECTORY_CATEGORIES, favoritesShowsTools, isCloudSignedOut, libraryKindsForCategory, orderUserFolders, type DirectoryCategory, type DirectoryLibraryKind, type DirectoryLibraryState, type PresetDirectoryState } from "./directory-library-model";
import { DIRECTORY_PRESET_CONTEXT_MENU } from "./coros-ui";

const DIRECTORY_MULTISELECT_GLYPH = "sort" as const;
const DIRECTORY_SORT_ORDER_GLYPH = "arrange" as const;
function presetDateSortValue(entry: PresetEntry): number {
  if (entry.dateMsSinceEpoch !== undefined) return entry.dateMsSinceEpoch;
  const parsed = Date.parse(entry.date ?? "");
  return Number.isFinite(parsed) ? parsed : 0;
}

export function CorOsDirectory({ snapshot, directory }: { snapshot: PresetSnapshot; directory: PresetDirectoryState }) {
  const context = JSON.stringify([directory.open, directory.list?.setlistKey, snapshot.stateVersion?.session]);
  const owner = useRef({ context, session: directory.interactionSessionKey, generation: 0 });
  if (owner.current.context !== context || owner.current.session !== directory.interactionSessionKey) {
    owner.current = { context, session: directory.interactionSessionKey, generation: owner.current.generation + 1 };
  }
  const generation = owner.current.generation;
  const currentDirectory = useRef(directory);
  currentDirectory.current = directory;
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  const accepts = () => mounted.current && owner.current.generation === generation
    && currentDirectory.current.open && !currentDirectory.current.disabled && !currentDirectory.current.loading;
  const scopedDirectory: PresetDirectoryState = {
    ...directory,
    onRecall: (entry) => { if (accepts()) directory.onRecall(entry); },
    onSelectSetlist: (key) => { if (accepts()) directory.onSelectSetlist(key); },
    onNewSetlist: () => { if (accepts()) directory.onNewSetlist(); },
    onPresetAction: (action, entry) => { if (accepts()) directory.onPresetAction(action, entry); },
    onPresetBatchAction: (action, entries) => { if (accepts()) directory.onPresetBatchAction(action, entries); },
    onArrangePreset: (source, destination) => { if (accepts()) directory.onArrangePreset(source, destination); }
  };
  return <CorOsDirectorySession key={generation} snapshot={snapshot} directory={scopedDirectory} />;
}

function CorOsDirectorySession({ snapshot, directory }: { snapshot: PresetSnapshot; directory: PresetDirectoryState }) {
  const referenceFallback = snapshot.setlistKey === "/user/A/" && !directory.list;
  // The setlist names the `preset-directory` frame actually holds.
  const referenceFolders = ["My Presets", "Set Live", "Set Rec", "Set AcousticLive"];
  const referencePresets: PresetEntry[] = Array.from({ length: 8 }, (_, index) => ({
    position: 248 + index,
    location: `32${String.fromCharCode(65 + index)}`,
    name: index === 7 ? "Pyquadcortex scratch" : "Unsaved",
    instrument: 0
  }));
  const viewingActiveSetlist = referenceFallback || directory.list?.setlistKey === snapshot.setlistKey;
  const currentBank = viewingActiveSetlist ? Math.floor(snapshot.presetPosition / 8) + 1 : 1;
  const [selectedBank, setSelectedBank] = useState(currentBank);
  const [presetMenuPosition, setPresetMenuPosition] = useState<number>();
  const [uploadMode, setUploadMode] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const onSearchOpenChange = directory.onSearchOpenChange;
  useEffect(() => { onSearchOpenChange?.(searchOpen); }, [searchOpen, onSearchOpenChange]);
  useEffect(() => () => onSearchOpenChange?.(false), [onSearchOpenChange]);
  const [sortOpen, setSortOpen] = useState(false);
  const [sortBy, setSortBy] = useState<"Banks" | "Name" | "Date Added" | "Author" | "Preferred Instrument">("Banks");
  const [arrangeMode, setArrangeMode] = useState(false);
  const [arrangeSource, setArrangeSource] = useState<PresetEntry>();
  const [multiSelectMode, setMultiSelectMode] = useState(false);
  const [selectedPresetPositions, setSelectedPresetPositions] = useState<Set<number>>(() => new Set());
  const [category, setCategory] = useState<DirectoryCategory>("presets");
  const [categoryMenuOpen, setCategoryMenuOpen] = useState(false);
  const [libraryFolderKey, setLibraryFolderKey] = useState<string>();
  const [favoritesTab, setFavoritesTab] = useState<"favorites" | "recents">("favorites");
  const [favoritesSubcategory, setFavoritesSubcategory] = useState<"presets" | "captures" | "irs">("presets");
  const library = directory.library;
  const onLoadLibrary = directory.onLoadLibrary;
  useEffect(() => {
    if (!directory.open || !onLoadLibrary) return;
    for (const kind of libraryKindsForCategory(category, favoritesTab)) {
      if (!library?.[kind] && !library?.loading.includes(kind) && !library?.errors[kind]) onLoadLibrary(kind);
    }
  }, [category, directory.open, favoritesTab, library, onLoadLibrary]);
  useEffect(() => {
    if (directory.open) setSelectedBank(currentBank);
  }, [currentBank, directory.list?.setlistKey, directory.open]);
  const highestBank = referenceFallback ? 32 : Math.max(1, ...(directory.list?.presets.map((entry) => Math.floor(entry.position / 8) + 1) ?? []));
  const banks = Array.from({ length: referenceFallback ? 14 : highestBank }, (_, index) => index + (referenceFallback ? 19 : 1));
  const bankPresets = referenceFallback ? referencePresets : directory.list?.presets.filter((entry) => Math.floor(entry.position / 8) + 1 === selectedBank) ?? [];
  const matchingPresets = searchOpen
    ? (directory.list?.presets ?? []).filter((entry) => `${entry.location} ${entry.name}`.toLocaleLowerCase().includes(searchQuery.trim().toLocaleLowerCase()))
    : bankPresets;
  const presets = sortBy === "Name"
    ? [...matchingPresets].sort((left, right) => left.name.localeCompare(right.name, undefined, { sensitivity: "base" }) || left.position - right.position)
    : sortBy === "Date Added"
      ? [...matchingPresets].sort((left, right) => presetDateSortValue(right) - presetDateSortValue(left) || left.position - right.position)
    : sortBy === "Author"
      ? [...matchingPresets].sort((left, right) => (left.author ?? "").localeCompare(right.author ?? "", undefined, { sensitivity: "base" }) || left.name.localeCompare(right.name, undefined, { sensitivity: "base" }) || left.position - right.position)
    : sortBy === "Preferred Instrument"
      ? [...matchingPresets].sort((left, right) => left.instrument - right.instrument || left.position - right.position)
      : matchingPresets;
  // Recent and Favorites rows carry no date on the wire; the unit's date line is
  // the preset's own, so remember each preset date as its list is read.
  const presetDatesRef = useRef(new Map<string, string | number>());
  for (const entry of directory.list?.presets ?? []) {
    if (!entry.productKey) continue;
    const date = entry.dateMsSinceEpoch ?? (entry.date || undefined);
    if (date !== undefined) presetDatesRef.current.set(entry.productKey, date);
  }
  for (const [productKey, date] of directory.setlistDates ?? []) if (!presetDatesRef.current.has(productKey)) presetDatesRef.current.set(productKey, date);
  const presetDates = presetDatesRef.current;
  const onLoadSetlistDates = directory.onLoadSetlistDates;
  const recentRows = library?.recents?.entries;
  useEffect(() => {
    if (!directory.open || !onLoadSetlistDates || category !== "favorites" || favoritesTab !== "recents" || favoritesSubcategory !== "presets") return;
    const undated = (recentRows ?? []).filter((entry) => entry.productKey && entry.folderKey && !presetDatesRef.current.has(entry.productKey));
    if (undated.length) onLoadSetlistDates(undated.map((entry) => entry.folderKey as string));
  }, [category, directory.open, directory.setlistDates, directory.list, favoritesSubcategory, favoritesTab, onLoadSetlistDates, recentRows]);
  const locatePreset = (entry: LibraryEntry) => {
    if (!entry.folderKey) return;
    setCategory("presets");
    setSearchOpen(false);
    setSearchQuery("");
    setLibraryFolderKey(undefined);
    directory.onSelectSetlist(entry.folderKey);
  };
  const factoryFolder = directory.list?.folders.find((folder) => folder.isFactory);
  const userFolders = orderUserFolders(directory.list?.folders.filter((folder) => !folder.isFactory) ?? []);
  const runPresetAction = (action: "upload" | "edit" | "copy" | "cut" | "paste" | "delete", entry: PresetEntry) => {
    setPresetMenuPosition(undefined);
    directory.onPresetAction(action, entry);
  };
  const menuEntry = presetMenuPosition === undefined ? undefined : presets.find((entry) => entry.position === presetMenuPosition);
  const selectablePresets = presets.filter((entry) => entry.occupied !== false && entry.name !== "Unsaved");
  const selectedPresets = selectablePresets.filter((entry) => selectedPresetPositions.has(entry.position));
  const leaveMultiSelect = () => {
    setMultiSelectMode(false);
    setSelectedPresetPositions(new Set());
  };
  const runBatchAction = (action: "copy" | "delete") => {
    if (!selectedPresets.length) return;
    directory.onPresetBatchAction(action, selectedPresets);
    leaveMultiSelect();
  };

  return <section className="coros-directory" aria-label="Preset Directory">
    {multiSelectMode ? <header className="coros-directory-header directory-multiselect-header">
      <button type="button" className="directory-select-all" aria-label="Select all presets in this view" aria-pressed={selectedPresets.length > 0 && selectedPresets.length === selectablePresets.length} onClick={() => setSelectedPresetPositions(selectedPresets.length === selectablePresets.length ? new Set() : new Set(selectablePresets.map((entry) => entry.position)))}>
        <span>{selectedPresets.length > 0 && selectedPresets.length === selectablePresets.length ? <QcUiIcon kind="check" /> : null}</span>
      </button>
      <strong>Multi Select</strong>
      <span className="directory-header-spacer" />
      <button type="button" className="directory-batch-action" disabled={!selectedPresets.length || directory.disabled} onClick={() => runBatchAction("copy")}><b aria-hidden="true"><QcEditorIcon kind="copy" /></b><span>Copy</span>{selectedPresets.length > 0 && <i>{selectedPresets.length}</i>}</button>
      <button type="button" className="directory-batch-action" disabled={selectedPresets.length !== 1 || directory.disabled} onClick={() => { const source = selectedPresets[0]; leaveMultiSelect(); setArrangeSource(source); setArrangeMode(true); }}><QcDirectoryIcon kind="arrange" /><span>Arrange</span></button>
      <button type="button" className="directory-batch-action is-danger" disabled={!selectedPresets.length || directory.disabled || !!directory.list?.folders.find((folder) => folder.key === directory.list?.setlistKey)?.isFactory} onClick={() => runBatchAction("delete")}><QcDirectoryIcon kind="trash" /><span>Delete</span></button>
      <button type="button" className="directory-close" aria-label="Finish multi select" onClick={leaveMultiSelect}><QcDirectoryIcon kind="done" /></button>
    </header> : <header className="coros-directory-header">
      <button type="button" className="directory-category" aria-label="Directory category" aria-haspopup="menu" aria-expanded={categoryMenuOpen} onClick={() => { setPresetMenuPosition(undefined); setSortOpen(false); setCategoryMenuOpen((open) => !open); }}><span className="directory-grid-icon"><DirectoryCategoryGlyph category={category} /></span><strong>{DIRECTORY_CATEGORIES.find((item) => item.id === category)?.label}</strong><span className="directory-chevron"><QcUiIcon kind={categoryMenuOpen ? "up" : "down"} /></span></button>
      {category === "favorites" && <DirectoryFavoritesTabs tab={favoritesTab} onTab={setFavoritesTab} />}
      {category === "presets" && directory.cloudSignedIn && <button className={`directory-cloud${uploadMode ? " is-active" : ""}`} aria-label="Upload to Cloud" aria-pressed={uploadMode} onClick={() => { setPresetMenuPosition(undefined); setUploadMode((active) => !active); }}><QcDirectoryIcon kind="cloud" /></button>}
      {(category === "presets" || category === "captures" || category === "irs") && !(category === "presets" && directory.cloudSignedIn) && <span className="directory-cloud directory-account" role="img" aria-label="Cloud account: signed out" title="Not signed in to a Neural DSP cloud account on the Quad Cortex."><QcAccountSignedOutGlyph /></span>}
      <span className="directory-header-spacer" />
      <div className={category === "presets" ? "directory-tools" : category === "favorites" ? "directory-library-tools is-compact" : "directory-library-tools"} aria-label="Directory tools">
        {category !== "presets" && category !== "plugins" && (category !== "favorites" || favoritesShowsTools(favoritesTab)) && <>
          <button type="button" aria-label="Multi select" disabled title="Multi select is not available for this list."><QcDirectoryIcon kind={DIRECTORY_MULTISELECT_GLYPH} /></button>
          {category !== "favorites" && <button type="button" aria-label="Filter" disabled title="Filtering is not available through the device API."><QcDirectoryIcon kind="filter" /></button>}
          <button type="button" aria-label="Sort" disabled title="Sorting is not available for this list."><QcDirectoryIcon kind={DIRECTORY_SORT_ORDER_GLYPH} /></button>
        </>}
        {category === "presets" && <>
          <button type="button" aria-label="Multi select presets" disabled={directory.disabled || !directory.list} onClick={() => { setPresetMenuPosition(undefined); setArrangeMode(false); setArrangeSource(undefined); setMultiSelectMode(true); setSelectedPresetPositions(new Set()); }}><QcDirectoryIcon kind={DIRECTORY_MULTISELECT_GLYPH} /></button>
          <button type="button" aria-label="Sort presets" aria-haspopup="menu" aria-expanded={sortOpen} aria-pressed={sortOpen} className={sortOpen ? "is-active" : ""} onClick={() => { setPresetMenuPosition(undefined); setSortOpen((open) => !open); }}><QcDirectoryIcon kind={DIRECTORY_SORT_ORDER_GLYPH} /></button>
        </>}
        {(category !== "favorites" || favoritesShowsTools(favoritesTab)) && <>
          <button type="button" aria-label={category === "presets" ? "Search presets" : "Search Directory"} aria-pressed={searchOpen} className={searchOpen ? "is-active" : ""} onClick={() => { setSortOpen(false); setSearchOpen((open) => { if (open) setSearchQuery(""); return !open; }); }}><QcDirectoryIcon kind="search" /></button>
          <span className="directory-tool-divider" />
        </>}
        <button className="directory-close" aria-label="Return to Grid" onClick={directory.onClose}><QcDirectoryIcon kind="done" /></button>
      </div>
    </header>}
    {category !== "presets" ? <DirectoryLibraryBody category={category} library={library ?? { loading: [], errors: {} }} query={searchOpen ? searchQuery : ""} folderKey={libraryFolderKey} onSelectFolder={setLibraryFolderKey} favoritesTab={favoritesTab} favoritesSubcategory={favoritesSubcategory} onFavoritesSubcategory={setFavoritesSubcategory} cloudSignedIn={directory.cloudSignedIn} dates={presetDates} onLocate={locatePreset} searchField={searchOpen ? <label className="directory-search-field"><QcDirectoryIcon kind="search" /><input type="search" aria-label="Search Directory" placeholder="Search Directory" value={searchQuery} autoFocus onChange={(event) => setSearchQuery(event.target.value)} /><button type="button" aria-label="Close directory search" onClick={() => { setSearchQuery(""); setSearchOpen(false); }}>×</button></label> : undefined} /> : <div className="coros-directory-body">
      <nav className="directory-folders qc-scroll-rail" aria-label="Preset folders">
        <DirectoryProviderRow kind="download" label="Downloads" signedOut={isCloudSignedOut(directory.cloudSignedIn)} />
        <DirectoryProviderRow kind="cloud" label="Cloud Presets" signedOut={isCloudSignedOut(directory.cloudSignedIn)} />
        <button type="button" disabled={!factoryFolder} className={factoryFolder && factoryFolder.key === directory.list?.setlistKey ? "is-active" : ""} onClick={() => factoryFolder && directory.onSelectSetlist(factoryFolder.key)}><span><QcDirectoryIcon kind="folder" number={0} /></span>Factory Presets</button>
        {userFolders.map((folder, index) => <button key={folder.key} className={folder.key === directory.list?.setlistKey ? "is-active" : ""} onClick={() => directory.onSelectSetlist(folder.key)}><span><QcDirectoryIcon kind="folder" number={index + 1} /></span>{folder.name}<b className="directory-folder-more"><QcUiIcon kind="more" /></b></button>)}
        {referenceFallback && referenceFolders.map((name, index) => <button type="button" disabled key={name} className={index === 0 ? "is-active" : ""}><span><QcDirectoryIcon kind="folder" number={index + 1} /></span>{name}<b className="directory-folder-more"><QcUiIcon kind="more" /></b></button>)}
        {!referenceFallback && !userFolders.length && <button type="button" disabled className="is-active"><span><QcDirectoryIcon kind="folder" number={1} /></span>{directory.list?.setlistName ?? snapshot.setlistName}<b className="directory-folder-more"><QcUiIcon kind="more" /></b></button>}
        <button className="directory-new-setlist" disabled={directory.disabled} title="Create a new user setlist" onClick={directory.onNewSetlist}><span><QcDirectoryIcon kind="new-folder" /></span>New Setlist</button>
      </nav>
      <nav className="directory-banks qc-scroll-rail" aria-label="Preset banks">
        {banks.length ? banks.map((bank) => <button key={bank} className={bank === selectedBank ? "is-active" : ""} onClick={() => setSelectedBank(bank)}>{bank}</button>) : <span className="directory-loading">{directory.loading ? "READING…" : "NO BANKS"}</span>}
      </nav>
      <div className="directory-presets qc-scroll-rail" role="listbox" aria-label={`Bank ${selectedBank} presets`}>
        {searchOpen && <label className="directory-search-field"><QcDirectoryIcon kind="search" /><input type="search" aria-label="Search Directory" placeholder="Search Directory" value={searchQuery} autoFocus onChange={(event) => setSearchQuery(event.target.value)} /><button type="button" aria-label="Close directory search" onClick={() => { setSearchQuery(""); setSearchOpen(false); }}>×</button></label>}
        {directory.loading && !directory.list ? <div className="directory-loading">READING PRESETS FROM QUAD CORTEX…</div> : presets.map((entry) => <div key={entry.position} role="option" aria-selected={multiSelectMode ? selectedPresetPositions.has(entry.position) : viewingActiveSetlist && entry.position === snapshot.presetPosition} className={`directory-preset-row${entry.occupied === false || entry.name === "Unsaved" ? " is-empty" : ""}${viewingActiveSetlist && entry.position === snapshot.presetPosition ? " is-current" : ""}${uploadMode ? " is-upload-mode" : ""}${multiSelectMode ? " is-multiselect" : ""}${selectedPresetPositions.has(entry.position) ? " is-selected" : ""}`}>
          <button className={`preset-recall${arrangeSource?.position === entry.position ? " is-arrange-source" : ""}`} disabled={directory.disabled} onClick={() => {
            if (multiSelectMode) {
              if (entry.occupied === false || entry.name === "Unsaved") return;
              setSelectedPresetPositions((current) => {
                const next = new Set(current);
                if (next.has(entry.position)) next.delete(entry.position); else next.add(entry.position);
                return next;
              });
              return;
            }
            if (!arrangeMode) { directory.onRecall(entry); return; }
            if (!arrangeSource) {
              if (entry.occupied === false || entry.name === "Unsaved") return;
              setArrangeSource(entry);
              return;
            }
            if (arrangeSource.position === entry.position) { setArrangeSource(undefined); return; }
            const source = arrangeSource;
            setArrangeSource(undefined);
            directory.onArrangePreset(source, entry);
          }} onContextMenu={(event) => {
            if (!arrangeSource) return;
            event.preventDefault();
            setArrangeSource(undefined);
          }}>{multiSelectMode && <i className="directory-selection-box">{selectedPresetPositions.has(entry.position) ? <QcUiIcon kind="check" /> : null}</i>}<span className="preset-label"><strong>{entry.location}</strong>{" "}<span>{entry.name}</span></span></button>
          {multiSelectMode ? null : arrangeMode ? <span className="preset-arrange-hint">{arrangeSource?.position === entry.position ? "SOURCE" : arrangeSource ? "DESTINATION" : ""}</span> : uploadMode ? <button className="preset-upload-action" onClick={() => runPresetAction("upload", entry)}>UPLOAD</button> : <button className="preset-row-more" aria-label={`Open menu for ${entry.location} ${entry.name}`} aria-expanded={presetMenuPosition === entry.position} onClick={() => setPresetMenuPosition((position) => position === entry.position ? undefined : entry.position)}><QcUiIcon kind="more" /></button>}
        </div>)}
        {searchOpen && !directory.loading && !presets.length && <div className="directory-loading">{searchQuery.trim() ? "NO MATCHING PRESETS" : "NO PRESETS"}</div>}
      </div>
    </div>}
    {menuEntry && <><button type="button" className="directory-context-scrim" aria-label="Close preset menu" onClick={() => setPresetMenuPosition(undefined)} /><aside className="preset-context-menu" role="menu" aria-label={`${menuEntry.name} menu`}>
      {DIRECTORY_PRESET_CONTEXT_MENU.map((item) => {
        const factory = directory.list?.folders.find((folder) => folder.key === directory.list?.setlistKey)?.isFactory ?? false;
        const empty = menuEntry.occupied === false || menuEntry.name === "Unsaved";
        const disabled = directory.disabled || arrangeMode || ("requiresPreset" in item && item.requiresPreset && empty) || (item.action === "delete" && factory) || ((item.action === "paste" || item.action === "cut") && factory) || (item.action === "paste" && !directory.list);
        const title = item.action === "delete" && factory ? "Factory presets cannot be deleted." : (item.action === "paste" || item.action === "cut") && factory ? "Factory presets are read-only destinations and cannot be cut." : undefined;
        return <button key={item.action} type="button" role="menuitem" className={"danger" in item && item.danger ? "context-danger" : undefined} disabled={disabled} title={title} onClick={() => runPresetAction(item.action, menuEntry)}>{item.label}</button>;
      })}
    </aside></>}
    {categoryMenuOpen && <DirectoryCategoryMenu category={category} onClose={() => setCategoryMenuOpen(false)} onSelect={(next) => { setCategoryMenuOpen(false); if (next !== category) { setCategory(next); setLibraryFolderKey(undefined); setSearchOpen(false); setSearchQuery(""); setArrangeMode(false); setArrangeSource(undefined); setUploadMode(false); setPresetMenuPosition(undefined); } }} />}
    {sortOpen && <><button type="button" className="directory-context-scrim" aria-label="Close preset sorting" onClick={() => setSortOpen(false)} /><aside className="directory-tool-menu" aria-label="Sort By"><strong>Sort By</strong>{(["Banks", "Name", "Date Added", "Author", "Preferred Instrument"] as const).map((label) => {
      const selected = sortBy === label;
      return <button type="button" key={label} className={selected ? "is-active" : ""} onClick={() => { setSortBy(label); setSortOpen(false); }}>{label}</button>;
    })}</aside></>}
  </section>;
}
