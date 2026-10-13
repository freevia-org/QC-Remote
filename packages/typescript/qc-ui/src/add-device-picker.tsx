import { useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent } from "react";
import type { LibraryEntry, LibraryFolder, ModelEntry } from "@qc-remote/client";
import { addDevicePickerSide, COROS_DEVICE_CATEGORIES, deviceBrowserInstruments, deviceBrowserRows, type DeviceBrowserInstrument } from "./add-device-catalog";
import { QcDeviceCategoryGlyph } from "./device-category-glyph";
import { QcDirectoryIcon, QcLibraryIcon, QcUiIcon } from "./theme-icons";
import { useHoldGesture } from "./use-hold-gesture";
import "./official-device-browser.css";
import "./add-device-picker.css";

/** The live Add device browser's state, owned by the Grid workflow. */
export interface CorOsAddDevicePickerState {
  /** The empty Grid cell the chosen model is placed into. */
  row: number;
  column: number;
  /** Every installed model the QC reported; the browser does its own grouping. */
  models: ModelEntry[];
  captures?: LibraryEntry[];
  captureFolders?: LibraryFolder[];
  captureError?: string;
  loading: boolean;
  pending: boolean;
  query: string;
  onQueryChange(value: string): void;
  /** Undefined lists every category; choosing the active one again clears it. */
  activeCategory?: string;
  onCategoryChange(category: string): void;
  selectedModelId?: number;
  pinnedModelIds: number[];
  onSelect(modelId: number): void;
  onTogglePinned(modelId: number): void;
  onSelectCapture?(capture: LibraryEntry): void;
  onClose(): void;
}

// The Grid's drawing box: the cell centres below arrive in its 800x480 units.
const GRID_WIDTH = 800;
const GRID_HEIGHT = 480;
// device-browser-root and -models light the target slot as a 70px plate centred
// on the cell - x 322 at column 4 (centre 357), y 206 on row 2 (centre 241).
const TARGET_PLATE = 70;

/**
 * The unit's own device browser, live: the category rail and model list of the
 * official browser frames, opened over one half of the live Grid with the other
 * half washed and the target slot lit, as device-browser-root shows it.
 */
export function CorOsAddDevicePicker({ picker, cellCenter }: { picker: CorOsAddDevicePickerState; cellCenter: { x: number; y: number } }) {
  const { row, column, models, captures = [], captureFolders = [], captureError, loading, pending, query, onQueryChange, activeCategory, onCategoryChange, selectedModelId, pinnedModelIds, onSelect, onTogglePinned, onSelectCapture, onClose } = picker;
  const [instrument, setInstrument] = useState<DeviceBrowserInstrument>("guitar");
  const [captureFolder, setCaptureFolder] = useState<string>();
  const [captureFilter, setCaptureFilter] = useState<"all" | "factory" | "user" | "v1" | "v2">("all");
  const [captureSort, setCaptureSort] = useState<"ascending" | "descending">("ascending");
  const [captureSearchOpen, setCaptureSearchOpen] = useState(false);
  const [captureFilterOpen, setCaptureFilterOpen] = useState(false);
  const consumedModelTap = useRef<number | undefined>(undefined);
  const consumedTapClearTimer = useRef<number | undefined>(undefined);
  useEffect(() => () => {
    if (consumedTapClearTimer.current !== undefined) window.clearTimeout(consumedTapClearTimer.current);
  }, []);
  // Holding a model pins it; a press that travels more than 8 px is a scroll.
  // The release click after a pin is swallowed for that model, for up to 1 s.
  const modelHold = useHoldGesture<number>({
    moveTolerancePx: 8,
    onHold: (modelId) => {
      consumedModelTap.current = modelId;
      if (consumedTapClearTimer.current !== undefined) window.clearTimeout(consumedTapClearTimer.current);
      consumedTapClearTimer.current = window.setTimeout(() => {
        if (consumedModelTap.current === modelId) consumedModelTap.current = undefined;
        consumedTapClearTimer.current = undefined;
      }, 1_000);
      onTogglePinned(modelId);
    }
  });
  useEffect(() => setInstrument("guitar"), [activeCategory]);
  const instruments = useMemo(() => deviceBrowserInstruments(models, activeCategory), [activeCategory, models]);
  const captureCategory = activeCategory === "Neural Capture";
  const captureRoot = useMemo(() => captureFolders.find((folder) => folder.isLibrary && !folder.parentKey)
    ?? captureFolders.find((folder) => !folder.parentKey), [captureFolders]);
  const currentCaptureFolder = captureFolder ?? captureRoot?.key;
  const captureChildren = useMemo(() => captureFolders
    .filter((folder) => folder.key !== captureRoot?.key && folder.parentKey === currentCaptureFolder)
    .sort((left, right) => left.name.localeCompare(right.name)), [captureFolders, captureRoot?.key, currentCaptureFolder]);
  const rows = useMemo(() => [...deviceBrowserRows(models, { category: activeCategory, instrument: instruments.length ? instrument : undefined, query })]
    .sort((left, right) => Number(pinnedModelIds.includes(right.id)) - Number(pinnedModelIds.includes(left.id))), [activeCategory, instrument, instruments.length, models, pinnedModelIds, query]);
  const matchingCaptures = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return captures.filter((entry) => {
      if (captureFolders.length && (entry.folderKey || entry.folderName || "captures") !== currentCaptureFolder) return false;
      if (!captureFolders.length && captureFolder && (entry.folderKey || entry.folderName || "captures") !== captureFolder) return false;
      if (captureFilter === "factory" && !entry.isFactory) return false;
      if (captureFilter === "user" && (entry.isFactory || entry.isPlugin)) return false;
      if (captureFilter === "v1" && String(entry.version) !== "1") return false;
      if (captureFilter === "v2" && String(entry.version) !== "2") return false;
      return !needle || `${entry.folderName ?? ""} ${entry.name} ${entry.author ?? ""} ${entry.description ?? ""}`.toLocaleLowerCase().includes(needle);
    });
  }, [captureFilter, captureFolder, captureFolders.length, captures, currentCaptureFolder, query]);
  const captureGroups = useMemo(() => {
    const groups = new Map<string, { key: string; name: string; entries: LibraryEntry[] }>();
    for (const capture of matchingCaptures) {
      const key = capture.folderKey || capture.folderName || "captures";
      const group = groups.get(key) ?? { key, name: capture.folderName?.trim() || "Captures", entries: [] };
      group.entries.push(capture);
      groups.set(key, group);
    }
    return [...groups.values()].sort((left, right) => left.name.localeCompare(right.name)).map((group) => ({
      ...group,
      entries: group.entries.sort((left, right) => left.name.localeCompare(right.name))
    }));
  }, [matchingCaptures]);
  const sortedCaptures = useMemo(() => [...matchingCaptures].sort((left, right) => {
    const order = left.name.localeCompare(right.name, undefined, { sensitivity: "base", numeric: true });
    return captureSort === "ascending" ? order : -order;
  }), [captureSort, matchingCaptures]);
  const groupedCaptureCount = captureGroups.reduce((count, group) => count + group.entries.length, 0);
  const side = addDevicePickerSide(column);
  const target: CSSProperties = {
    left: `${((cellCenter.x - TARGET_PLATE / 2) / GRID_WIDTH) * 100}%`,
    top: `${((cellCenter.y - TARGET_PLATE / 2) / GRID_HEIGHT) * 100}%`,
  };
  const closeOnEscape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    onClose();
  };
  const beginModelHold = (event: PointerEvent<HTMLButtonElement>, modelId: number) => {
    if (event.button !== 0 || pending) return;
    modelHold.start(modelId, event);
  };
  const moveModelHold = (event: PointerEvent<HTMLButtonElement>) => modelHold.move(event);
  const finishModelHold = (event: PointerEvent<HTMLButtonElement>) => modelHold.cancel(event);
  if (captureCategory) {
    const visibleCaptures = sortedCaptures;
    const libraryCount = captures.length;
    return <section className="qc-screen capture-library-browser is-live-capture-picker" aria-label={`Add Neural Capture to row ${row + 1}, column ${column + 1}`} onKeyDown={closeOnEscape}>
      <nav className="capture-library-rail qc-scroll-rail" aria-label="Device categories">
        {COROS_DEVICE_CATEGORIES.slice(0, 6).map(([label]) => <button key={label} type="button" className={label === "Neural Capture" ? "is-active" : ""} aria-pressed={label === "Neural Capture"} aria-label={label} onClick={() => onCategoryChange(label)}>
          <i><QcDeviceCategoryGlyph label={label} /></i>
        </button>)}
      </nav>
      <header>
        <span>Add device</span>
        <button type="button" aria-label="Filter captures" aria-pressed={captureFilterOpen} onClick={() => setCaptureFilterOpen((open) => !open)}><QcDirectoryIcon kind="filter" /></button>
        <button type="button" aria-label={`Sort captures ${captureSort === "ascending" ? "descending" : "ascending"}`} aria-pressed={captureSort === "descending"} onClick={() => setCaptureSort((sort) => sort === "ascending" ? "descending" : "ascending")}><QcDirectoryIcon kind="sort" /></button>
        <button type="button" aria-label={captureSearchOpen ? "Hide capture search" : "Search captures"} aria-pressed={captureSearchOpen} onClick={() => setCaptureSearchOpen((open) => !open)}><QcDirectoryIcon kind="search" /></button>
        <button type="button" aria-label="Close device browser" onClick={onClose}><QcUiIcon kind="close" /></button>
      </header>
      <aside className="qc-scroll-rail" aria-label="Capture folders">
        {captureFolder && <button type="button" className="capture-picker-back" onClick={() => {
          const parent = captureFolders.find((folder) => folder.key === captureFolder)?.parentKey;
          setCaptureFolder(parent === captureRoot?.key ? undefined : parent);
        }}><b><QcUiIcon kind="previous" /></b><span>Back</span></button>}
        <button type="button" disabled title="Capture favorites are not published by the local device catalog."><b><QcLibraryIcon kind="heart" /></b><span>Favorites</span></button>
        <button type="button" disabled title="Recent captures are not published by the local device catalog."><b><QcLibraryIcon kind="clock" /></b><span>Recent</span></button>
        <button type="button" disabled title="Downloads require the authenticated Cortex Cloud provider."><b><QcDirectoryIcon kind="download" /></b><span>Downloads</span></button>
        <button type="button" className={!captureFolder ? "is-active" : ""} aria-pressed={!captureFolder} onClick={() => setCaptureFolder(undefined)}><b><QcLibraryIcon kind="capture-library" /></b><span>{captureRoot?.name || "Captures Library"}</span><small>{libraryCount}</small></button>
        <i />
        {captureChildren.map((folder) => <button key={folder.key} type="button" className={captureFolder === folder.key ? "is-active" : ""} aria-pressed={captureFolder === folder.key} onClick={() => setCaptureFolder(folder.key)}>
          <b><QcDirectoryIcon kind="folder-outline" /></b><span>{folder.name || "Captures"}</span><small>{captures.filter((entry) => entry.folderKey === folder.key).length || ""}</small>
        </button>)}
      </aside>
      <main className="qc-scroll-rail">
        {captureFilterOpen && <div className="capture-picker-filters" role="group" aria-label="Filter captures">
          {(["all", "factory", "user", "v1", "v2"] as const).map((filter) => <button type="button" key={filter} className={captureFilter === filter ? "is-active" : ""} aria-pressed={captureFilter === filter} onClick={() => setCaptureFilter(filter)}>{filter === "all" ? "All" : filter === "v1" || filter === "v2" ? filter.toUpperCase() : filter === "user" ? "User" : "Factory"}</button>)}
        </div>}
        {captureSearchOpen && <label className="device-browser-search is-capture-search"><QcDirectoryIcon kind="search" /><input type="search" value={query} placeholder="Search captures" aria-label="Search captures" autoFocus onChange={(event) => onQueryChange(event.target.value)} />{query && <button type="button" aria-label="Clear search" onClick={() => onQueryChange("")}><QcUiIcon kind="close" /></button>}</label>}
        {loading ? <p className="device-browser-status">Reading existing Neural Captures…</p>
          : captureError ? <p className="device-browser-status" role="alert">{captureError}</p>
            : <div className="capture-library-rows">
              {visibleCaptures.map((capture) => {
                const version = Number(capture.version);
                const supported = version === 1 || version === 2;
                return <button key={capture.key} type="button" disabled={pending || !onSelectCapture || !supported} title={!supported ? "This capture does not report a supported version." : capture.description || undefined} onClick={() => onSelectCapture?.(capture)}>
                  <span>{capture.name}{capture.author && <small>{capture.author}</small>}</span>{supported && capture.instrument !== undefined && <b>{capture.instrument}</b>}
                </button>;
              })}
              {!visibleCaptures.length && <p className="device-browser-status">{query.trim() ? "No captures match this search." : "No captures in this folder."}</p>}
            </div>}
      </main>
    </section>;
  }
  return <>
    <div className={`coros-add-device-wash is-${side === "right" ? "left" : "right"}`} aria-hidden="true" />
    <button type="button" className={`coros-add-device-dismiss is-${side === "right" ? "left" : "right"}`} aria-label="Close device browser" onClick={onClose} />
    <div className="coros-add-device-target" style={target} aria-hidden="true"><QcUiIcon kind="add" /></div>
    <section className={`device-browser-official is-live-picker is-${side}${activeCategory === "Plugins" ? " is-plugins" : ""}`} aria-label={`Add device to row ${row + 1}, column ${column + 1}`} onKeyDown={closeOnEscape}>
      <main>
        <nav className="qc-scroll-rail" aria-label="Device categories">
          {COROS_DEVICE_CATEGORIES.map(([label, color]) => (
            <button key={label} type="button" data-category={label} className={label === activeCategory ? "is-active" : ""} aria-pressed={label === activeCategory} aria-label={label} title={label} style={{ "--device-color": color } as CSSProperties} onClick={() => onCategoryChange(label)}>
              <i>
                <QcDeviceCategoryGlyph label={label} />
              </i>
            </button>
          ))}
        </nav>
        <section className="device-browser-list qc-scroll-rail" aria-label={activeCategory ? `${activeCategory} devices` : "All devices"}>
          <header className={captureCategory ? "has-capture-toolbar" : undefined}>
            {captureCategory ? <>
              <b>Add device</b>
              <div className="capture-picker-toolbar" aria-label="Capture chooser actions">
                <button type="button" aria-label="Filter captures" aria-pressed={captureFilterOpen} onClick={() => setCaptureFilterOpen((open) => !open)}><QcDirectoryIcon kind="filter" /></button>
                <button type="button" aria-label={`Sort captures ${captureSort === "ascending" ? "descending" : "ascending"}`} aria-pressed={captureSort === "descending"} onClick={() => setCaptureSort((sort) => sort === "ascending" ? "descending" : "ascending")}><QcDirectoryIcon kind="sort" /></button>
                <button type="button" aria-label={captureSearchOpen ? "Hide capture search" : "Search captures"} aria-pressed={captureSearchOpen} onClick={() => setCaptureSearchOpen((open) => !open)}><QcDirectoryIcon kind="search" /></button>
                <button type="button" aria-label="Close device browser" onClick={onClose}><QcUiIcon kind="close" /></button>
              </div>
            </> : instruments.length ? instruments.map((entry) => (
              <button key={entry} type="button" className={entry === instrument ? "is-active" : ""} aria-pressed={entry === instrument} onClick={() => setInstrument(entry)}>{entry.toUpperCase()}</button>
            )) : <b>Add device</b>}
          </header>
          {captureCategory && captureFilterOpen && <div className="capture-picker-filters" role="group" aria-label="Filter captures">
            {(["all", "factory", "user", "v1", "v2"] as const).map((filter) => <button type="button" key={filter} className={captureFilter === filter ? "is-active" : ""} aria-pressed={captureFilter === filter} onClick={() => setCaptureFilter(filter)}>{filter === "all" ? "All" : filter === "v1" || filter === "v2" ? filter.toUpperCase() : filter === "user" ? "User" : "Factory"}</button>)}
          </div>}
          {!captureCategory && <label className="device-browser-search">
            <QcDirectoryIcon kind="search" />
            <input type="search" value={query} placeholder="Search devices" aria-label="Search devices" autoFocus onChange={(event) => onQueryChange(event.target.value)} />
            {query && <button type="button" aria-label="Clear search" onClick={() => onQueryChange("")}><QcUiIcon kind="close" /></button>}
          </label>}
          {captureCategory && captureSearchOpen && <label className="device-browser-search is-capture-search">
            <QcDirectoryIcon kind="search" />
            <input type="search" value={query} placeholder="Search captures" aria-label="Search captures" autoFocus onChange={(event) => onQueryChange(event.target.value)} />
            {query && <button type="button" aria-label="Clear search" onClick={() => onQueryChange("")}><QcUiIcon kind="close" /></button>}
          </label>}
          {captureCategory
            ? loading
              ? <p className="device-browser-status">Reading existing Neural Captures…</p>
              : captureError
                ? <p className="device-browser-status" role="alert">{captureError}</p>
                : captureGroups.length || captureChildren.length
                  ? <div className="device-browser-capture-groups">
                    {captureFolders.length ? <>
                      {captureFolder && <button type="button" className="capture-picker-back" onClick={() => {
                        const parent = captureFolders.find((folder) => folder.key === captureFolder)?.parentKey;
                        setCaptureFolder(parent === captureRoot?.key ? undefined : parent);
                      }}><QcUiIcon kind="previous" />Back</button>}
                      <p className="capture-picker-count">{sortedCaptures.length} listed captures</p>
                      {captureChildren.map((folder) => <button key={folder.key} type="button" className="capture-picker-folder" onClick={() => setCaptureFolder(folder.key)}>
                        <QcDirectoryIcon kind="folder-outline" /><span>{folder.name || "Captures"}</span><small>{captures.filter((entry) => entry.folderKey === folder.key).length}</small>
                      </button>)}
                      {sortedCaptures.map((capture) => {
                        const version = Number(capture.version);
                        const supported = version === 1 || version === 2;
                        return <button className="capture-picker-row" key={capture.key} type="button" disabled={pending || !onSelectCapture || !supported} title={!supported ? "This capture does not report a supported version." : capture.description || undefined} onClick={() => onSelectCapture?.(capture)}>
                          <span className="capture-picker-copy"><span>{capture.name}</span>{capture.author && <small>{capture.author}</small>}</span>
                          {supported && <small className="capture-picker-version">V{version}</small>}
                        </button>;
                      })}
                    </> : captureFolder ? <>
                      <button type="button" className="capture-picker-back" onClick={() => setCaptureFolder(undefined)}><QcUiIcon kind="previous" />All capture folders</button>
                      <p className="capture-picker-count">{sortedCaptures.length} listed captures</p>
                      {sortedCaptures.map((capture) => {
                        const version = Number(capture.version);
                        const supported = version === 1 || version === 2;
                        return <button className="capture-picker-row" key={capture.key} type="button" disabled={pending || !onSelectCapture || !supported} title={!supported ? "This capture does not report a supported version." : capture.description || undefined} onClick={() => onSelectCapture?.(capture)}>
                          <span className="capture-picker-copy"><span>{capture.name}</span>{capture.author && <small>{capture.author}</small>}</span>
                          {supported && <small className="capture-picker-version">V{version}</small>}
                        </button>;
                      })}
                    </> : <>
                      <p className="capture-picker-count">{groupedCaptureCount} listed captures</p>
                      {captureGroups.map((group) => <button key={group.key} type="button" className="capture-picker-folder" onClick={() => setCaptureFolder(group.key)}>
                        <QcDirectoryIcon kind="folder-outline" /><span>{group.name}</span><small>{group.entries.length}</small>
                      </button>)}
                    </>}
                  </div>
                  : <p className="device-browser-status">{query.trim() ? "No captures match this search." : "No existing Neural Captures."}</p>
            : loading
            ? <p className="device-browser-status">Reading installed models from the Quad Cortex…</p>
            : rows.length
              ? rows.map((model) => {
                const unavailable = model.available === false;
                const pinned = pinnedModelIds.includes(model.id);
                return <button key={model.id} type="button" className={`${model.id === selectedModelId ? "is-selected" : ""}${unavailable ? " is-unavailable" : ""}${pinned ? " is-pinned" : ""}`.trim() || undefined} aria-current={model.id === selectedModelId || undefined} disabled={pending || unavailable} title={unavailable ? `${model.name} · Unavailable on this QC` : `${pinned ? "Pinned · " : "Hold to pin · "}${model.basedOn ? `Based on ${model.basedOn}` : model.name}`} onPointerDown={(event) => beginModelHold(event, model.id)} onPointerMove={moveModelHold} onPointerUp={finishModelHold} onPointerCancel={finishModelHold} onClick={() => {
                  if (consumedModelTap.current === model.id) {
                    consumedModelTap.current = undefined;
                    if (consumedTapClearTimer.current !== undefined) window.clearTimeout(consumedTapClearTimer.current);
                    consumedTapClearTimer.current = undefined;
                    return;
                  }
                  onSelect(model.id);
                }}>{pinned && <i className="device-model-pin" aria-label="Pinned"><QcUiIcon kind="pin" /></i>}{model.name}</button>;
              })
              : <p className="device-browser-status">{query.trim() ? "No devices match this search." : "No devices in this category."}</p>}
        </section>
      </main>
    </section>
  </>;
}
