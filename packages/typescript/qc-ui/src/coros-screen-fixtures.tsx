import { useState, type CSSProperties, type ReactNode } from "react";
import type { GridBlock, PresetEntry, PresetList, PresetSnapshot } from "@qc-remote/client";
import { QC_COLORS, QC_TYPOGRAPHY } from "@qc-remote/theme";
import { OFFICIAL_BLOCK_CATEGORY_BY_KEY } from "./block-visuals";
import { CorOsCaptureConnections, type CaptureConnectionView } from "./coros-capture-connections";
import { QcDeviceGlyph } from "./device-glyph";
import { COROS_DEVICE_CATEGORIES } from "./add-device-catalog";
import { QcDeviceCategoryGlyph as DeviceCategoryGlyph, QcGigStompGlyph } from "./device-category-glyph";
import { QcDeviceTile } from "./device-tile";
// The trigger upper-cases the label the popup shows in mixed case, which is
// what global-eq-filter-type draws: `LO SHELF` on the closed EQBandButton and
// `Lo Shelf` on the open selector's row.
import { EQ_FILTER_TYPE_LABELS } from "./parameter-model";
import { QcIoJack, type QcIoJackKind, type QcIoJackState } from "./qc-io-jack";
import { IoReading } from "./io-reading";
import { QcLevelMeter } from "./qc-level-meter";
import { QcPillToggle } from "./qc-pill-toggle";
import { QcRadioButton } from "./qc-radio-button";
import { QcSegmentBar } from "./qc-segment-bar";
import { QcRotaryDial } from "./qc-rotary-dial";
import { QcBrightnessInstructions, QcBrightnessModeRow } from "./qc-brightness-panel";
import { DirectoryDownloadBadge, QcCaptureFilterIcon, QcCaptureTypeGlyph, type QcCaptureTypeGlyphName, QcDirectoryBroomIcon as DirectoryBroomIcon, QcDirectoryIcon, QcDeviceEmptySlotGlyph as DeviceEmptySlotGlyph, QcDeviceWaveformGlyph as DeviceWaveformGlyph, QcEditorIcon, QcEqIcon, QcExpressionAssignmentLink, QcHardwareIcon, QcIoIcon, QcLibraryIcon, QcLooperActionGlyph, QcModeGlyph, QcNavigationBarIcon, QcPresetStackIcon, QcRouteGlyph, QcScreenGlyph, QcScreenHeaderGlyph, QcScreenVectorGlyph, QcSettingsIcon, QcSynthFooterIcon, QcSynthWaveformGlyph, QcUiIcon, type QcLooperActionGlyphName, type QcSynthWaveformName } from "./theme-icons";
import { QcFootswitchAssignmentMode, QcGridAddTile, QcMultiSelectAction, QcPillToggleLabels, QcSceneBadge, QcSceneSelector } from "./qc-shared-controls";
import { AMP_MODELS_FIRST_PAGE, CAPTURE_EDITOR_CAPTURE_NAME, CAPTURE_EDITOR_PRESET_NAME, CAPTURE_LIBRARY_FUZZ_FOLDERS, CAPTURE_LIBRARY_FUZZ_ROWS, CLOUD_CAPTURE_AUTHOR, CLOUD_CAPTURE_SEARCH_RESULT, DEVICE_SEARCH_QUERY_CHIEF, DEVICE_SEARCH_ROWS, DIRECTORY_CAPTURE_NAMES, DIRECTORY_IR_NAMES, DIRECTORY_PLUGIN_PRESET_NAMES, DIRECTORY_SEARCH_QUERY_BRIT, DIRECTORY_SEARCH_RESULTS_AMP, DIRECTORY_SEARCH_RESULTS_BRIT, DSP_LIMITED_AMPS, FACTORY_AMP_DEVICE_PRESETS, FACTORY_CAPTURE_AUTHOR, FACTORY_CAPTURE_ROWS, FACTORY_OVERDRIVE_DEVICE_PRESETS, FACTORY_PRESET_BANK_1_ROWS, FAVORITE_CAPTURE_ROWS, GIG_OFFICIAL_PRESET_TILES, GIG_OFFICIAL_PRESET_TITLE, GIG_OFFICIAL_SCENE_TILES, GIG_OFFICIAL_STOMP_TILES, LOCKED_PLUGIN_MODEL_ROWS, LOOPER_X_TITLE, MANUAL_IR_ROWS, MODEL_ADAPTIVE_GATE, MODEL_AMBIENCE, MODEL_BRIT_2203, MODEL_DIGITAL_DELAY, MODEL_OVERLORD_SYNTH, MODEL_PARAMETRIC_5, MODEL_PARAMETRIC_8, MODEL_SIMPLE_GATE, MODEL_STUDIO_CAB, MODEL_STUDIO_COMP, MODEL_TRANSPOSE, MODEL_UK_C30_CAB, OVERDRIVE_MODELS, OVERDRIVE_MODELS_FIRST_PAGE, PLUGIN_ARCHETYPE_CORY_WONG, PLUGIN_ARCHETYPE_PLINI, PLUGIN_BROWSER_ROWS, PLUGIN_FOLDER_NEURAL_DSP, PLUGIN_LICENSES, PLUGIN_MODEL_ROWS, PLUGIN_PARALLAX_X, PLUGIN_PRESET_AUTHOR_LABEL, PRESET_12_STRING_BASS, USER_DEVICE_PRESETS } from "./device-reference/coros-factory-content";
import { ACCOUNT_LINKED_BODY, ACTION_UNAVAILABLE_BODY, ASSIGN_FOOTSWITCH_PROMPT, BRIGHTNESS_BODY, CAPTURE_CALIBRATION_TIPS, CAPTURE_GROUND_LIFT_TIP, CAPTURE_INTRO_BODY, CAPTURE_INTRO_STEPS, CAPTURE_PROGRESS_BODY, CAPTURE_RESULT_BODY, CAPTURE_ROUTING_BODY, CAPTURE_ROUTING_LOAD_BOX_TIP, CAPTURE_ROUTING_TITLE, CAPTURE_SANITY_ERROR_BODY, CAPTURE_SANITY_ERROR_TITLE, CAPTURE_TYPE_BODY, CAPTURE_TYPE_CHOICES, CAPTURE_TYPE_TITLE, DELETE_PRESET_BODY, DSP_LIMIT_DIALOG, EXPRESSION_CHOOSER_BODY, EXPRESSION_TRIM_DETAIL, EXPRESSION_TRIM_HINT, GLOBAL_BYPASS_BODY, GLOBAL_BYPASS_FOOTNOTE, INPUT_CLIPPING_DIALOG, LOOPER_HINT_CLOSE_VIEW, LOOPER_HINT_START_RECORDING, LOOPER_HINT_USE, MODES_CONFIGURATION_BODY, OVERWRITE_PRESET_BODY, PASTE_CONSECUTIVE_FRAGMENTS, PASTE_OPTION_PROMPT, PLUGIN_LICENSE_NOT_FOUND, PLUGIN_REFRESH_NOTICE, RECOVERY_ENTRY_INSTRUCTION, RECOVERY_OPTIONS, RECOVERY_OPTIONS_PROMPT, RECOVERY_RELEASE_HINT, RESTART_DEVICE_BODY, SAVING_PRESET_BODY, SCENE_COMMAND_PROMPT, SHUTDOWN_DIALOG, VIRTUAL_DEVICE_PRESETS_TIP } from "./device-reference/coros-screen-text";
import { DSP_CORES, DSP_DIAGNOSTICS, DSP_SOC_COUNTERS, FOOTSWITCH_STATISTICS, USB_STATISTICS } from "./device-reference/coros-software-info";
// The shared element library is the base every screen file refines, so it is
// imported ahead of them: rules that share the .qc-screen-fixture-root scope are
// settled by specificity then order, so importing it first is what lets a screen
// still have the last word on anything it measures differently.
import "./qc-screen-elements.css";
import "./fixture-live-surface.css";
import "./remaining-fixtures.css";
import "./remaining-fixtures-fixes.css";
import "./official-settings-midi.css";
import "./official-expression.css";
import "./official-empty-slot.css";
import "./official-plugin-folders.css";
import "./official-directory.css";
import "./official-device-browser.css";
import "./official-looper-eq.css";
import "./official-io.css";
import "./official-gig.css";
import "./official-modes.css";
import { ModesCycleTrack, ModesResetToDefaults } from "./modes-configuration-screen";
import "./official-settings-device.css";
import "./official-tuner.css";
import "./settings-system-panes.css";
import "./official-overlord-synth.css";
import "./qc-device-typography.css";

type OfficialGigMode = "preset" | "scene" | "stomp" | "hybrid";

function GigStompGlyph({ index }: { index: number }) {
  return <QcGigStompGlyph index={index} />;
}

function OfficialGigModeIcon({ mode }: { mode: OfficialGigMode }) {
  if (mode === "hybrid")
    return (
      <span className="gig-mode-hybrid">
        <QcModeGlyph mode="HYBRID" />
      </span>
    );
  // A plain box, not a second <svg>: a 24-unit wrapper viewport clipped the
  // 26-unit scene and stomp vectors to their first 24 columns and anchored them
  // on the wrapper rather than on their own art.
  return (
    <span className={`gig-mode-icon gig-mode-icon-${mode}`} aria-hidden="true">
      <QcModeGlyph mode={mode.toUpperCase() as PresetSnapshot["mode"]} />
    </span>
  );
}

function CorOsOfficialGig({ mode }: { mode: OfficialGigMode }) {
  const preset = GIG_OFFICIAL_PRESET_TILES;
  const scenes = GIG_OFFICIAL_SCENE_TILES;
  const stomps = GIG_OFFICIAL_STOMP_TILES;
  const sceneMode = mode === "scene";
  const stompMode = mode === "stomp";
  return (
    <section className={`qc-screen gig-official is-${mode === "hybrid" ? "hybrid" : sceneMode ? "scene" : stompMode ? "stomp" : "preset"}`}>
      <header>
        <span>{GIG_OFFICIAL_PRESET_TITLE}</span>
        <button>
          <OfficialGigModeIcon mode={mode} />
        </button>
        <button>
          <b>A</b>
        </button>
        <button>
          <QcUiIcon kind="check" />
        </button>
      </header>
      <i />
      <main className="gig-official-tiles">
        {mode === "preset"
          ? preset.map(([location, name]) => (
              <article key={location}>
                <small>{location}</small>
                <strong>
                  {name.split("\n").map((line, i) => (
                    <span key={i}>
                      {line}
                      <br />
                    </span>
                  ))}
                </strong>
              </article>
            ))
          : sceneMode
            ? scenes.map((name, index) => (
                <article key={name} data-letter={String.fromCharCode(65 + index)}>
                  <SceneTileTools />
                  <strong>
                    {name.split("\n").map((line, i) => (
                      <span key={i}>
                        {line}
                        <br />
                      </span>
                    ))}
                  </strong>
                </article>
              ))
            : mode === "hybrid"
              ? [
                  ...scenes.slice(0, 4).map((name, index) => ({
                    letter: String.fromCharCode(65 + index),
                    name,
                    scene: true,
                  })),
                  ...stomps.slice(4).map(([letter, name]) => ({ letter, name, scene: false })),
                ].map(({ letter, name, scene }, index) => (
                  <article
                    key={letter}
                    className={scene ? "hybrid-scene" : "hybrid-stomp"}
                    data-letter={letter}
                    style={{
                      background: ["var(--qc-palette-ff272d)", "var(--qc-palette-0b2027)", "var(--qc-palette-302f10)", "var(--qc-palette-301021)", "var(--qc-palette-ff272d)", "var(--qc-palette-302f10)", "var(--qc-palette-171b18)", "var(--qc-palette-10ead5)"][index],
                    }}
                  >
                    {scene ? (
                      <SceneTileTools />
                    ) : (
                      <small>
                        <QcEditorIcon kind="footswitch" />
                        {letter}
                      </small>
                    )}
                    {!scene && (
                      <b className="has-device-glyph">
                        <GigStompGlyph index={index} />
                      </b>
                    )}
                    <strong>
                      {name.split("\n").map((line, i) => (
                        <span key={i}>
                          {line}
                          <br />
                        </span>
                      ))}
                    </strong>
                  </article>
                ))
              : stomps.map(([letter, name], index) => (
                  <article key={letter}>
                    <small>
                      <QcEditorIcon kind="footswitch" />
                      {letter}
                    </small>
                    <b className="has-device-glyph">
                      <GigStompGlyph index={index} />
                    </b>
                    <strong>
                      {name.split("\n").map((line, i) => (
                        <span key={i}>
                          {line}
                          <br />
                        </span>
                      ))}
                    </strong>
                  </article>
                ))}
      </main>
    </section>
  );
}

export type { CorOsScreenView } from "./coros-screen-fixture-data";
import type { CorOsScreenView } from "./coros-screen-fixture-data";
import { CorOsGigView, CorOsMidiOut, CorOsTempo, CorOsTuner, SceneTileTools } from "./coros-gig-view";
import { CorOsCpuMonitor } from "./coros-cpu-monitor";
import { CorOsOfficialGrid, ModeGlyph } from "./coros-official-grid";
import { CorOsReferenceSettingsPane, ReferenceSettingsIcon, SETTINGS_DEVICE_ROWS, type CorOsDeviceInformation, type ReferenceSettingsView } from "./coros-reference-settings-pane";
function DirectoryIcon({ kind, number }: Parameters<typeof QcDirectoryIcon>[0]) {
  return <QcDirectoryIcon kind={kind} number={number} />;
}

function PhysicalDirectoryStatusIcon() {
  return <QcScreenGlyph kind="status" className="physical-directory-status" />;
}

function CaptureLibraryIcon() {
  return <QcLibraryIcon kind="capture-library" />;
}

function CaptureHeaderIcon() {
  return <QcLibraryIcon kind="capture-header" />;
}

function FavoriteIcon({ kind }: { kind: "heart" | "clock" | "binoculars" | "broken-heart" }) {
  return <QcLibraryIcon kind={kind} />;
}

function DirectoryCategoryGlyph({ label }: { label: string }) {
  if (label === "Presets") return <QcDirectoryIcon kind="grid" />;
  if (label === "Neural Captures") return <QcLibraryIcon kind="capture-header" />;
  if (label === "Impulse Responses") return <QcLibraryIcon kind="impulse-response" />;
  return <DeviceCategoryGlyph label="Plugins" />;
}

type DirectoryFixtureView = "directory-presets" | "directory-categories" | "directory-captures" | "directory-irs" | "directory-plugins" | "directory-favorites" | "directory-search" | "directory-search-results" | "directory-sort" | "directory-filter" | "directory-arrange" | "directory-copy" | "directory-nested" | "directory-new-folder" | "directory-item-context" | "directory-cloud-upload";
type OfficialDirectoryView = "directory-presets" | "directory-captures" | "directory-irs" | "directory-plugins" | "directory-favorites" | "directory-search-results" | "directory-search-results-physical" | "directory-nested" | "directory-cloud-upload";
const CAPTURE_FILTERS = ["Default", "Amp", "Combo Amp", "Amp + Cab", "Cab", "Overdrive", "Fuzz", "Compressor"] as const;

// `manualReference` draws the manual's Neural Captures and Impulse Responses
// artwork rather than the unit's own capture of the same two screens. They are
// the same screen in two states, not two screens: the manual photographs a unit
// whose IR library holds seven entries and whose capture library counts 2062,
// and it scrolls neither tree.
function CorOsOfficialDirectory({ view, filter = false, manualReference = false }: { view: OfficialDirectoryView; filter?: boolean; manualReference?: boolean }) {
  if (view === "directory-presets") {
    const rows = FACTORY_PRESET_BANK_1_ROWS;
    return (
      <section className="qc-screen directory-official directory-presets-official">
        <header>
          <button className="directory-official-category">
            <DirectoryIcon kind="grid" />
            Presets{" "}
            <b>
              <QcUiIcon kind="down" />
            </b>
          </button>
          <span />
          <button>
            <DirectoryIcon kind="sort" />
          </button>
          <button>
            <DirectoryIcon kind="arrange" />
          </button>
          <button>
            <DirectoryIcon kind="search" />
          </button>
          <button>
            <DirectoryIcon kind="done" />
          </button>
        </header>
        <main>
          <nav>
            <button>
              <b>
                <DirectoryIcon kind="download" />
              </b>
              <span>Downloads</span>
            </button>
            <button>
              <b>
                <DirectoryIcon kind="cloud" />
              </b>
              <span>Cloud Presets</span>
            </button>
            <button className="is-active">
              <b>
                <DirectoryIcon kind="folder" number={0} />
              </b>
              <span>Factory Presets</span>
            </button>
            <button>
              <b>
                <DirectoryIcon kind="folder" number={1} />
              </b>
              <span>My Presets</span>
              <small>
                <QcUiIcon kind="more" />
              </small>
            </button>
            <button className="is-muted">
              <b>
                <DirectoryIcon kind="new-folder" />
              </b>
              <span>New Setlist</span>
            </button>
          </nav>
          <nav className="directory-preset-banks">
            {Array.from({ length: 14 }, (_, index) => (
              <button key={index}>{index + 1}</button>
            ))}
          </nav>
          <section className="directory-official-list">
            {rows.map((name, index) => (
              <button key={name} style={index === 0 ? { color: "var(--qc-palette-2df36a)" } : undefined}>
                <span>{name}</span>
                <b>
                  <QcUiIcon kind="more" />
                </b>
              </button>
            ))}
          </section>
        </main>
      </section>
    );
  }
  if (view === "directory-cloud-upload") {
    // Bank 4 is open, its first five slots are saved and the last three are
    // not, and the setlist column runs seven deep.
    const folders = ["My Presets", "Set Live", "Set Rec", "Set AcousticLive", "Downloaded", "Downloaded2", "Setlist Backup-\nmtos3yws-copy"];
    const rows = ["4A Demo Acoustic 1", "4B Demo Acoustic Layer", "4C Demo 12string Bass", "4D Demo Rock Rhythm", "4E QC MCP TEST_2", "4F Unsaved", "4G Unsaved", "4H Unsaved"];
    return (
      <section className="qc-screen directory-official directory-upload-official">
        <header>
          <button className="directory-official-category">
            <DirectoryIcon kind="grid" />
            Presets{" "}
            <b>
              <QcUiIcon kind="down" />
            </b>
          </button>
          <button className="is-cloud">
            <DirectoryIcon kind="cloud-upload" />
          </button>
          <span />
          <button>
            <DirectoryIcon kind="upload" />
          </button>
          <button>
            <DirectoryIcon kind="done" />
          </button>
        </header>
        <main>
          <nav>
            {folders.map((name, index) => (
              <button key={name} className={index === 0 ? "is-active" : ""}>
                <b>
                  <DirectoryIcon kind="folder" number={index + 1} />
                </b>
                <span>{name}</span>
                <small>
                  <QcUiIcon kind="more" />
                </small>
              </button>
            ))}
          </nav>
          <nav className="directory-upload-banks">
            {Array.from({ length: 14 }, (_, index) => (
              <button key={index} className={index === 3 ? "is-active" : ""}>
                {index + 1}
              </button>
            ))}
          </nav>
          <section className="directory-official-list">
            {rows.map((name, index) => (
              <button key={name} className={name.endsWith("Unsaved") ? "is-empty" : ""} style={index === 1 ? { color: "var(--qc-palette-2df36a)" } : undefined}>
                <span>{name}</span>
                {!name.endsWith("Unsaved") && (
                  <b>
                    <DirectoryIcon kind="cloud-upload" />
                  </b>
                )}
              </button>
            ))}
          </section>
        </main>
      </section>
    );
  }
  if (view === "directory-search-results" || view === "directory-search-results-physical") {
    const physical = view === "directory-search-results-physical";
    const rows = physical
      ? DIRECTORY_SEARCH_RESULTS_AMP
      : DIRECTORY_SEARCH_RESULTS_BRIT;
    return (
      <section className="qc-screen directory-official directory-search-official">
        <header>
          <button className="directory-search-field">
            <DirectoryIcon kind="search" />
            <span>{physical ? "amp" : DIRECTORY_SEARCH_QUERY_BRIT}</span>
          </button>
          <button className="directory-search-tab is-active">
            <QcModeGlyph mode="PRESET" />
            <span>({physical ? 18 : 15})</span>
          </button>
          <button className="directory-search-tab">
            <CaptureHeaderIcon />
            <span>(50)</span>
          </button>
          <button className="directory-search-tab">
            <QcLibraryIcon kind="impulse-response" />
            <span>(0)</span>
          </button>
          <span />
          <button>
            <DirectoryIcon kind="arrange" />
          </button>
          <button>
            <DirectoryIcon kind="done" />
          </button>
        </header>
        <main>
          <section className="directory-official-list">
            <header>
              DEVICE DIRECTORIES{" "}
              <b>
                <SearchSectionChevron />
              </b>
            </header>
            {rows.map(([name, author]) => (
              <button key={name}>
                <span>
                  {name}
                  <small>{author}</small>
                </span>
                <b>
                  {physical ? "A" : "B"}　<QcUiIcon kind="more" />
                </b>
              </button>
            ))}
          </section>
        </main>
      </section>
    );
  }
  if (view === "directory-nested") {
    // The directory-nested and directory-filter views are one screen, the
    // second with the filter menu over it: both are Captures Library > Fuzz,
    // six pedal folders, and the seven captures the folder lists. The badge is
    // the row's own first character, the way the 4-Comp rows carried theirs.
    const rows = CAPTURE_LIBRARY_FUZZ_ROWS;
    return (
      <section className="qc-screen directory-official directory-nested-official">
        <header>
          <button className="directory-nested-back" aria-label="Back">
            <QcUiIcon kind="previous" />
          </button>
          <button className="directory-official-category">
            <CaptureHeaderIcon />
            <span>Captures</span>
            <b>
              <QcUiIcon kind="down" />
            </b>
          </button>
          <span />
          <button>
            <DirectoryIcon kind="sort" />
          </button>
          <button className={filter ? "is-open" : ""}>
            <DirectoryIcon kind="filter" />
          </button>
          <button>
            <DirectoryIcon kind="arrange" />
          </button>
          <button>
            <DirectoryIcon kind="search" />
          </button>
          <button>
            <DirectoryIcon kind="done" />
          </button>
        </header>
        <main>
          <nav>
            {/* directory-nested and directory-filter draw the same folder
                outline on every row of this nav - the root at x=24 and each
                child at x=40, all on a cell at row + 16. */}
            <button className="directory-nested-root">
              <b>
                <DirectoryIcon kind="folder-outline" />
              </b>
              <span>
                <small>Captures Library</small>Fuzz
              </span>
            </button>
            {CAPTURE_LIBRARY_FUZZ_FOLDERS.map((label) => (
              <button key={label} className="is-child">
                <b>
                  <DirectoryIcon kind="folder-outline" />
                </b>
                <span>{label}</span>
              </button>
            ))}
          </nav>
          <section className="directory-official-list">
            {rows.map((name) => (
              <button key={name}>
                <span>
                  {name}
                  <small>{FACTORY_CAPTURE_AUTHOR}</small>
                </span>
                <b>
                  {name[0]}　<QcUiIcon kind="more" />
                </b>
              </button>
            ))}
            <aside className="qc-jump-index directory-nested-index">
              {["#", "•", "A", "•", "I", "•", "R", "•", "Z"].map((label, index) => (
                <span key={`${label}-${index}`}>{label}</span>
              ))}
            </aside>
          </section>
        </main>
        {filter && (
          <>
            <i className="directory-context-scrim" />
            <aside className="directory-tool-menu is-filter">
              {CAPTURE_FILTERS.map((label) => (
                <button key={label}>
                  <b>
                    <QcCaptureFilterIcon kind={label} />
                  </b>
                  {label}
                </button>
              ))}
            </aside>
          </>
        )}
      </section>
    );
  }
  const favorite = view === "directory-favorites";
  const captures = view === "directory-captures";
  const irs = view === "directory-irs";
  const category = favorite ? "Favorites and Recent" : captures ? "Neural Captures" : irs ? "Impulse Responses" : "Plugin Presets";
  // `directory-favorites` holds a single favourite - shown here as the
  // neutral "Combo Deluxe 212" rather than an owner's own preset name - and so
  // draws no jump-scrollbar letters; `directory-irs` holds none at all and
  // shows the same empty-folder waveform the Plugin Presets directory shows.
  // Listing seven rows on either would put six rows of edges on the screen
  // with nothing to align to.
  const rows = favorite ? FAVORITE_CAPTURE_ROWS : captures ? FACTORY_CAPTURE_ROWS : irs && manualReference ? MANUAL_IR_ROWS : [];
  const nav = favorite
    ? [
        ["category", "Presets", "active"],
        ["category", "Neural Captures", ""],
        ["category", "Impulse Responses", ""],
      ]
    : captures
      ? [
          ["⇩", "Downloads", ""],
          ["☁", "Cloud Captures", ""],
          ["≋", "Captures Library", "active counted"],
          ["□", "Factory Captures V1", "child"],
          ["□", "Factory Captures V2", "child"],
          ["□", "My Captures", "child owned"],
          ["□", "New Folder", "child muted"],
        ]
      : irs
        ? [
            // The manual's frame has Cloud IRs open, so its highlight band runs
            // y 60-120 and My IRs draws in plain white; the unit's own frame has
            // My IRs open, and its band runs 172-232.
            ["☁", "Cloud IRs", manualReference ? "active" : ""],
            ["≋", "IRs Library", "counted"],
            ["□", "My IRs", manualReference ? "child owned" : "child active owned"],
            ["□", "New Folder", "child muted"],
          ]
        : PLUGIN_LICENSES.map(([name, licensed]) => ["□", pluginLicenseDisplayName(name), licensed ? "" : "locked"]);
  return (
    <section className={`qc-screen directory-official${view === "directory-plugins" ? " is-plugins" : ""}${favorite ? " directory-favorites-official" : ""}${captures ? " directory-captures-official" : ""}${irs ? " directory-irs-official" : ""}${((captures && !filter) || irs) && !manualReference ? " is-tree-scrolled" : ""}${manualReference ? " is-manual-reference" : ""}`}>
      <header>
        <button className="directory-official-category">
          {view === "directory-plugins" ? (
            <>
              <QcUiIcon kind="plugin-presets" />
              <span>{category}</span>
            </>
          ) : captures ? (
            <>
              <CaptureHeaderIcon />
              <span>{category}</span>
            </>
          ) : favorite ? (
            <>
              <FavoriteIcon kind="heart" />
              <span>{category}</span>
            </>
          ) : (
            <>
              <DirectoryCategoryGlyph label="Impulse Responses" />
              <span>{category}</span>
            </>
          )}
          <b>
            <QcUiIcon kind="down" />
          </b>
        </button>
        {favorite ? (
          <>
            <button className="favorite-tab">
              <FavoriteIcon kind="heart" />
            </button>
            <button className="favorite-tab">
              <FavoriteIcon kind="clock" />
            </button>
            <button className="favorite-sort">
              <DirectoryIcon kind="sort" />
            </button>
            <button>
              <DirectoryIcon kind="arrange" />
            </button>
            <button>
              <DirectoryIcon kind="search" />
            </button>
            <button>
              <DirectoryIcon kind="done" />
            </button>
          </>
        ) : view === "directory-plugins" ? (
          <>
            <button className="plugin-refresh">
              <GridToolbarIcon kind="refresh" />
            </button>
            <span />
            <button>
              <DirectoryIcon kind="search" />
            </button>
            <button>
              <DirectoryIcon kind="done" />
            </button>
          </>
        ) : captures ? (
          <>
            <button className="directory-primary-action">
              <DirectoryIcon kind="cloud-upload" />
            </button>
            <span />
            <button>
              <DirectoryIcon kind="sort" />
            </button>
            <button>
              <DirectoryIcon kind="filter" />
            </button>
            <button>
              <DirectoryIcon kind="arrange" />
            </button>
            <button>
              <DirectoryIcon kind="search" />
            </button>
            <button>
              <DirectoryIcon kind="done" />
            </button>
          </>
        ) : irs ? (
          <>
            {manualReference && (
              <button className="directory-primary-action">
                <GridToolbarIcon kind="refresh" />
              </button>
            )}
            <span />
            <button>
              <DirectoryIcon kind="sort" />
            </button>
            <button>
              <DirectoryIcon kind="arrange" />
            </button>
            <button>
              <DirectoryIcon kind="search" />
            </button>
            <button>
              <DirectoryIcon kind="done" />
            </button>
          </>
        ) : (
          <>
            <button className="directory-primary-action">
              <GridToolbarIcon kind="refresh" />
            </button>
            <span />
            <button>
              <DirectoryIcon kind="sort" />
            </button>
            <button>
              <DirectoryIcon kind="arrange" />
            </button>
            <button>
              <DirectoryIcon kind="search" />
            </button>
            <button>
              <DirectoryIcon kind="done" />
            </button>
          </>
        )}
      </header>
      <main>
        <nav>
          {nav.map(([glyph, label, className]) => (
            <button
              key={label}
              className={className
                ?.split(" ")
                .filter(Boolean)
                .map((name) => `is-${name}`)
                .join(" ")}
            >
              <b>
                {/* Which of the unit's two folders a row carries depends on
                    where it sits: a solid silhouette on a top-level row, an
                    outline on a child row and on every row of the plugins nav.
                    They are separate registry entries, so this is the one
                    place that chooses. */}
                {className?.includes("locked") ? <PluginLockIcon /> : glyph === "□" ? <DirectoryIcon kind={className?.includes("muted") ? "new-folder" : className?.includes("child") || view === "directory-plugins" ? "folder-outline" : "folder"} /> : glyph === "⇩" ? <DirectoryIcon kind="download" /> : glyph === "☁" ? <DirectoryIcon kind="cloud" /> : label.endsWith("Library") ? <CaptureLibraryIcon /> : glyph === "category" ? <DirectoryCategoryGlyph label={label} /> : glyph}
                {className?.includes("owned") && <DirectoryDownloadBadge />}
              </b>
              <span>{label}</span>
              {className?.includes("counted") && <em>{captures ? (manualReference ? "2062" : "2127") : "3"}</em>}
              {className?.includes("owned") && (
                <small>
                  <QcUiIcon kind="more" />
                </small>
              )}
            </button>
          ))}
        </nav>
        <section className="directory-official-list">
          {view === "directory-plugins" ? (
            <DeviceWaveformGlyph className="plugin-directory-logo" />
          ) : irs && !manualReference ? (
            <DeviceWaveformGlyph className="ir-directory-logo" />
          ) : (
            rows.map((name) => (
              <button key={name}>
                <span>
                  {name}
                  {!favorite && (irs ? (
                    // Each IR row carries a downloaded mark: a 14px grey disc
                    // with the check knocked out of it at x 289, then the label
                    // underlined at x 306.
                    <small>
                      <b>
                        <QcUiIcon kind="check" />
                      </b>
                      <span>On device</span>
                    </small>
                  ) : (
                    <small>{captures ? FACTORY_CAPTURE_AUTHOR : ""}</small>
                  ))}
                </span>
                {favorite ? (
                  <i>
                    <b>
                      <FavoriteIcon kind="binoculars" />
                    </b>
                    <b>
                      <FavoriteIcon kind="broken-heart" />
                    </b>
                  </i>
                ) : irs ? (
                  <i>
                    <b>
                      <span>
                        <QcUiIcon kind="check" />
                      </span>
                    </b>
                    <b>
                      <DirectoryIcon kind="trash" />
                    </b>
                  </i>
                ) : (
                  <b>
                    {captures && "4　"}
                    <QcUiIcon kind="more" />
                  </b>
                )}
              </button>
            ))
          )}
          {captures && (
            <aside className="qc-jump-index directory-capture-index">
              {["#", "•", "A", "•", "I", "•", "R", "•", "Z"].map((label, index) => (
                <span key={`${label}-${index}`}>{label}</span>
              ))}
            </aside>
          )}
        </section>
      </main>
      {filter && (
        <>
          <i className="directory-context-scrim" />
          <aside className="directory-tool-menu is-filter">
            {CAPTURE_FILTERS.map((label) => (
              <button key={label}>
                <b>
                  <QcCaptureFilterIcon kind={label} />
                </b>
                {label}
              </button>
            ))}
          </aside>
        </>
      )}
    </section>
  );
}

function CorOsDirectoryFixture({ view, physicalContext = false, currentRowOverride }: { view: DirectoryFixtureView; physicalContext?: boolean; currentRowOverride?: number }) {
  // Every directory screen uses the same layout - 60px folder rows, a 2x7
  // bank grid, 52px preset rows. The compact list used for the paste, sort,
  // category and multi-select screens is the manual's directory, not the
  // unit's.
  const itemContext = ["directory-item-context", "directory-sort", "directory-categories", "directory-copy", "directory-arrange"].includes(view) || physicalContext;
  // The selector arrow states which way the category menu will move, and the
  // unit draws both: a down triangle normally, and an up triangle only while
  // the category menu is open. The two are vertical mirrors of each other.
  const categoryMenuOpen = view === "directory-categories";
  const multiSelect = view === "directory-arrange" || view === "directory-copy";
  const selectedCount = view === "directory-copy" ? 1 : 0;
  // `physicalContext` is only raised for the cloud-upload overwrite dialog,
  // which puts the directory in upload mode.
  const upload = physicalContext;
  // All of these views are set in bank 4 of My Presets. `directory-item-context`
  // and `cloud-upload-overwrite` have 4E selected; the later views have 4F.
  const currentRow = currentRowOverride ?? (view === "directory-item-context" || physicalContext ? 4 : 5);
  const category = view === "directory-captures" ? "Neural Captures" : view === "directory-irs" ? "Impulse Responses" : view === "directory-plugins" ? "Plugin Presets" : "Presets";
  const names = itemContext ? ["Demo Acoustic 1", "Demo Acoustic Layer", "Demo 12string Bass", "Demo Rock Rhythm", "QC MCP TEST_2", "Unsaved", "Unsaved", "Unsaved"] : category === "Neural Captures" ? DIRECTORY_CAPTURE_NAMES : category === "Impulse Responses" ? DIRECTORY_IR_NAMES : category === "Plugin Presets" ? DIRECTORY_PLUGIN_PRESET_NAMES : ["pyquadcortex scratch", "QC MCP TEST", "Edge of Breakup", "Ambient Lead", "Modern Rhythm", "Bass Parallel", "Acoustic Live"];
  const search = view === "directory-search" || view === "directory-search-results";
  const results = view === "directory-search-results" ? names.filter((name) => /clean|crunch|scratch/i.test(name)) : names;
  return (
    <section className={`qc-screen coros-directory-fixture${itemContext ? " is-physical-context" : ""}${upload ? " is-cloud-upload" : ""}`} aria-label={view.replaceAll("-", " ")}>
      {multiSelect ? (
        <header className="directory-multiselect">
          <QcRadioButton className="multiselect-all" shape="check" plate plateWidth={98} />
          <strong>Multi Select</strong>
          <i />
          {view === "directory-copy" ? (
            <>
              <QcMultiSelectAction enabled={selectedCount > 0}>
                <QcUiIcon kind="edit" />
              </QcMultiSelectAction>
              <QcMultiSelectAction enabled={selectedCount > 0}>
                <DirectoryBroomIcon />
              </QcMultiSelectAction>
              <QcMultiSelectAction enabled={selectedCount > 0} className="has-count">
                <QcEditorIcon kind="paste" />
                <i>1</i>
              </QcMultiSelectAction>
              <QcMultiSelectAction enabled={selectedCount > 0}>
                <QcEditorIcon className="qc-stateful-action-icon" kind="copy" />
              </QcMultiSelectAction>
            </>
          ) : (
            <>
              <QcMultiSelectAction enabled={selectedCount > 0}>
                <DirectoryIcon kind="trash" />
              </QcMultiSelectAction>
              <QcMultiSelectAction enabled={selectedCount > 0}>
                <QcUiIcon kind="edit" />
              </QcMultiSelectAction>
              <QcMultiSelectAction enabled={selectedCount > 0}>
                <QcEditorIcon className="qc-stateful-action-icon" kind="copy" />
              </QcMultiSelectAction>
              <QcMultiSelectAction enabled={selectedCount > 0}>
                <QcScreenVectorGlyph icon="directory.multi-upload" />
              </QcMultiSelectAction>
              <QcMultiSelectAction enabled={selectedCount > 0}>
                <QcLibraryIcon kind="heart" />
              </QcMultiSelectAction>
            </>
          )}
          <em />
          <button className="is-done">
            <DirectoryIcon kind="done" />
          </button>
        </header>
      ) : (
        <header>
          <button className="directory-fixture-category">
            {itemContext ? (
              <DirectoryIcon kind="grid" />
            ) : (
              <span>
                <DirectoryCategoryGlyph label={category} />
              </span>
            )}
            <strong>{category}</strong>
            <b>
              <QcUiIcon kind={categoryMenuOpen ? "up" : "collapse"} />
            </b>
          </button>
          <button className={view === "directory-cloud-upload" || upload ? "is-cloud" : ""}>{itemContext ? <DirectoryIcon kind="cloud-upload" /> : "☁"}</button>
          <i />
          {!upload && <button>{itemContext ? <DirectoryIcon kind="sort" /> : "☷"}</button>}
          {!upload && <button>{itemContext ? <DirectoryIcon kind="arrange" /> : "↕"}</button>}
          {/* Staging an upload leaves the unit one tool, not the usual pair:
              four left-aligned bars of ascending width with a double-headed
              vertical arrow to their right. That is the `arrange` entry's
              art, not `sort`'s - `sort` draws three equal bars with a
              checkmark against two of them, which is the unit's multiselect
              mark. The two registry names run the other way round from the
              unit's own asset names; the drawing is what settles it. */}
          <button>{itemContext ? <DirectoryIcon kind={upload ? "arrange" : "search"} /> : "⌕"}</button>
          <em />
          <button>{itemContext ? <DirectoryIcon kind="done" /> : <QcUiIcon kind="check" />}</button>
        </header>
      )}
      <main>
        <nav className="directory-fixture-folders">
          {itemContext ? (
            <>
              {!upload && (
                <button>
                  <b>
                    <DirectoryIcon kind="download" />
                  </b>
                  <span>Downloads</span>
                </button>
              )}
              {!upload && (
                <button>
                  <b>
                    <DirectoryIcon kind="cloud" />
                  </b>
                  <span>Cloud Presets</span>
                </button>
              )}
              {!upload && (
                <button>
                  <b>
                    <DirectoryIcon kind="folder" number={0} />
                  </b>
                  <span>Factory Presets</span>
                </button>
              )}
              <button className="is-active">
                <b>
                  <DirectoryIcon kind="folder" number={1} />
                </b>
                <span>My Presets</span>
                <b>
                  <QcUiIcon kind="more" />
                </b>
              </button>
              <button className="is-child">
                <b>
                  <DirectoryIcon kind="folder" number={2} />
                </b>
                <span>Set Live</span>
                <b>
                  <QcUiIcon kind="more" />
                </b>
              </button>
              <button className="is-child">
                <b>
                  <DirectoryIcon kind="folder" number={3} />
                </b>
                <span>Set Rec</span>
                <b>
                  <QcUiIcon kind="more" />
                </b>
              </button>
              <button className="is-child">
                <b>
                  <DirectoryIcon kind="folder" number={4} />
                </b>
                <span>Set AcousticLive</span>
                <b>
                  <QcUiIcon kind="more" />
                </b>
              </button>
              {upload && (
                <button className="is-child">
                  <b>
                    <DirectoryIcon kind="folder" number={5} />
                  </b>
                  <span>Downloaded</span>
                  <b>
                    <QcUiIcon kind="more" />
                  </b>
                </button>
              )}
              {upload && (
                <button className="is-child">
                  <b>
                    <DirectoryIcon kind="folder" number={6} />
                  </b>
                  <span>Downloaded2</span>
                  <b>
                    <QcUiIcon kind="more" />
                  </b>
                </button>
              )}
              {upload && (
                <button className="is-child">
                  <b>
                    <DirectoryIcon kind="folder" number={7} />
                  </b>
                  <span>Setlist Backup-mtos3yws-copy</span>
                  <b>
                    <QcUiIcon kind="more" />
                  </b>
                </button>
              )}
            </>
          ) : (
            <>
              <button>
                ⇩ <span>Downloads</span>
              </button>
              <button>
                ☁ <span>Cloud {category}</span>
              </button>
              <button>
                <DirectoryIcon kind="folder" /> <span>Factory {category}</span>
              </button>
              <button className="is-active">
                <DirectoryIcon kind="folder" /> <span>My {category}</span>
                <b>
                  <QcUiIcon kind="more" />
                </b>
              </button>
              {view === "directory-nested" ? (
                <>
                  <button className="is-child">
                    └ <DirectoryIcon kind="folder" /> <span>Set Live</span>
                  </button>
                  <button className="is-child is-active">
                    　└ <DirectoryIcon kind="folder" /> <span>Festival</span>
                  </button>
                </>
              ) : (
                <button className="is-child">
                  └ <DirectoryIcon kind="folder" /> <span>Set Live</span>
                </button>
              )}
              <button className="is-new">
                <DirectoryIcon kind="folder" />
                <QcUiIcon kind="add" /> <span>New {category === "Presets" ? "Setlist" : "Folder"}</span>
              </button>
            </>
          )}
        </nav>
        <nav className="directory-fixture-banks">
          {(itemContext ? Array.from({ length: 14 }, (_, index) => index + 1) : [29, 30, 31, 32, 33, 34, 35]).map((bank) => (
            <button key={bank} className={bank === (itemContext ? 4 : 32) ? "is-active" : ""}>
              {bank}
            </button>
          ))}
        </nav>
        <section className="directory-fixture-items">
          {search && (
            <div className="directory-search-field">
              <span>⌕</span>
              <strong>{view === "directory-search-results" ? "Clean" : "Search Directory"}</strong>
              <button>×</button>
            </div>
          )}
          {(search ? results : names).map((name, index) =>
            itemContext ? (
              <button key={`${name}-${index}`} className={`${index === currentRow ? "is-current" : ""}${name === "Unsaved" ? " is-empty" : ""}`}>
                {multiSelect && <QcRadioButton className="preset-select" shape="check" />}
                <span className="physical-preset-name">{`4${String.fromCharCode(65 + index)} ${name}`}</span>
                {upload ? (
                  <b className="preset-upload">
                    <DirectoryIcon kind="cloud-upload" />
                  </b>
                ) : (
                  !multiSelect && (
                    <b>
                      <QcUiIcon kind="more" />
                    </b>
                  )
                )}
              </button>
            ) : (
              <button key={name} className={index === 0 ? "is-current" : ""}>
                <strong>{category === "Presets" ? `32${String.fromCharCode(65 + index)}` : <DirectoryCategoryGlyph label={category} />}</strong>
                <span>
                  {name}
                  <small>{category === "Neural Captures" ? "GUITAR · AMP" : category === "Impulse Responses" ? "48 kHz · 1024 samples" : category === "Plugin Presets" ? PLUGIN_PRESET_AUTHOR_LABEL : "USER"}</small>
                </span>
                {view === "directory-arrange" ? (
                  <i>☰</i>
                ) : (
                  <b>
                    <QcUiIcon kind="more" />
                  </b>
                )}
              </button>
            ),
          )}
        </section>
      </main>
      {categoryMenuOpen && (
        <>
          <i className="directory-context-scrim" />
          {/* Each row's symbol is the one the unit draws: the Presets chip is
              pixel-for-pixel mode.preset, identical to capture-type's header
              chip; Neural Captures is the capture-header stack rather than
              the library shelf; and Plugin Presets is interface.plugin-presets,
              a filled socket rather than the outlined browser-category plug. */}
          <aside className="directory-category-menu">
            <button>
              <span>
                <FavoriteIcon kind="heart" />
              </span>
              Favorites and Recent
            </button>
            <button className="is-active">
              <span>
                <QcModeGlyph mode="PRESET" />
              </span>
              Presets
              <b>
                <QcUiIcon kind="check" />
              </b>
            </button>
            <button>
              <span>
                <CaptureHeaderIcon />
              </span>
              Neural Captures
            </button>
            <button>
              <span>
                <QcLibraryIcon kind="impulse-response" />
              </span>
              Impulse Responses
            </button>
            <button>
              <span>
                {/* The registry renderer, not QcUiIcon: the menu sheet sizes
                    `> span svg` to 24px, which the #root#root rule on
                    .qc-ui-icon outranks, leaving the socket at the span's 20px
                    font size (16x16 where the unit draws 20x20). */}
                <QcScreenVectorGlyph icon="interface.plugin-presets" />
              </span>
              Plugin Presets
            </button>
          </aside>
        </>
      )}
      {view === "directory-favorites" && (
        <aside className="directory-favorites-panel">
          <header>
            <button className="is-active">FAVORITES</button>
            <button>RECENT</button>
          </header>
          {names.slice(0, 5).map((name) => (
            <button key={name}>
              <span>★</span>
              {name}
              <b>
                <QcUiIcon kind="more" />
              </b>
            </button>
          ))}
        </aside>
      )}
      {/* Title, options and active-item styling taken from the device: the
        CorOS tree for `directory-sort` reads "Sort By" over Banks, Name,
        Date Added, Author, Preferred Instrument, and marks the active row in
        green rather than with a tick. */}
      {view === "directory-sort" && (
        <>
          <i className="directory-context-scrim" />
          <aside className="directory-tool-menu">
            <strong>Sort By</strong>
            {["Banks", "Name", "Date Added", "Author", "Preferred Instrument"].map((label, index) => (
              <button key={label} className={index === 0 ? "is-active" : ""}>
                {label}
              </button>
            ))}
          </aside>
        </>
      )}
      {/* The device's filter is a capture-type list, not a scope list. This is
        `directory-filter`, from the Neural Captures directory, where the
        funnel appears: Default, Amp, Combo Amp, Amp + Cab, Cab, Overdrive,
        Fuzz, Compressor, each with its category glyph and no heading. The
        previous FILTER / All items / Favorites / Downloaded / My items /
        Factory menu does not exist on the unit. */}
      {/* The device calls this Multi Select, not Arrange, and draws it as a top
        header: a select-all box, the green title, then trash / rename / copy /
        upload / favourite and a done tick. The previous bar - "ARRANGE",
        "Drag items to reorder them", CANCEL and DONE along the bottom - came
        from the manual rather than from the unit. */}
      {/* Pasting does not ask for a destination folder; the destination is
        wherever you already are. It asks how to lay the items into that
        folder's banks. This is `directory-copy`; the previous
        "Copy 3 items to..." folder picker with COPY HERE was not on the unit.
        CorOS emits its own <b> markup inside these label strings. */}
      {view === "directory-copy" && (
        <>
          <i className="directory-context-scrim" />
          <aside className="directory-copy-dialog">
            <header>Choose pasting option</header>
            <p>
              {PASTE_OPTION_PROMPT}<b>My Presets</b>:
            </p>
            {[
              <>Choose each slot manually</>,
              <>
                {PASTE_CONSECUTIVE_FRAGMENTS[0]}<b>chosen</b>{PASTE_CONSECUTIVE_FRAGMENTS[1]}
              </>,
              <>
                {PASTE_CONSECUTIVE_FRAGMENTS[0]}<b>empty</b>{PASTE_CONSECUTIVE_FRAGMENTS[1]}
              </>,
            ].map((label, index) => (
              <button key={index} className={index === 0 ? "is-active" : ""}>
                <span>{label}</span>
                {index === 0 && (
                  <i>
                    <QcUiIcon kind="check" />
                  </i>
                )}
              </button>
            ))}
            <footer>
              <button>CANCEL</button>
              <button className="is-primary">CONTINUE</button>
            </footer>
          </aside>
        </>
      )}
      {view === "directory-item-context" && (
        <>
          <i className="directory-context-scrim" />
          <aside className="directory-item-menu">
            {["Edit", "Copy", "Cut", "Paste to replace", "Delete"].map((label) => (
              <button key={label}>{label}</button>
            ))}
          </aside>
        </>
      )}
    </section>
  );
}

type RemainingFixtureView = "fixture-boot" | "fixture-shutdown" | "fixture-copy-scene" | "fixture-swap-scene" | "fixture-delete" | "fixture-input-gate" | "fixture-editor-pages" | "fixture-editor-cab" | "fixture-editor-eq" | "fixture-editor-capture" | "fixture-warning-clip" | "fixture-warning-dsp";

function CorOsRemainingFixture({ view }: { view: RemainingFixtureView }) {
  if (view === "fixture-boot")
    return (
      <section className="qc-screen coros-boot">
        <b>
          <QcHardwareIcon kind="brand-pulse" />
        </b>
        <h1>QUAD CORTEX</h1>
        <i>
          <span />
        </i>
        <small>STARTING COROS</small>
      </section>
    );
  if (view === "fixture-delete") return <CorOsDeleteConfirmation />;
  const dialog = view === "fixture-shutdown" ? SHUTDOWN_DIALOG : view === "fixture-warning-clip" ? INPUT_CLIPPING_DIALOG : view === "fixture-warning-dsp" ? DSP_LIMIT_DIALOG : undefined;
  if (dialog)
    return (
      <section className="qc-screen coros-fixture-dialog">
        <div className="fixture-grid-ghost">
          {Array.from({ length: 7 }, (_, i) => (
            <i key={i} />
          ))}
        </div>
        <aside className={view.includes("warning") ? "is-warning" : ""}>
          <b>{view.includes("warning") ? "!" : "?"}</b>
          <h1>{dialog[0]}</h1>
          <p>{dialog[1]}</p>
          <footer>
            <button>CANCEL</button>
            <button>{dialog[2]}</button>
          </footer>
        </aside>
      </section>
    );
  if (view === "fixture-copy-scene" || view === "fixture-swap-scene")
    return (
      <section className="qc-screen coros-fixture-dialog is-scene-command">
        <div className="scene-command-grid">
          <header>
            <strong>
              <span>32</span>D
            </strong>
            <span>Unsaved</span>
            <nav>
              <GridToolbarIcon kind="undo" />
              <b>A</b>
              <GridToolbarIcon kind="save" />
              <GridToolbarIcon kind="more" />
            </nav>
          </header>
          <span className="scene-command-mode">
            <ModeGlyph mode="STOMP" />
            <b>STOMP</b>
          </span>
          <main>
            <span className="scene-command-insert">
              <QcUiIcon kind="add" />
            </span>
            {Array.from({ length: 8 }, (_, index) => (
              <i key={index}>
                {index === 0 ? (
                  <>
                    In
                    <br />1
                  </>
                ) : index === 1 ? (
                  <>
                    Multi
                    <br />
                    Out
                  </>
                ) : (
                  <QcUiIcon kind="add" />
                )}
              </i>
            ))}
          </main>
        </div>
        <aside>
          <h1>{view === "fixture-copy-scene" ? "Copy Scene A" : "Swap Scene A"}</h1>
          <p>{SCENE_COMMAND_PROMPT}</p>
          <footer>
            <button>CANCEL</button>
          </footer>
        </aside>
      </section>
    );
  if (view === "fixture-editor-capture")
    return (
      <section className="qc-screen capture-editor-physical" aria-label="Neural Capture editor">
        <header>
          <strong>
            <span>1</span>D
          </strong>
          <h1>{CAPTURE_EDITOR_PRESET_NAME}</h1>
          <nav>
            <i>
              <GridToolbarIcon kind="undo" />
            </i>
            <b>A</b>
            <i>
              <GridToolbarIcon kind="save" />
            </i>
            <i>
              <GridToolbarIcon kind="more" />
            </i>
          </nav>
          <em>
            <ModeGlyph mode="PRESET" />
            <span>PRESET</span>
          </em>
        </header>
        <div className="capture-editor-grid">
          <span>
            In
            <br />1
          </span>
          <i className="capture-cable" />
          <i className="capture-block is-gate">
            <DeviceCategoryGlyph label="Utility" />
          </i>
          <i className="capture-block is-capture">
            <DeviceCategoryGlyph label="Neural Capture" />
          </i>
          <i className="capture-block">
            <DeviceCategoryGlyph label="Neural Capture" />
          </i>
          <span>
            Row
            <br />3
          </span>
        </div>
        <section className="capture-editor-panel">
          <header>
            <button>
              <QcUiIcon kind="more" />
            </button>
            <span>
              <small>NEURAL CAPTURE</small>
              <strong>{CAPTURE_EDITOR_CAPTURE_NAME}</strong>
            </span>
            <nav>
              <i>
                <QcEditorIcon kind="scene-previous" />
              </i>
              <b>A</b>
              <i>
                <QcEditorIcon kind="scene-next" />
              </i>
            </nav>
            <button className="capture-editor-power">
              <QcEditorIcon kind="bypass" />
            </button>
            <button className="capture-editor-confirm">
              <QcEditorIcon kind="confirm" />
            </button>
          </header>
          <main>
            {/* Dial positions follow the pointer's angle on the unit's
                135..405 sweep, so the fill follows the pointer the way the
                unit's does. */}
            {[
              ["GAIN", "-3.8 dB", 0.4231],
              ["BASS", "1.0 dB", 0.5777],
              ["MID", "0.0 dB", 0.5],
              ["TREBLE", "1.6 dB", 0.6271],
              ["VOLUME", "-3.8 dB", 0.4231],
            ].map(([label, value, dial]) => (
              <section key={label as string}>
                <span>{label}</span>
                <QcRotaryDial value={dial as number} accent={QC_COLORS.reference.utilityMark} />
                <strong>{value}</strong>
              </section>
            ))}
          </main>
        </section>
      </section>
    );
  if (view === "fixture-input-gate")
    return (
      <section className="qc-screen coros-input-gate" aria-label="Input Gate Control">
        <div className="input-gate-grid">
          <header>
            <strong>
              <span>4</span>E
            </strong>
            <h1>QC MCP TEST_2</h1>
            <nav>
              <i className="input-gate-undo">
                <GridToolbarIcon kind="undo" />
              </i>
              <b>A</b>
              <i>
                <GridToolbarIcon kind="save" />
              </i>
              <i>
                <QcUiIcon kind="more" />
              </i>
            </nav>
            <em>
              <ModeGlyph mode="STOMP" />
              <span>STOMP</span>
            </em>
          </header>
          <main>
            <span className="input-gate-route">
              In
              <br />1
            </span>
            <i className="input-gate-cable" />
            {/* The unit draws this tile bypassed while its editor is open:
                `input-gate-control` inks its ring and its mark alike at the
                editor-disabled shadow over 69..132 x 115..178, one flat value
                with no category colour in the box. We drew it lit. */}
            <span className="input-gate-device">
              <QcDeviceTile category="Utility" />
            </span>
            <span className="input-gate-output">
              Multi
              <br />
              Out
            </span>
            <span className="input-gate-plus is-left">
              <QcUiIcon kind="add" />
            </span>
            <span className="input-gate-plus">
              <QcUiIcon kind="add" />
            </span>
          </main>
        </div>
        <section className="input-gate-panel">
          <header>
            <button>
              <QcUiIcon kind="more" />
            </button>
            <span>
              <small>INPUT GATE CONTROL</small>
              <strong>Path 1</strong>
            </span>
            <nav>
              <i>
                <QcEditorIcon kind="scene-previous" />
              </i>
              <b>A</b>
              <i>
                <QcEditorIcon kind="scene-next" />
              </i>
            </nav>
            <em />
            <button className="input-gate-power">
              <QcEditorIcon kind="bypass" />
            </button>
            <button>
              <QcUiIcon kind="check" />
            </button>
          </header>
          <main>
            <section>
              <span>NOISE REDUCTION</span>
              {/* input-gate-control's left pane is unlit: its ring reads
                  99,101,99, its pointer 90,97,90 and its face 33,40,33 - not
                  the lit dial dimmed, which is why they are named here. The
                  pointer sits at 218.3 degrees. */}
              <QcRotaryDial
                className="input-gate-knob reduction"
                value={0.3087}
                accent="var(--qc-palette-636563)"
                pointer="var(--qc-palette-5a615a)"
                face="var(--qc-palette-212821)"
                track="var(--qc-palette-212821)"
                rim="var(--qc-palette-181c18)"
              />
              <strong>
                30.0 <small>%</small>
              </strong>
            </section>
            <section>
              <span>GAIN REDUCTION</span>
              <strong>
                0.0 <small>dB</small>
              </strong>
              <i className="input-gate-meter">
                <b />
              </i>
            </section>
            <section>
              <span>INPUT GAIN</span>
              <QcRotaryDial className="input-gate-knob gain" value={0.5} accent={QC_COLORS.reference.iconPrimary} />
              <strong>
                0.0 <small>dB</small>
              </strong>
            </section>
          </main>
        </section>
      </section>
    );
  // The amp editor is the shared Grid + action bar with two parameter strips,
  // not the manual's full-screen page layout.
  if (view === "fixture-editor-pages") {
    /* Every 5.0 reading sits at exactly 270 degrees - the middle of the unit's
       sweep. */
    const ampParameters: Array<[string, string, number]> = [
      ["GAIN", "5.0", 0.5],
      ["BASS", "5.0", 0.5],
      ["MID", "5.0", 0.5],
      ["TREBLE", "5.0", 0.5],
      ["PRESENCE", "5.0", 0.5],
      ["MASTER", "3.0", 0.3086],
      ["OUTPUT", "0.0 dB", 0.5],
    ];
    return (
      <section className="qc-screen coros-assignment is-amp-editor" aria-label="Amp parameter pages">
        <PhysicalEditorUnderlay slot="4" letter="E" title="QC MCP TEST_2*" category="GUITAR AMP" device={MODEL_BRIT_2203} showPresetStack blocks={2} blockGlyphs={["Utility", "Amp"]} expression={false} mode="PRESET">
          <div className="assignment-parameters">
            {ampParameters.slice(0, 5).map(([label, value, dial]) => (
              <section key={label}>
                <span>{label}</span>
                <QcRotaryDial className="assignment-rotary" value={dial} accent={QC_COLORS.browserCategory.amp} />
                <strong>{value}</strong>
              </section>
            ))}
          </div>
          <div className="assignment-parameters is-row-2">
            {ampParameters.slice(5).map(([label, value, dial]) => (
              <section key={label}>
                <span>{label}</span>
                <QcRotaryDial className="assignment-rotary" value={dial} accent={QC_COLORS.browserCategory.amp} />
                <strong>{value}</strong>
              </section>
            ))}
            {[0, 1, 2].map((index) => (
              <section key={`empty-${index}`} />
            ))}
          </div>
        </PhysicalEditorUnderlay>
      </section>
    );
  }
  const editor = view === "fixture-editor-cab" ? [MODEL_UK_C30_CAB, "CABINET", ["MIC 1 · 57", "MIC 2 · 121", "POSITION", "DISTANCE", "LEVEL", "PAN"]] : view === "fixture-editor-eq" ? [MODEL_PARAMETRIC_8, "EQUALIZER", ["LOW CUT", "BAND 1", "BAND 2", "BAND 3", "HIGH CUT", "LEVEL"]] : [MODEL_AMBIENCE, "REVERB · PAGE 2/2", ["MOD RATE", "MOD DEPTH", "DUCKING", "TRAILS", "WIDTH", "MIX"]];
  return (
    <section className={`qc-screen coros-detail-editor ${view}`}>
      <header>
        <button>
          <QcUiIcon kind="more" />
        </button>
        <span>
          <small>{editor[1] as string}</small>
          <strong>{editor[0] as string}</strong>
        </span>
        <i className="status-dot" />
        <button>
          <QcUiIcon kind="check" />
        </button>
      </header>
      {view === "fixture-editor-cab" && (
        <div className="cab-stage">
          <span>57</span>
          <b>
            <DeviceCategoryGlyph label="Cab" />
          </b>
          <span>121</span>
        </div>
      )}
      {view === "fixture-editor-eq" && (
        <svg viewBox="0 0 800 150" preserveAspectRatio="none">
          <path d="M0 120 C100 120 110 35 205 55 S335 115 410 70 S565 20 640 75 S735 105 800 60" />
        </svg>
      )}
      <main>
        {(editor[2] as string[]).map((label, index) => (
          <section key={label}>
            <span>{label}</span>
            <i>
              <b style={{ transform: `rotate(${index * 23 - 35}deg)` }} />
            </i>
            <strong>{index % 2 ? "0.0 dB" : index === 0 ? "80 Hz" : "5.0"}</strong>
          </section>
        ))}
      </main>
      <footer>
        <button>1</button>
        <button className="is-active">2</button>
        <span />
        <button>BYPASS</button>
      </footer>
    </section>
  );
}

type SystemFixtureView = "recovery-entry" | "recovery-options" | "overlay-keyboard" | "overlay-confirmation" | "overlay-overwrite" | "overlay-error" | "overlay-busy";

const KEYBOARD_ROWS = [
  ["Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P"],
  ["A", "S", "D", "F", "G", "H", "J", "K", "L"],
  ["Shift", "Z", "X", "C", "V", "B", "N", "M", "Backspace"],
  ["123", ",", "Space", ".", "Done"],
] as const;

// The keyboard screen CorOS opens over a folder's naming screen; only the
// header and the selected name differ, and this one leaves Done enabled
// because the field is not empty.
function CorOsDirectoryNameScreen() {
  const shifted = ["", "", "", "", "", "", "", "(", ")"];
  return (
    <section className="qc-screen coros-physical-keyboard is-name-editor" aria-label="Name folder">
      <header>
        <button>
          <QcUiIcon kind="close" />
        </button>
        <button className="keyboard-save-mark">
          <svg viewBox="695 0 48 48" aria-hidden="true">
            <QcScreenHeaderGlyph kind="nav-save" />
          </svg>
        </button>
      </header>
      <h1>
        <b>My IRs 2</b>
      </h1>
      <div className="physical-keyboard-rows">
        {KEYBOARD_ROWS.map((row, rowIndex) => (
          <div key={rowIndex}>
            {row.map((key, keyIndex) => (
              <button key={key} className={key === "Space" ? "is-space" : key === "Shift" ? "is-shift" : key === "Backspace" ? "is-backspace" : key === "Done" ? "is-done" : key === "123" ? "is-numeric" : ""}>
                {rowIndex < 2 && <small>{rowIndex === 0 ? (keyIndex + 1) % 10 : shifted[keyIndex]}</small>}
                {key === "Backspace" ? <QcUiIcon kind="backspace" /> : key === "Shift" ? <QcScreenGlyph kind="shift" /> : key}
              </button>
            ))}
          </div>
        ))}
      </div>
    </section>
  );
}

function CorOsKeyboardScreen() {
  const shifted = ["", "", "", "", "", "", "'", "(", ")"];
  return (
    <section className="qc-screen coros-physical-keyboard" aria-label="On-screen keyboard">
      <header>
        <button>
          <QcUiIcon kind="close" />
        </button>
        <button className="keyboard-default-mark" aria-label="Save as default User Preset">
          <QcRadioButton shape="check" plate surface="var(--qc-screen-surface-control)" />
        </button>
        <span>Save as default User Preset</span>
        <b>#</b>
        <button className="keyboard-save-mark">
          <QcEditorIcon kind="save" />
        </button>
      </header>
      <h1>Name your Virtual Device Preset</h1>
      <div className="physical-keyboard-rows">
        {KEYBOARD_ROWS.map((row, rowIndex) => (
          <div key={rowIndex}>
            {row.map((key, keyIndex) => (
              <button key={key} className={key === "Space" ? "is-space" : key === "Shift" ? "is-shift" : key === "Backspace" ? "is-backspace" : key === "Done" ? "is-done" : key === "123" ? "is-numeric" : ""}>
                {rowIndex < 2 && <small>{rowIndex === 0 ? (keyIndex + 1) % 10 : shifted[keyIndex]}</small>}
                {key === "Backspace" ? <QcUiIcon kind="backspace" /> : key === "Shift" ? <QcScreenGlyph kind="shift" /> : key}
              </button>
            ))}
          </div>
        ))}
      </div>
    </section>
  );
}

// CorOS reuses one confirmation overlay for more than deletion: uploading a
// preset that already exists in Cortex Cloud raises the same dialog with an
// OVERWRITE action, named `cloud-upload-overwrite`. Only the copy and
// the confirm label differ. Note the device's own graphics tree exposes just
// the two buttons for this dialog - its title and body are not in the tree.
function PhysicalGridUnderlay({ rows = 1, lit = false }: { rows?: 1 | 2; lit?: boolean }) {
  return (
    <div className={`physical-grid-underlay${rows === 2 ? " is-full-grid" : ""}${lit ? " is-lit" : ""}`}>
      <div className="underlay-grid">
        <header>
          <strong>
            <span>4</span>E
          </strong>
          <h1>QC MCP TEST_2</h1>
          <nav>
            <i>
              <GridToolbarIcon kind="undo" />
            </i>
            <b>A</b>
            <i>
              <GridToolbarIcon kind="save" />
            </i>
            <i>
              <QcUiIcon kind="more" />
            </i>
          </nav>
          <em>
            <ModeGlyph mode="STOMP" />
            STOMP
          </em>
        </header>
        <main>
          <span className="underlay-route">
            In
            <br />1
          </span>
          <i className="underlay-cable" />
          <i className="underlay-block is-block-1">
            <DeviceCategoryGlyph label="Utility" />
          </i>
          <span className="underlay-output">
            Multi
            <br />
            Out
          </span>
          {rows === 2 && (
            <>
              <span className="underlay-route is-row-2">
                In
                <br />1
              </span>
              <i className="underlay-cable is-row-2" />
              <span className="underlay-output is-row-2">
                Multi
                <br />
                Out
              </span>
              <i className="underlay-slot is-left">
                <QcUiIcon kind="add" />
              </i>
              <i className="underlay-slot is-right">
                <QcUiIcon kind="add" />
              </i>
              <i className="underlay-slot is-left is-row-2">
                <QcUiIcon kind="add" />
              </i>
              <i className="underlay-slot is-right is-row-2">
                <QcUiIcon kind="add" />
              </i>
            </>
          )}
        </main>
      </div>
    </div>
  );
}

function CorOsDeleteConfirmation({ variant = "delete" }: { variant?: "delete" | "overwrite" }) {
  const copy =
    variant === "overwrite"
      ? {
          title: "Preset already exists",
          body: OVERWRITE_PRESET_BODY,
          confirm: "OVERWRITE",
          label: "Overwrite cloud preset confirmation",
        }
      : {
          title: "QC MCP TEST_2",
          body: DELETE_PRESET_BODY,
          confirm: "DELETE PRESET",
          label: "Delete preset confirmation",
        };
  // The delete variant is raised from the Grid and the overwrite variant from
  // the Directory in cloud-upload mode - the same directory view, with the
  // dialog on top of it, down to the green-bordered upload button and the
  // cloud icon on every row.
  const overGrid = variant !== "overwrite";
  return (
    <section className={`qc-screen coros-physical-confirmation${overGrid ? " is-over-grid" : ""}`} aria-label={copy.label}>
      {/* The row the unit marks is the preset the dialog is about to overwrite -
          the second one, which is the name the message names. */}
      {overGrid ? <PhysicalGridUnderlay rows={2} /> : <CorOsDirectoryFixture view="directory-cloud-upload" physicalContext currentRowOverride={1} />}
      <i className="confirmation-scrim" />
      <aside>
        <h1>{copy.title}</h1>
        <p>{copy.body}</p>
        <footer>
          <button>CANCEL</button>
          <button>{copy.confirm}</button>
        </footer>
      </aside>
    </section>
  );
}

function CorOsSystemFixture({ view }: { view: SystemFixtureView }) {
  if (view === "recovery-entry")
    return (
      <section className="qc-screen coros-recovery">
        <div className="recovery-logo">
          <QcHardwareIcon kind="brand-pulse" />
        </div>
        <h1>Recovery Mode</h1>
        <p>{RECOVERY_ENTRY_INSTRUCTION}</p>
        <div className="recovery-switches">
          <b>A</b>
          <span>HOLD</span>
          <b>H</b>
        </div>
        <small>{RECOVERY_RELEASE_HINT}</small>
      </section>
    );
  if (view === "recovery-options")
    return (
      <section className="qc-screen coros-recovery">
        <div className="recovery-logo">
          <QcHardwareIcon kind="brand-pulse" />
        </div>
        <h1>Recovery Mode</h1>
        <p>{RECOVERY_OPTIONS_PROMPT}</p>
        <div className="recovery-options">
          {RECOVERY_OPTIONS.map(([title, detail], index) => (
            <button key={title} className={index === 0 ? "is-active" : index === 2 ? "is-danger" : ""}>
              <strong>{title}</strong>
              <small>{detail}</small>
              <b>
                <QcUiIcon kind="next" />
              </b>
            </button>
          ))}
        </div>
      </section>
    );
  return (
    <section className="qc-screen coros-system-overlay">
      <div className={`overlay-underlay${view === "overlay-overwrite" ? " is-directory" : ""}`}>
        {view === "overlay-overwrite" ? (
          <CorOsOfficialDirectory view="directory-cloud-upload" />
        ) : (
          <>
            <header>
              <span>32H Demo Scratch</span>
              <b>A</b>
            </header>
            <main>
              {[1, 2, 3, 4, 5].map((item) => (
                <i key={item} />
              ))}
            </main>
          </>
        )}
      </div>
      {view === "overlay-keyboard" && <CorOsKeyboardScreen />}
      {view === "overlay-confirmation" && <CorOsDeleteConfirmation />}
      {view === "overlay-overwrite" && <CorOsDeleteConfirmation variant="overwrite" />}
      {view === "overlay-error" && (
        <aside className="system-dialog">
          <b className="dialog-icon is-error">!</b>
          <h1>Action unavailable</h1>
          <p>{ACTION_UNAVAILABLE_BODY}</p>
          <footer>
            <button>OK</button>
          </footer>
        </aside>
      )}
      {view === "overlay-busy" && (
        <aside className="system-dialog is-busy">
          <b className="dialog-spinner" />
          <h1>Saving preset</h1>
          <p>{SAVING_PRESET_BODY}</p>
        </aside>
      )}
    </section>
  );
}

type CaptureFixtureView = "capture-intro" | "capture-type" | "capture-routing" | "capture-calibration" | "capture-progress" | "capture-sanity-error" | "capture-result" | "capture-save";

/**
 * The device the capture is being taken from, as `capture-calibration` and
 * `capture-result` illustrate it: the shared block tile showing a Cab, with the
 * signal line running through it.
 *
 * Both views draw the same 64-unit tile inside a 70-unit ground - 523..586
 * within 520..589 on `capture-calibration`, the same box eight left and five up
 * on `capture-result` - and the mark inside it is `device-category.cab` at 1:1
 * in the 40-unit box every tile gives its art. There is no crosshair-in-a-circle
 * mark to render here.
 */
function CaptureTargetIcon() {
  return (
    <span className="capture-target-icon">
      <QcDeviceTile category="Cab" surround />
    </span>
  );
}

/** The six capture types `capture-save` offers, three of them library marks. */
const CAPTURE_TYPE_TILES: ReadonlyArray<{ type: string; category?: string; mark?: QcCaptureTypeGlyphName }> = [
  { type: "Default", category: "Neural Capture" },
  { type: "Amp", category: "Amp" },
  { type: "Combo Amp", mark: "combo-amp-tile" },
  { type: "Amp + Cab", mark: "amp-cab-tile" },
  { type: "Cab", category: "Cab" },
  { type: "Overdrive", mark: "overdrive-tile" },
];

const SETTINGS_CLOUD_PATH = "M8 25h16a6 6 0 0 0 1-11.9A9 9 0 0 0 8 11a7 7 0 0 0 0 14Z";

function CorOsOfficialCapture({ view }: { view: "capture-calibration" | "capture-progress" | "capture-sanity-error" | "capture-result" | "capture-save" }) {
  if (view === "capture-sanity-error")
    return (
      <section className="qc-screen capture-official capture-official-progress capture-official-error is-sanity-error">
        <header>
          <span>Neural Capture</span>
          <button>×</button>
        </header>
        <main>
          <nav>
            {(
              [
                ["check", "Calibration", ""],
                ["check", "Recording Signals", ""],
                ["error", "Sanity Check", "is-error"],
                ["", "Training", "is-pending"],
              ] as const
            ).map(([icon, label, state]) => (
              <div key={label} className={state}>
                <b>{icon === "check" ? <QcUiIcon kind="check" /> : icon === "error" ? "!" : ""}</b>
                {label}
              </div>
            ))}
          </nav>
          <section>
            <h1>{CAPTURE_SANITY_ERROR_TITLE}</h1>
            <p>
              {CAPTURE_SANITY_ERROR_BODY[0]}
              <br />
              {CAPTURE_SANITY_ERROR_BODY[1]}<br />
              {CAPTURE_SANITY_ERROR_BODY[2]}
            </p>
            <strong>30 %</strong>
            <i className="capture-official-progress-bar">
              <b />
            </i>
            <button>CALIBRATION SCREEN</button>
          </section>
        </main>
      </section>
    );
  if (view === "capture-progress")
    return (
      <section className="qc-screen capture-official capture-official-progress">
        <header>
          <span>Neural Capture</span>
          <button>×</button>
        </header>
        <main>
          <nav>
            {["Calibration", "Recording Signals", "Sanity Check", "Training"].map((label, index) => (
              <div key={label}>
                <b>
                  <QcScreenGlyph kind={index < 3 ? "capture-step-check" : "capture-step-next"} />
                </b>
                {label}
              </div>
            ))}
          </nav>
          <section>
            <h1>Neural Capture in progress</h1>
            <p>
              {CAPTURE_PROGRESS_BODY[0]}
              <br />
              {CAPTURE_PROGRESS_BODY[1]}
            </p>
            <strong>88 %</strong>
            <i className="capture-official-progress-bar">
              <b style={{ width: "88.9%" }} />
            </i>
            <em>
              <QcScreenGlyph kind="progress" />
            </em>
          </section>
        </main>
      </section>
    );
  if (view === "capture-save")
    return (
      <section className="qc-screen capture-official capture-official-save">
        <header>
          <button>
            <QcUiIcon kind="close" />
          </button>
          <button className="capture-folder">
            <DirectoryIcon kind="folder" />
            <span>My Captures</span>
          </button>
          <button>Name</button>
          <button className="capture-note">
            <QcUiIcon kind="capture-note" />
          </button>
          <button className="capture-save-now">
            <QcEditorIcon kind="save" />
          </button>
        </header>
        <main>
          <small>TYPE OF CAPTURE</small>
          <h1>Amp</h1>
          <div className="capture-kinds">
            {CAPTURE_TYPE_TILES.map(({ type, category, mark }, index) => (
              <button key={type} className={index === 1 ? "is-active" : ""} aria-label={type}>
                <QcDeviceTile
                  category={category}
                  art={mark ? <QcCaptureTypeGlyph kind={mark} /> : undefined}
                  selected={index === 1}
                  badge={index > 0}
                />
              </button>
            ))}
          </div>
          <small>PREFERRED INSTRUMENT</small>
          <div className="capture-instruments">
            {["Guitar", "Bass", "Synth", "Vocal", "Other"].map((label, index) => (
              <button key={label} className={index === 0 ? "is-active" : ""}>
                {label}
              </button>
            ))}
          </div>
        </main>
      </section>
    );
  if (view === "capture-result")
    return (
      <section className="qc-screen capture-official capture-official-result">
        <header>
          <span>Neural Capture</span>
          <button>×</button>
        </header>
        <p>
          {CAPTURE_RESULT_BODY[0]}
          <br />
          {CAPTURE_RESULT_BODY[1]}
        </p>
        <div className="capture-result-actions">
          <button>BACK TO CALIBRATION</button>
          <button>SAVE</button>
          <CaptureTargetIcon />
        </div>
        <main>
          <section>
            <button>CORTEX</button>
            <label>
              <span className="capture-level-label">
                <IoHeadphonesGlyph />
                LEVEL
              </span>
              <QcRotaryDial className="capture-level-dial" value={0.7672} />
              <small>0.0 dB</small>
            </label>
          </section>
          <section>
            <button>REFERENCE</button>
          </section>
        </main>
      </section>
    );
  return (
    <section className="qc-screen capture-official capture-official-settings">
      <header>
        <span>Neural Capture</span>
        <button>×</button>
      </header>
      <ul>
        <li>{CAPTURE_CALIBRATION_TIPS[0]}</li>
        <li>{CAPTURE_CALIBRATION_TIPS[1]}</li>
        <li>
          {CAPTURE_GROUND_LIFT_TIP[0]}
          <br />
          {CAPTURE_GROUND_LIFT_TIP[1]}
        </li>
      </ul>
      <div className="capture-setting-actions">
        <button>CONNECTION DIAGRAM</button>
        <button>START CAPTURE</button>
        <CaptureTargetIcon />
        <nav>
          <button className="is-active">1</button>
          <button>2</button>
        </nav>
      </div>
      <main>
        <section>
          <span>IN 1 LEVEL</span>
          <em className="capture-info">i</em>
          <small>INST</small>
          <b>0.0 dB</b>
          {/* capture-calibration puts both input levels at 183.7 degrees. */}
          <QcRotaryDial className="capture-level-dial" value={0.1804} />
        </section>
        <section>
          <span>IN 2 LEVEL</span>
          <em className="capture-info">i</em>
          <small>DEVICE</small>
          <b>0.0 dB</b>
          <QcRotaryDial className="capture-level-dial" value={0.1804} />
        </section>
        <section className="capture-input-type">
          <span>IN 1 TYPE</span>
          <label>
            <QcPillToggle index={1} />
            <small>Mic</small>
            <b>Instrument</b>
          </label>
        </section>
        <section className="capture-input-type">
          <span>IN 2 TYPE</span>
          <label>
            <QcPillToggle index={1} />
            <small>Mic</small>
            <b>Instrument</b>
          </label>
        </section>
        <section>
          <span>IN 1 LEVEL</span>
          <b>
            -40.0 <small>dB</small>
          </b>
          <i className="capture-meter" />
        </section>
        <section>
          <span>IN 2 LEVEL</span>
          <b>
            -40.0 <small>dB</small>
          </b>
          <i className="capture-meter" />
        </section>
        <section>
          <span className="capture-level-label"><IoHeadphonesGlyph />LEVEL</span>
          <QcRotaryDial className="capture-level-dial" value={0.7672} />
          <b>
            0.0 <small>dB</small>
          </b>
        </section>
      </main>
    </section>
  );
}

function CorOsCaptureFixture({ view }: { view: CaptureFixtureView }) {
  if (view === "capture-calibration" || view === "capture-progress" || view === "capture-sanity-error" || view === "capture-result" || view === "capture-save") return <CorOsOfficialCapture view={view} />;
  const steps: Array<[string, CaptureFixtureView]> = [
    ["TYPE", "capture-type"],
    ["CONNECTIONS", "capture-routing"],
    ["CALIBRATE", "capture-calibration"],
    ["CAPTURE", "capture-progress"],
    ["RESULT", "capture-result"],
    ["SAVE", "capture-save"],
  ];
  const current = Math.max(
    0,
    steps.findIndex(([, step]) => step === view),
  );
  return (
    <section className="qc-screen coros-capture-fixture" aria-label={view.replaceAll("-", " ")}>
      <header>
        <button>
          <QcUiIcon kind="previous" />
        </button>
        <strong>New Neural Capture</strong>
        <button>
          <QcUiIcon kind="close" />
        </button>
      </header>
      {view === "capture-intro" ? (
        <main className="capture-intro">
          <div className="capture-orbit">
            <i>
              <DeviceCategoryGlyph label="Neural Capture" />
            </i>
            <span />
            <span />
            <span />
          </div>
          <h1>Neural Capture</h1>
          <p>{CAPTURE_INTRO_BODY}</p>
          <aside>
            <b>1</b>
            <span>{CAPTURE_INTRO_STEPS[0]}</span>
            <b>2</b>
            <span>{CAPTURE_INTRO_STEPS[1]}</span>
            <b>3</b>
            <span>{CAPTURE_INTRO_STEPS[2]}</span>
          </aside>
          <button>GET STARTED</button>
        </main>
      ) : (
        <>
          <nav className="capture-steps">
            {steps.map(([label, step], index) => (
              <span key={step} className={index === current ? "is-active" : index < current ? "is-done" : ""}>
                <b>{index < current ? <QcUiIcon kind="check" /> : index + 1}</b>
                {label}
              </span>
            ))}
          </nav>
          <main className={`capture-workspace ${view}`}>
            {view === "capture-type" && (
              <>
                <h1>{CAPTURE_TYPE_TITLE}</h1>
                <p>{CAPTURE_TYPE_BODY}</p>
                <div className="capture-type-grid">
                  {CAPTURE_TYPE_CHOICES.map(([category, title, text], index) => (
                    <button key={title} className={index === 0 ? "is-active" : ""}>
                      <b>
                        <DeviceCategoryGlyph label={category} />
                      </b>
                      <strong>{title}</strong>
                      <small>{text}</small>
                    </button>
                  ))}
                </div>
              </>
            )}
            {view === "capture-routing" && (
              <>
                <h1>{CAPTURE_ROUTING_TITLE}</h1>
                <p>{CAPTURE_ROUTING_BODY}</p>
                <div className="capture-routing-map">
                  <span>
                    <b>QC SEND 1</b>
                    <i>OUT</i>
                  </span>
                  <em>
                    <QcUiIcon kind="arrow-right" />
                  </em>
                  <span className="capture-gear">
                    <b>AMPLIFIER</b>
                    <i>INPUT</i>
                  </span>
                  <em>
                    <QcUiIcon kind="arrow-right" />
                  </em>
                  <span className="capture-gear">
                    <b>CAB / LOAD</b>
                    <i>OUTPUT</i>
                  </span>
                  <em>
                    <QcUiIcon kind="arrow-right" />
                  </em>
                  <span>
                    <b>QC RETURN 1</b>
                    <i>IN</i>
                  </span>
                </div>
                <div className="capture-check">
                  <i>
                    <QcUiIcon kind="check" />
                  </i>
                  <span>
                    <strong>Connections complete</strong>
                    <small>{CAPTURE_ROUTING_LOAD_BOX_TIP}</small>
                  </span>
                </div>
              </>
            )}
            <button className="capture-next">NEXT</button>
          </main>
        </>
      )}
    </section>
  );
}

function SettingsAccountGlyph({ kind }: { kind: "cloud" | "user" | "backup" }) {
  return <QcScreenGlyph kind={kind === "cloud" ? "account-cloud" : kind} />;
}

// The Global Bypass pane's own prose names this mark: "Globally bypassed
// devices will have a bypass icon". That is `editor.bypass`, so the row marks
// and the inline one render the entry rather than a second stroked ring of
// their own.
function SettingsPowerIcon() {
  return <QcUiIcon kind="power-toggle" />;
}

function SettingsDeviceSectionIcon() {
  return <QcScreenGlyph kind="device" />;
}

/**
 * The Device pane's row marks, from the same table the reference panes use.
 *
 * This used to carry its own label ladder ending in `system-volume`, so every
 * row the ladder did not name - Hold Timing, Latency Compensation, MIDI - drew
 * the Master Volume knob instead of its own mark. One table, both panes, and
 * a row nothing names now fails to compile instead of falling back to a knob.
 */
function SettingsDeviceIcon({ label }: { label: string }) {
  const row = SETTINGS_DEVICE_ROWS.find(([, name]) => name === label);
  return <ReferenceSettingsIcon kind={row ? row[0] : "device"} />;
}

function SettingsSectionGlyph({ view, label }: { view: "settings-account" | "settings-system" | "settings-device" | "settings-midi"; label?: string }) {
  if (view === "settings-account") return <SettingsAccountGlyph kind={label === "Backups" ? "backup" : label ? "user" : "cloud"} />;
  if (view === "settings-system") {
    if (!label) return <QcUiIcon kind="settings" />;
    const kind = label === "Connection" ? "connection" : label === "Updates" ? "updates" : label === "Brightness" ? "brightness" : label === "Power Functions" ? "power" : label === "Master Volume Knob" ? "volume" : label === "Device Storage" ? "storage" : "factory-reset";
    return <QcSettingsIcon kind={kind} />;
  }
  if (view === "settings-midi" && label === "MIDI") return <QcUiIcon kind="menu-midi" />;
  return label ? <SettingsDeviceIcon label={label} /> : <QcScreenGlyph kind="device" />;
}

// The chevron that closes a search section: 18 wide, 10 deep, centred in the
// 44px band.
function SearchSectionChevron() {
  return (
    <svg className="search-section-chevron" viewBox="0 0 18 10" aria-hidden="true">
      <path d="M1 1 9 9 17 1" />
    </svg>
  );
}

function SettingsDeviceModelIcon({ kind }: { kind: string }) {
  return <DeviceCategoryGlyph label={kind === "ir" ? "IR Loader" : "Cab"} />;
}

function CorOsOfficialSettings({ view, onClose }: { view: "settings-account" | "settings-system" | "settings-device" | "settings-midi"; onClose: () => void }) {
  const data =
    view === "settings-midi"
      ? {
          title: "Device",
          active: 6,
          rows: ["Scene Bypass Behavior", "Stomp Mode Bypass", "Hold Timing", "Swap Tempo and Tuner", "Gig View Access", "Latency Compensation", "MIDI"],
        }
      : view === "settings-account"
        ? { title: "Account", active: 0, rows: ["My Account", "Backups"] }
        : view === "settings-system"
          ? {
              title: "System",
              active: 2,
              rows: ["Connection", "Updates", "Brightness", "Power Functions", "Master Volume Knob", "Device Storage", "Factory Reset"],
            }
          : {
              title: "Device",
              active: 0,
              rows: ["Global Bypass", "Scene Bypass Behavior", "Stomp Mode Bypass", "Hold Timing", "Swap Tempo and Tuner", "Gig View Access", "Latency Compensation"],
            };
  return (
    <section className={`qc-screen coros-settings-official ${view}`} aria-label={`${data.title} Settings`}>
      <header>
        <button className="settings-section">
          <b>
            <SettingsSectionGlyph view={view} />
          </b>
          {data.title}
          <i />
        </button>
        <button className="settings-done" onClick={onClose}>
          <QcUiIcon kind="check" />
        </button>
      </header>
      <main>
        <nav>
          {data.rows.map((label, index) => (
            <button key={label} className={index === data.active ? "is-active" : ""}>
              <b>
                <SettingsSectionGlyph view={view} label={label} />
              </b>
              {label}
            </button>
          ))}
        </nav>
        {view === "settings-account" ? (
          <section className="settings-account-detail is-my-account">
            <h1>Device linked to</h1>
            <p>{ACCOUNT_LINKED_BODY}</p>
            <button>UNLINK DEVICE</button>
          </section>
        ) : view === "settings-system" ? (
          <section className="settings-system-detail">
            <h1>Brightness</h1>
            <QcBrightnessInstructions lines={BRIGHTNESS_BODY} />
            {[
              ["Screen", "16", 16],
              ["LEDs", "32", 32],
              ["Dimmed LEDs", "2", 2],
            ].map(([label, value, bars]) => (
              <div key={String(label)}>
                <span>{label}</span>
                <strong>{value}</strong>
                <QcSegmentBar segments={32} filled={Number(bars)} label={`${label} brightness`} />
              </div>
            ))}
            <QcBrightnessModeRow />
          </section>
        ) : view === "settings-midi" ? (
          <section className="settings-midi-detail">
            <h1>MIDI Settings</h1>
            {[
              ["MIDI Channel", "select", "1"],
              ["MIDI Thru", "toggle", "Off"],
              ["MIDI Over USB", "toggle", "On"],
              ["Ignore Duplicate PC", "toggle", "Off"],
              ["MIDI Clock Out", "select", "OFF"],
              ["MIDI Clock In", "toggle", "Off"],
            ].map(([label, kind, value]) => (
              <div key={label}>
                <b>i</b>
                <span>{label}</span>
                {kind === "select" ? (
                  <button>
                    {value}
                    <i>
                      <QcUiIcon kind="down" />
                    </i>
                  </button>
                ) : (
                  <label>
                    <QcPillToggle index={value === "On" ? 0 : 1} />
                    <QcPillToggleLabels labels={["On", "Off"]} index={value === "On" ? 0 : 1} />
                  </label>
                )}
              </div>
            ))}
          </section>
        ) : (
          <section className="settings-device-detail">
            <h1>Global Bypass</h1>
            <p>
              {GLOBAL_BYPASS_BODY[0]}
              <br />
              {GLOBAL_BYPASS_BODY[1]}<br />
              {GLOBAL_BYPASS_BODY[2]}{" "}
              <span className="inline-settings-power">
                <SettingsPowerIcon />
              </span>{" "}
              {GLOBAL_BYPASS_BODY[3]}
            </p>
            <small>
              {GLOBAL_BYPASS_FOOTNOTE[0]}
              <br />
              {GLOBAL_BYPASS_FOOTNOTE[1]}
            </small>
            {["cab", "ir"].map((key) => (
              <div key={key}>
                <b>
                  <SettingsDeviceModelIcon kind={key} />
                </b>
                {[1, 2, 3, 4].map((row) => (
                  <label key={row}>
                    <span>ROW {row}</span>
                    <i>
                      <SettingsPowerIcon />
                    </i>
                  </label>
                ))}
              </div>
            ))}
          </section>
        )}
      </main>
    </section>
  );
}

export type { CorOsDeviceInformation, ReferenceSettingsView } from "./coros-reference-settings-pane";

type SettingsFixtureView = ReferenceSettingsView | "settings-account" | "settings-system" | "settings-device" | "settings-midi" | "settings-diagnostics-dsp" | "settings-diagnostics-footswitch" | "settings-diagnostics-usb";

function CorOsDiagnosticsDialog({ view, onClose }: { view: "settings-diagnostics-dsp" | "settings-diagnostics-footswitch" | "settings-diagnostics-usb"; onClose: () => void }) {
  const title = view === "settings-diagnostics-dsp" ? "DSP Diagnostics" : view === "settings-diagnostics-footswitch" ? "Footswitch Statistics" : "USB Statistics";
  return (
    <section className={`qc-screen coros-diagnostics-dialog ${view}`} aria-label={title}>
      <header>
        <h1>{title}</h1>
        <button className="settings-done" onClick={onClose}>
          <QcUiIcon kind="check" />
        </button>
      </header>
      {view === "settings-diagnostics-usb" && (
        <div className="usb-statistics">
          {USB_STATISTICS.map(([label, value, tone]) => (
            <span key={label}>
              <b>{label}</b>
              <i className={tone}>{value}</i>
            </span>
          ))}
        </div>
      )}
      {view === "settings-diagnostics-footswitch" && (
        <div className="footswitch-statistics">
          {FOOTSWITCH_STATISTICS.map(([name, press, hold]) => (
            <span key={name}>
              <b>{name}</b>
              <i>{press}</i>
              <em>{hold}</em>
            </span>
          ))}
        </div>
      )}
      {view === "settings-diagnostics-dsp" && (
        <div className="dsp-diagnostics">
          <header>
            {DSP_SOC_COUNTERS.map(([soc, count]) => (
              <span key={soc}>
                <b>{soc}</b>
                <i>{count}</i>
              </span>
            ))}
          </header>
          <div className="dsp-cores">
            {DSP_CORES.map((core) => (
              <span key={core}>
                <button>Off</button>
                <small>{core}</small>
              </span>
            ))}
          </div>
          <div className="dsp-table">
            {DSP_DIAGNOSTICS.map(([label, values]) => (
              <section key={label}>
                <b>{label}</b>
                {values.map((value, index) => (
                  <i key={index}>{value}</i>
                ))}
              </section>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}

function CorOsReferenceSettings({ view, deviceInformation, onClose }: { view: ReferenceSettingsView; deviceInformation?: CorOsDeviceInformation; onClose: () => void }) {
  const liveInformation = view === "settings-info" && deviceInformation !== undefined;
  const support = view === "settings-support" || view === "settings-info" || view === "settings-diagnostics" || view === "settings-support-report" || view === "settings-support-licenses";
  const account = view === "settings-account-backups";
  const deviceRows = SETTINGS_DEVICE_ROWS;
  // The category selector is drawn on some panes and not others. Every frame
  // that reaches these three shows the dialog without it, and the menu below
  // does not move when it goes, so only the header changes.
  const selector = !["settings-system-power", "settings-system-volume", "settings-update-idle", "settings-system-reset"].includes(view);
  const device = view.startsWith("settings-device-");
  const rows = account
    ? [
        ["user", "My Account"],
        ["backup", "Backups"],
      ]
    : view.startsWith("settings-device-")
      ? deviceRows
      : support
        ? [
            ["about", "About and Contact"],
            ["info", "Device Information"],
            ["report", "Send Report"],
            ["diagnostics", "Diagnostics"],
            ["licenses", "3rd Party Licenses"],
          ]
        : [
            ["wifi", "Connection"],
            ["updates", "Updates"],
            ["brightness", "Brightness"],
            ["power", "Power Functions"],
            ["volume", "Master Volume Knob"],
            ["storage", "Device Storage"],
            ["factory", "Factory Reset"],
          ];
  const active =
    (
      {
        "settings-support": 0,
        "settings-info": 1,
        "settings-diagnostics": 3,
        "settings-wifi": 0,
        "settings-update-idle": 1,
        "settings-system-power": 3,
        "settings-system-volume": 4,
        "settings-storage": 5,
        "settings-system-reset": 6,
        "settings-device-scene-bypass": 1,
        "settings-device-stomp-bypass": 2,
        "settings-device-hold-timing": 3,
        "settings-device-swap-tempo-tuner": 4,
        "settings-device-gig-view": 5,
        "settings-device-latency": 6,
        "settings-support-report": 2,
        "settings-support-licenses": 4,
        "settings-account-backups": 1,
      } as Record<string, number>
    )[view] ?? 5;
  return (
    <section className={`qc-screen coros-settings-official coros-settings-reference ${view}`} aria-label={view.replaceAll("-", " ")}>
      <header>
        {selector && (
          <button className="settings-section" disabled={liveInformation} title={liveInformation ? "Additional Support pages are not exposed by the current USB gateway." : undefined}>
            <b>
              <ReferenceSettingsIcon kind={device ? "device" : account ? "cloud" : support ? "support" : "system"} />
            </b>
            {device ? "Device" : account ? "Account" : support ? "Support" : "System"}
            <i />
          </button>
        )}
        {account && (
          <button className="backups-refresh" aria-label="Refresh backups">
            <QcUiIcon kind="refresh" />
          </button>
        )}
        {view === "settings-info" && (
          <button className="settings-edit" aria-label="Edit device name" disabled={liveInformation} title={liveInformation ? "Rename the device from the app Settings window." : undefined}>
            <QcUiIcon kind="edit" />
          </button>
        )}
        <button className="settings-done" onClick={onClose}>
          <QcUiIcon kind="check" />
        </button>
      </header>
      <main>
        <nav>
          {rows.map(([icon, label], index) => (
            <button key={label} className={index === active ? "is-active" : ""} disabled={liveInformation && index !== active}>
              <b>
                <ReferenceSettingsIcon kind={icon} />
              </b>
              {label}
            </button>
          ))}
        </nav>
        <section className="reference-settings-detail">
          <CorOsReferenceSettingsPane view={view} deviceInformation={deviceInformation} />
        </section>
      </main>
    </section>
  );
}

function CorOsSettingsFixture({ view, deviceInformation, onClose }: { view: SettingsFixtureView; deviceInformation?: CorOsDeviceInformation; onClose: () => void }) {
  if (view === "settings-account" || view === "settings-system" || view === "settings-device" || view === "settings-midi") return <CorOsOfficialSettings view={view} onClose={onClose} />;
  // The restart confirmation is CorOS's `ConfirmationMessage` drawn over the
  // pane that raised it: a title, a message, and two buttons whose captions the
  // message itself carries.
  if (view === "settings-diagnostics-dsp" || view === "settings-diagnostics-footswitch" || view === "settings-diagnostics-usb") return <CorOsDiagnosticsDialog view={view} onClose={onClose} />;
  if (view === "settings-restart-confirm")
    return (
      <div className="coros-restart-confirm">
        <CorOsReferenceSettings view="settings-system-power" onClose={onClose} />
        <div className="restart-scrim" />
        <aside className="restart-dialog">
          <h1>Restart device</h1>
          <p>{RESTART_DEVICE_BODY}</p>
          <footer>
            <button>CANCEL</button>
            <button className="is-primary">RESTART</button>
          </footer>
        </aside>
      </div>
    );
  return <CorOsReferenceSettings view={view} deviceInformation={deviceInformation} onClose={onClose} />;
}

type IoView = "overview" | "input" | "output" | "send-return" | "usb" | "headphones";

/*
 * The rear panel, socket by socket. Every I/O frame draws all twelve of these
 * groups in the same boxes; the page only changes which socket is lit, so each
 * socket carries a name and the lighting is a lookup rather than an index test.
 *
 * `label` captions the upper socket and `sub` the lower, which is why a group
 * with one socket carries the caption the unit puts under or over it.
 */
const IO_PORTS: Array<{
  id: IoView;
  label: string;
  sub: string;
  kind?: "square" | "midi" | "input" | "combo";
  paired?: boolean;
  /** Upper socket, then lower where the group stacks two. */
  sockets: [string] | [string, string];
}> = [
  { id: "usb", label: "", sub: "USB", kind: "square", sockets: ["usb"] },
  { id: "send-return", label: "EXP 2", sub: "EXP 1", paired: true, sockets: ["exp2", "exp1"] },
  { id: "send-return", label: "", sub: "MIDI OUT", kind: "midi", sockets: ["midi-out"] },
  { id: "send-return", label: "", sub: "MIDI IN", kind: "midi", sockets: ["midi-in"] },
  { id: "output", label: "", sub: "OUT 2/R", kind: "combo", sockets: ["out2"] },
  { id: "output", label: "", sub: "OUT 1/L", kind: "combo", sockets: ["out1"] },
  { id: "headphones", label: "", sub: "CAPTURE OUT", paired: true, sockets: ["phones", "capture-out"] },
  { id: "output", label: "OUT 4/R", sub: "OUT 3/L", paired: true, sockets: ["out4", "out3"] },
  { id: "send-return", label: "RET 2", sub: "RET 1", paired: true, sockets: ["ret2", "ret1"] },
  { id: "send-return", label: "SEND 2", sub: "SEND 1", paired: true, sockets: ["send2", "send1"] },
  { id: "input", label: "INPUT 2", sub: "", kind: "input", sockets: ["input2"] },
  { id: "input", label: "INPUT 1", sub: "", kind: "input", sockets: ["input1"] },
];

/*
 * Which socket the unit lights on each page - not derived from the page's
 * name. io-headphones lights the upper socket of the headphone pair and
 * leaves CAPTURE OUT unlit, and io-send-return lights only the lower socket
 * of the send pair - the unit never lights a stacked pair except on
 * io-overview, where both OUT 3/L and OUT 4/R are lit.
 *
 * The USB port is the only socket the unit ever draws white, and it draws it
 * white on all five frames that are not io-usb.
 */
const IO_ACTIVE_SOCKETS: Record<IoView, readonly string[]> = {
  usb: ["usb"],
  input: ["input1"],
  output: ["out1", "out2"],
  overview: ["out3", "out4"],
  "send-return": ["send1"],
  headphones: ["phones"],
};

/** io.usb for the square plate, io.jack for a bare TRS nut, else the kind. */
function ioJackKind(kind?: string): QcIoJackKind {
  return kind === "square" ? "usb" : kind === "midi" ? "midi" : kind === "combo" ? "combo" : kind === "input" ? "input" : "jack";
}

function ioJackState(socket: string, view: IoView): QcIoJackState {
  if (IO_ACTIVE_SOCKETS[view].includes(socket)) return "active";
  return socket === "usb" ? "plugged" : "idle";
}

/* 0.7672 is where every "0.0 dB" level knob sits on the unit, so it is the
   default here rather than a per-screen constant. */
function IoDial({ value, dial = 0.7672, steps, accent }: { value: string; dial?: number; steps?: number; accent?: string }) {
  return (
    <span className="io-dial-wrap">
      <QcRotaryDial className={`io-dial${steps ? " is-stepped" : ""}`} value={dial} mode={steps ? "step" : "arc"} steps={steps} accent={accent} />
      <strong>
        <IoReading text={value} />
      </strong>
    </span>
  );
}

function IoHeadphonesGlyph({ active = false }: { active?: boolean }) {
  return <QcIoIcon className={active ? "is-active" : "is-muted"} kind="headphones-symbol" />;
}

function CorOsIoSettings({ initialView, onClose }: { initialView: IoView; onClose: () => void }) {
  // The overview page is the second output pair: the header reads Output 3/4,
  // OUT 4/R and OUT 3/L carry the green rings, and the strips drop GROUND LIFT.
  const [view, setView] = useState<IoView>(initialView);
  const [globalEqOpen, setGlobalEqOpen] = useState(false);
  if (globalEqOpen) return <CorOsGlobalEq onClose={() => setGlobalEqOpen(false)} />;
  const outputLike = view === "output" || view === "overview" || view === "send-return";
  const title = view === "input" ? "Input 1" : view === "output" ? "Output 1/2" : view === "overview" ? "Output 3/4" : view === "send-return" ? "Send 1" : view === "usb" ? "USB" : "Headphones";
  const meters = view === "usb" ? ["IN 1/2", "IN 3/4", "IN 5/6", "IN 7/8", "OUT 1/2", "OUT 3/4", "OUT 5/6", "OUT 7/8"] : [];
  return (
    <section className={`coros-io-settings is-${view}`} aria-label={`I/O Settings ${title}`}>
      <header>
        <span className="io-heading">
          <i aria-hidden="true">
            <QcScreenVectorGlyph icon="io.header" />
          </i>
          <span>
            <small>I/O SETTINGS</small>
            <strong>{title}</strong>
          </span>
        </span>
        <button className="io-global-eq" onClick={() => setGlobalEqOpen(true)}>
          GLOBAL EQ
        </button>
        <button aria-label="Close I/O Settings" onClick={onClose}>
          <QcUiIcon kind="check" />
        </button>
      </header>
      <div className="io-ports">
        {IO_PORTS.map((port, index) => {
          // Each reference page lights its own socket, not its whole housing.
          const states = port.sockets.map((socket) => ioJackState(socket, view));
          const lit = states.some((state) => state === "active");
          // A stacked pair whose other socket is lit keeps its own caption
          // muted: io-send-return draws SEND 1 light and SEND 2 grey off one
          // housing, and io-headphones does the same with CAPTURE OUT.
          const muted = (position: number) => (lit && states[position] !== "active" ? "is-muted" : undefined);
          return (
            <button key={`${port.label}-${index}`} className={`${lit ? "is-active" : ""} is-${port.kind ?? "jack"}${port.paired ? " is-paired" : ""}`} onClick={() => setView(port.id)}>
              <span className={[port.id === "headphones" ? "io-headphone-label" : undefined, muted(0)].filter(Boolean).join(" ") || undefined}>{port.id === "headphones" ? <IoHeadphonesGlyph active={view === "headphones"} /> : port.label}</span>
              {port.sockets.map((socket, position) => (
                <i key={socket}>
                  <QcIoJack kind={ioJackKind(port.kind)} state={states[position]} />
                </i>
              ))}
              <small className={muted(port.sockets.length - 1)}>{port.sub}</small>
            </button>
          );
        })}
        <QcScreenVectorGlyph icon="io.linked" className="io-port-link io-port-link-output12" />
        <QcScreenVectorGlyph icon="io.linked" className="io-port-link io-port-link-output34" />
        {view === "usb" && (
          <div className="io-input-selectors">
            <button>1</button>
            <button>2</button>
          </div>
        )}
      </div>
      {view === "usb" ? (
        <div className="io-editor is-usb">
          <section>
            <span>USB LEVEL</span>
            <IoDial value="0.0 dB" dial={1} />
          </section>
          <section>
            <span>HP SOURCE</span>
            {/* io-usb marks four detents at 45, 135, 225 and 315 degrees and
                lights the one at 45 - the last of the four. */}
            <IoDial value="BOTH" dial={1} steps={4} />
          </section>
          <div className="io-meter-grid">
            {meters.map((meter) => (
              <span key={meter}>
                <b>{meter}</b>
                <i>
                  <QcScreenGlyph kind="info" />
                </i>
                <small>
                  <span>-40.0 dB</span>
                  <span>0.00</span>
                </small>
                <QcLevelMeter />
                <QcLevelMeter />
              </span>
            ))}
          </div>
        </div>
      ) : view === "headphones" ? (
        <div className="io-editor is-headphones">
          <section>
            <span>HP LEVEL</span>
            <IoDial value="0.0 dB" />
          </section>
          <section>
            <span>MULTI OUT</span>
            <IoDial value="0.0 dB" />
          </section>
          <div className="io-headphone-meter">
            <span>LEVEL</span>
            <small>-40.0 dB　　　0.00</small>
            <QcLevelMeter />
            <QcLevelMeter />
          </div>
          <div className="io-headphone-meter">
            <span>MULTI OUT</span>
            <small>-40.0 dB　　　0.00</small>
            <QcLevelMeter />
            <QcLevelMeter />
          </div>
        </div>
      ) : outputLike ? (
        <div className="io-editor is-output">
          {(view === "overview" ? [3, 4] : view === "output" ? [1, 2] : [1]).map((channel) => (
            <section key={channel}>
              <span>
                <em>
                  {view === "send-return" ? "SEND" : "OUT"} {channel} LEVEL
                </em>
                <IoDial value={view === "send-return" ? "0.0" : "0.0 dB"} />
              </span>
              {view === "output" && (
                <span className="io-switch">
                  <em>GROUND LIFT</em>
                  <QcPillToggle index={1} />
                  <label>On</label>
                  <label>Off</label>
                </span>
              )}
              <span className="io-readout">
                <em>{view === "send-return" ? "SEND 1 LEVEL" : "OUT LEVEL"}</em>
                <strong><IoReading text="-40.0 dB" /></strong>
                <QcLevelMeter />
              </span>
              <span className="io-readout">
                <em>LIMITER</em>
                <strong><IoReading text="0.0 dB" /></strong>
                <QcLevelMeter />
              </span>
              {view === "overview" && <span />}
              {view !== "send-return" && (
                <span className="io-switch">
                  <em>MUTE</em>
                  <QcPillToggle index={1} />
                  <label>On</label>
                  <label>Off</label>
                </span>
              )}
            </section>
          ))}
          {view === "send-return" && <section aria-hidden="true" />}
        </div>
      ) : (
        <div className="io-editor is-analog">
          <section>
            <span>{view === "input" ? "IN 1 LEVEL" : view === "send-return" ? "RETURN 1 LEVEL" : "HP LEVEL"}</span>
            <IoDial value="0.0 dB" dial={view === "input" ? 0.1804 : 0.7672} />
          </section>
          {view === "input" && (
            <>
              <section>
                <span>IMPEDANCE</span>
                {/* io-input marks nine detents, not four, spanning the unit's
                    full 270 degree sweep. The lit one is the last, index 8,
                    and the readout beside it is 10M - the ninth and last of
                    nine impedances for one control. */}
                <IoDial value="10M Ω" dial={1} steps={9} />
              </section>
              <section className="io-switch">
                <span>TYPE</span>
                <QcPillToggle index={1} />
                <label>Mic</label>
                <label>Instrument</label>
              </section>
              <section className="io-switch is-disabled">
                <span>PHANTOM 48V</span>
                <QcPillToggle index={1} />
                <label>On</label>
                <label>Off</label>
              </section>
            </>
          )}
          <section className="io-switch">
            <span>GROUND LIFT</span>
            <QcPillToggle index={1} />
            <label>On</label>
            <label>Off</label>
          </section>
          {view !== "input" && (
            <section className="io-switch">
              <span>MUTE</span>
              <QcPillToggle index={1} />
              <label>On</label>
              <label>Off</label>
            </section>
          )}
          <div className="io-level-meter">
            <span>{view === "input" ? "IN 1 LEVEL" : `${title.toUpperCase()} LEVEL`}</span>
            <strong>-40.0 dB</strong>
            <QcLevelMeter />
          </div>
        </div>
      )}
    </section>
  );
}

type GlobalEqView = "global-eq" | "global-eq-on" | "global-eq-enabled" | "global-eq-high-pass" | "global-eq-filter-type";

/*
 * The five Global EQ states, and why one `enabled` flag could not carry them.
 *
 * `enabled` used to switch the power pill, the band's filter type, its Q and
 * the response curve as one bundle, because the only on state the
 * reconstruction had was `global-eq-on`. The device draws three more distinct
 * states that disagree with that bundle: global-eq-enabled is on with band 1
 * still Lo Shelf over a flat response, global-eq-high-pass is on with band 1
 * at Hi Pass over the unit's own skirt - node 1 alone drops and 2..5 stay on
 * the flat line - and global-eq-filter-type is global-eq-enabled behind the
 * filter-type selector table. None of the three could be asked for, so
 * `?screen=` fell through the dispatcher and drew nothing at all.
 *
 * The node x positions repeat across every one of the device's own graphs:
 * 104, 210, 448, 607, 671. The y drawn is 60 more than the y here, because
 * the graph's own box starts at screen y 60 - so the flat line at 185 is 125,
 * and the dropped hi-pass node at 216 is 156.
 */
const GLOBAL_EQ_FLAT_NODES: ReadonlyArray<readonly [number, number]> = [[104, 125], [210, 125], [448, 125], [607, 125], [671, 125]];
const GLOBAL_EQ_HIGH_PASS_NODES: ReadonlyArray<readonly [number, number]> = [[104, 156], [210, 125], [448, 125], [607, 125], [671, 125]];
const GLOBAL_EQ_MANUAL_NODES: ReadonlyArray<readonly [number, number]> = [[104, 156], [244, 104], [511, 145], [607, 125], [645, 104]];
const GLOBAL_EQ_FLAT_RESPONSE = "M32 125H800";
// The opening skirt is the manual set's, for the same filter at the same
// 50 Hz and the same Q 0.10; only the tail differs, because this state's
// bands 2..5 are all flat where the manual's are not.
const GLOBAL_EQ_HIGH_PASS_RESPONSE = "M26 250C56 206 73 169 104 156C143 130 177 125 210 125H800";
const GLOBAL_EQ_MANUAL_RESPONSE = "M26 250C56 206 73 169 104 156C143 119 194 102 244 104C340 108 412 127 476 124C494 124 500 146 511 145C523 143 524 129 541 125C566 120 584 125 607 125C622 118 632 108 645 104C701 95 759 82 800 80";

const GLOBAL_EQ_STATE: Record<GlobalEqView, {
  enabled: boolean;
  filter: "lo-shelf" | "high-pass";
  q: string;
  nodes: ReadonlyArray<readonly [number, number]>;
  response: string;
  selector: boolean;
}> = {
  "global-eq": { enabled: false, filter: "lo-shelf", q: "0.71", nodes: GLOBAL_EQ_FLAT_NODES, response: GLOBAL_EQ_FLAT_RESPONSE, selector: false },
  "global-eq-enabled": { enabled: true, filter: "lo-shelf", q: "0.71", nodes: GLOBAL_EQ_FLAT_NODES, response: GLOBAL_EQ_FLAT_RESPONSE, selector: false },
  "global-eq-filter-type": { enabled: true, filter: "lo-shelf", q: "0.71", nodes: GLOBAL_EQ_FLAT_NODES, response: GLOBAL_EQ_FLAT_RESPONSE, selector: true },
  "global-eq-high-pass": { enabled: true, filter: "high-pass", q: "0.10", nodes: GLOBAL_EQ_HIGH_PASS_NODES, response: GLOBAL_EQ_HIGH_PASS_RESPONSE, selector: false },
  "global-eq-on": { enabled: true, filter: "high-pass", q: "0.10", nodes: GLOBAL_EQ_MANUAL_NODES, response: GLOBAL_EQ_MANUAL_RESPONSE, selector: false },
};

/*
 * The band's filter-type table, which only global-eq-filter-type opens.
 *
 * All five marks are shared registry contours at these exact 24px cells.
 * Keep the device's own inconsistent label casing here; the closed trigger
 * upper-cases independently.
 */
const EQ_FILTER_TYPE_MARKS: ReadonlyArray<"peak" | "high-pass" | "lo-pass" | "hi-shelf" | "lo-shelf"> = ["peak", "high-pass", "lo-pass", "hi-shelf", "lo-shelf"];

function EqFilterTypeSelector({ selected }: { selected: number }) {
  return (
    <div className="eq-filter-type-selector" role="menu" aria-label="Band 1 filter type">
      {EQ_FILTER_TYPE_LABELS.map((label, index) => (
        <button key={label} role="menuitem" className={index === selected ? "is-selected" : ""} aria-checked={index === selected}>
          <i><QcEqIcon kind={EQ_FILTER_TYPE_MARKS[index]} /></i>
          <span>{label}</span>
        </button>
      ))}
    </div>
  );
}

function CorOsGlobalEq({ onClose, view = "global-eq" }: { onClose: () => void; view?: GlobalEqView }) {
  const verticals = [46, 79, 105, 125, 143, 158, 172, 184, 263, 343, 369, 389, 407, 422, 436, 448, 527, 574, 607, 633, 653, 671, 686, 700, 712, 791].map((pixel) => pixel / 8);
  const { enabled, filter, q, nodes, response: responsePath, selector } = GLOBAL_EQ_STATE[view];
  const filterIndex = filter === "high-pass" ? 1 : 4;
  return (
    <section className={`coros-global-eq${enabled ? "" : " is-off"}${selector ? " is-filter-type-open" : ""}`} aria-label="Global EQ">
      <header>
        {/* The unit shows the model-preset stack in every state, as the live screen does. */}
        <button className="global-eq-more"><QcPresetStackIcon /></button>
        <span>
          <small>GLOBAL EQ</small>
          <strong>{MODEL_PARAMETRIC_5}</strong>
        </span>
        <button className={`global-eq-power${enabled ? "" : " is-off"}`}>
          <i /> {enabled ? "ON" : "OFF"}
        </button>
        <button aria-label="Close Global EQ" onClick={onClose}>
          <QcEditorIcon kind="confirm" />
        </button>
      </header>
      <div className="global-eq-graph">
        <div>
          {verticals.map((left) => (
            <i key={left} style={{ left: `${left}%` }} />
          ))}
        </div>
        {/* The box is 252px tall (52.5% of 480), so a 255-tall viewBox under
            preserveAspectRatio="none" scaled every y by 252/255 and put the
            whole graph 1.5px high: the flat response, authored as 125 for
            "screen 185 minus the graph's own top at 60", landed at 183.5 and
            painted rows 182..184 grey instead of 184..185 white. Matching the
            viewBox to the box makes that stated mapping - screen 185 is 125,
            the dropped hi-pass node at screen 216 is 156 - true rather than
            approximate. Strokes are non-scaling, so only positions move. */}
        <svg viewBox="0 0 800 252" preserveAspectRatio="none">
          <g className="eq-axis-labels">
            <text x="208" y="13">
              100
            </text>
            <text x="471" y="13">
              1k
            </text>
            <text x="736" y="13">
              10k
            </text>
          </g>
          <path d={responsePath} />
          <g>
            {nodes.map(([x, y], index) => (
              <g key={index}>
                <circle cx={x} cy={y} r="18" className={index === 0 ? "is-active" : ""} />
                <text x={x} y={y + 5}>
                  {index + 1}
                </text>
              </g>
            ))}
          </g>
        </svg>
      </div>
      <div className="global-eq-tabs">
        {[1, 2, 3, 4, 5].map((tab) => (
          <button key={tab} className={tab === 1 ? "is-active" : ""}>
            {tab}
          </button>
        ))}
        <button>OUT</button>
      </div>
      <div className="global-eq-controls">
        <section>
          <span>TYPE</span>
          <button>
            <QcEqIcon kind={filter} /> {EQ_FILTER_TYPE_LABELS[filterIndex].toUpperCase()} <QcUiIcon kind="down" />
          </button>
        </section>
        {/* GAIN's 0.0 dB is mid-sweep here, not the 0.7672 a level knob puts
            it at, and FREQ and Q sit near the bottom. */}
        {([
          ["GAIN", "0.0 dB", 0.5],
          ["FREQ", "50 Hz", 0.1565],
          ["Q", q, 0.0779],
        ] as Array<[string, string, number]>).map(([label, value, dial]) => (
          // HI PASS: the unit greys GAIN and Q (measured on global-eq-high-pass).
          <section key={label} className={filter === "high-pass" && label !== "FREQ" ? "is-disabled" : undefined}>
            <span>{label}</span>
            {/* global-eq here is switched off, and CorOS reflects that in the
                band controls rather than across the screen: the header, graph
                and control grounds keep the unit's ordinary values to the pixel,
                while the three dial fills read (8,44,74) and the graph's band
                marker (8,40,74) against the (8,117,231) an on EQ carries. */}
            <IoDial value={value} dial={dial} accent={enabled ? QC_COLORS.browserCategory.equalizer : "var(--qc-palette-082c4a)"} />
          </section>
        ))}
        <section className="global-eq-bypass">
          <span>BYPASS 1</span>
          <button>
            <ExpressionPowerIcon />
          </button>
        </section>
      </div>
      {selector && <i className="eq-filter-type-scrim" />}
      {selector && <EqFilterTypeSelector selected={filterIndex} />}
    </section>
  );
}

function CorOsPowerOverlay({ onClose }: { onClose: () => void }) {
  return (
    <section className="coros-power-overlay" aria-label="Power and Locking Functions">
      <div className="power-actions">
        <button className="power-lock">
          <QcScreenGlyph kind="lock" />
          <span>Lock Touchscreen and Master Volume knob</span>
        </button>
        <div>
          <button onClick={onClose}>CANCEL</button>
          <button>SHUT DOWN</button>
          <button>REBOOT</button>
          <button>BE RIGHT BACK</button>
        </div>
      </div>
    </section>
  );
}

function InformationGlyph() {
  return <QcScreenGlyph kind="info" />;
}

function CorOsModesConfiguration({ onClose }: { onClose: () => void }) {
  const modes = ["PRESET", "SCENE", "STOMP"] as const;
  return (
    <section className="coros-modes-configuration" aria-label="Modes Configuration">
      <ModesCycleTrack />
      <header>
        <span>Modes configuration</span>
        <div>
          <button aria-label="Modes Configuration information">
            <InformationGlyph />
          </button>
          <button aria-label="Close Modes Configuration" onClick={onClose}>
            <QcUiIcon kind="check" />
          </button>
        </div>
      </header>
      <p>
        {MODES_CONFIGURATION_BODY[0]}
        <br />
        {MODES_CONFIGURATION_BODY[1]}
      </p>
      <div className="modes-cycle">
        <span>CYCLE</span>
        <div>
          {modes.map((mode) => (
            <button key={mode}>
              <b>
                <ModeGlyph mode={mode} />
              </b>
              {mode[0] + mode.slice(1).toLowerCase()}
            </button>
          ))}
        </div>
      </div>
      <ModesResetToDefaults />
    </section>
  );
}

function CorOsOfficialModes({ onClose }: { onClose: () => void }) {
  return (
    <section className="coros-modes-configuration modes-official" aria-label="Modes Configuration">
      <header>
        <span>Modes configuration</span>
        <div>
          <button aria-label="Modes Configuration information">
            <InformationGlyph />
          </button>
          <button aria-label="Close Modes Configuration" onClick={onClose}>
            <QcUiIcon kind="check" />
          </button>
        </div>
      </header>
      <p>
        {MODES_CONFIGURATION_BODY[0]}
        <br />
        {MODES_CONFIGURATION_BODY[1]}
      </p>
      <div className="modes-cycle">
        <span>CYCLE</span>
        <i />
        <div>
          <button>
            <b>
              <ModeGlyph mode="PRESET" />
            </b>
            Preset
          </button>
          <button className="hybrid-mode">
            <b>
              <ModeGlyph mode="SCENE" />
              <ModeGlyph mode="STOMP" />
            </b>
            <span>
              Scene
              <br />
              Stomp
            </span>
            <em>
              <i />
              <QcUiIcon kind="down" />
              <i />
            </em>
          </button>
        </div>
      </div>
      <svg className="modes-device" viewBox="0 0 240 152" aria-hidden="true">
        <path d="M7 12 Q5 76 7 140 Q7 150 18 150 H222 Q233 150 233 140 Q235 76 233 12 Q233 2 222 2 H18 Q7 2 7 12 Z" />
        <rect x="67" y="10" width="106" height="62" rx="3" />
        <circle cx="38" cy="42" r="11" />
        <circle cx="203" cy="42" r="7" />
        {[38, 79, 120, 162, 203].map((x) => (
          <circle key={`a${x}`} cx={x} cy="98" r="6" className={x === 203 ? "off" : "yellow"} />
        ))}
        {[38, 79, 120, 162, 203].map((x) => (
          <circle key={`b${x}`} cx={x} cy="132" r="6" className={x === 203 ? "off" : "purple"} />
        ))}
      </svg>
    </section>
  );
}

function CorOsSaveAs({ onClose }: { onClose: () => void }) {
  // Both columns are the unit's own: the setlists it lists down the left, and
  // bank 2 of My Presets down the right, with 2F the slot being saved into
  // and 2H the one empty slot left.
  const folders = ["My Presets", "Set Live", "Set Rec", "Set AcousticLive", "Downloaded", "Downloaded2"];
  const presets = ["2A Demo Clean", "2B Demo Chorus", "2C Demo Crunch", "2D Demo Ambient", "2E Demo Lead", "2F Unsaved", "2G Demo Rhythm", "2H Unsaved"];
  return (
    <section className="coros-save-as" aria-label="Save As destination">
      <header>
        <button className="save-as-type">
          <b>
            <DirectoryIcon kind="grid" />
          </b>
          Presets{" "}
          <i>
            <QcUiIcon kind="down" />
          </i>
        </button>
        <span>Save to...</span>
        <button className="save-as-close" aria-label="Close Save As" onClick={onClose}>
          <QcUiIcon kind="close" />
        </button>
      </header>
      <div className="save-as-content">
        <nav>
          {folders.map((folder, index) => (
            <button key={folder} className={index === 0 ? "is-active" : ""}>
              <b>
                <DirectoryIcon kind="folder" number={index + 1} />
              </b>
              <span>{folder}</span>
              <i>
                <QcUiIcon kind="more" />
              </i>
            </button>
          ))}
          <button className="save-as-new">
            <b>
              <DirectoryIcon kind="new-folder" />
            </b>
            <span>New Setlist</span>
          </button>
        </nav>
        <aside>
          {Array.from({ length: 14 }, (_, index) => (
            <button key={index} className={index === 1 ? "is-active" : ""}>
              {index + 1}
            </button>
          ))}
        </aside>
        <section>
          {presets.map((preset, index) => (
            <button key={preset} className={index === 5 ? "is-active" : index === 7 ? "is-disabled" : ""}>
              {preset}
              <i>
                <QcUiIcon kind="more" />
              </i>
            </button>
          ))}
        </section>
      </div>
    </section>
  );
}

function CorOsPresetNameEditor({ snapshot, onClose }: { snapshot: PresetSnapshot; onClose: () => void }) {
  const rows = [
    ["Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P"],
    ["A", "S", "D", "F", "G", "H", "J", "K", "L"],
    ["Shift", "Z", "X", "C", "V", "B", "N", "M", "Backspace"],
  ];
  return (
    <section className="coros-name-editor" aria-label="Preset name editor">
      <header>
        <button className="name-editor-close" aria-label="Close preset name editor" onClick={onClose}>
          <QcUiIcon kind="close" />
        </button>
        <span className="name-editor-divider" />
        <button className="name-editor-folder">
          <QcDirectoryIcon kind="folder" />
          <span>My Pre... / {snapshot.presetLocation}</span>
        </button>
        <button className="name-editor-tab is-active">Name</button>
        <button className="name-editor-metadata" aria-label="Preset metadata">
          <QcUiIcon kind="document" />
        </button>
        <button className="name-editor-next" aria-label="Continue">
          <QcUiIcon kind="arrow-right" />
        </button>
      </header>
      <div className="name-editor-value">
        <span>{snapshot.presetName}</span>
      </div>
      <div className="name-editor-keyboard">
        {rows.map((row, rowIndex) => (
          <div className={`name-key-row row-${rowIndex + 1}`} key={rowIndex}>
            {row.map((key, index) => (
              <button key={`${key}-${index}`} className={key === "Shift" || key === "Backspace" ? "is-wide" : ""}>
                {key === "Shift" ? <QcScreenGlyph kind="shift" /> : key === "Backspace" ? <QcUiIcon kind="backspace" /> : key}
                <small>{rowIndex === 0 ? (index + 1) % 10 : rowIndex === 1 && index > 6 ? (index === 7 ? "(" : ")") : ""}</small>
              </button>
            ))}
          </div>
        ))}
        <div className="name-key-row row-4">
          <button>123</button>
          <button>,</button>
          <button className="name-key-space">Space</button>
          <button>.</button>
          <button>Next</button>
        </div>
      </div>
    </section>
  );
}

function RoutingToken({ kind, x, y, selected = false }: { kind: "S" | "M"; x: number; y: number; selected?: boolean }) {
  const color = kind === "S" ? "var(--qc-splitter-accent)" : "var(--qc-palette-e73e60)";
  return (
    <g className={selected ? "routing-token is-selected" : "routing-token"}>
      {selected && <circle cx={x} cy={y} r="20" fill="none" stroke="var(--qc-palette-f5f5f5)" strokeWidth="2" />}
      <circle cx={x} cy={y} r="15" fill="var(--qc-screen-background)" stroke="var(--qc-palette-202020)" strokeWidth="2" />
      <circle cx={x} cy={y} r="12" fill={color} />
      <text x={x} y={y + 5} textAnchor="middle" fill="var(--qc-route-text)" fontFamily={QC_TYPOGRAPHY.devicePlain} fontWeight="700" fontSize="15">
        {kind}
      </text>
    </g>
  );
}

function RoutingGridDiagram({ placement = false, selected }: { placement?: boolean; selected?: "S" | "M" }) {
  const cells = [112, 198, 284, 370, 456, 542, 628, 714];
  const blocks = [
    [112, "Filter", "var(--qc-palette-eee)"],
    [198, "Overdrive", "var(--qc-palette-ff7900)"],
    [284, "Amp", "var(--qc-palette-ff424c)"],
    [370, "Cab", "var(--qc-palette-7257ff)"],
    [542, "Utility", "var(--qc-palette-02d2bc)"],
    [628, "Reverb", "var(--qc-palette-35b9ff)"],
  ] as const;
  return (
    <svg className="routing-grid-diagram" viewBox="0 0 800 250" preserveAspectRatio="none" aria-label={placement ? "Empty Grid slots available for Splitter or Mixer placement" : "Parallel Grid signal paths"}>
      <rect width="800" height="250" fill="var(--qc-palette-030303)" />
      <g fill="none" stroke="var(--qc-palette-8e8f91)" strokeWidth="1.5">
        <path d="M52 92H748" />
        <path d="M284 92C284 118 284 143 284 168H628C628 143 628 118 628 92" />
      </g>
      <g fill="var(--qc-screen-surface-base)" stroke="var(--qc-screen-background)" strokeWidth="2">
        <rect x="8" y="52" width="44" height="80" rx="15" />
        <rect x="748" y="52" width="44" height="80" rx="15" />
      </g>
      <g fill="var(--qc-palette-eee)" fontFamily={QC_TYPOGRAPHY.deviceRoute} fontSize="14" textAnchor="middle">
        <text x="30" y="86">
          In
        </text>
        <text x="30" y="106">
          1
        </text>
        <text x="770" y="84">
          Multi
        </text>
        <text x="770" y="105">
          Out
        </text>
      </g>
      {blocks.map(([x, label, color]) => (
        <g key={x}>
          <rect x={x - 31} y="61" width="62" height="62" rx="14" fill="var(--qc-screen-background)" stroke={color} strokeWidth="3" />
          <DeviceCategoryGlyph label={label} x={x - 20} y={72} width={40} height={40} />
        </g>
      ))}
      {!placement && (
        <>
          <RoutingToken kind="S" x={284} y={92} selected={selected === "S"} />
          <RoutingToken kind="M" x={628} y={92} selected={selected === "M"} />
        </>
      )}
      {placement &&
        cells.map((x, index) => (
          <g key={x} className={index === 3 ? "placement-target is-active" : "placement-target"} transform={`translate(${x} 145)`}>
            <path d="M-17 35v-24c0-5 7-6 8-1V-1c0-6 8-6 8 0v8-15c0-6 8-6 8 0V7-4c0-6 8-6 8 0v14c4-5 10-2 9 4l-2 17c-1 10-9 16-19 16H0c-7 0-12-4-17-12Z" fill="var(--qc-palette-3df269)" stroke="var(--qc-palette-0d8e38)" strokeWidth="2" />
            <circle cx="24" cy="31" r="13" fill="var(--qc-route-text)" stroke="var(--qc-palette-168a40)" strokeWidth="2" />
            <path d="M24 23v9l6 3" fill="none" stroke="var(--qc-palette-168a40)" strokeWidth="2" strokeLinecap="round" />
          </g>
        ))}
    </svg>
  );
}

function ChoiceToken({ kind }: { kind: "S" | "M" }) {
  return (
    <svg viewBox="0 0 48 48" aria-hidden="true">
      <RoutingToken kind={kind} x={24} y={24} />
    </svg>
  );
}

function PhysicalRoutingGrid({ snapshot, selected }: { snapshot: PresetSnapshot; selected: "S" | "M" }) {
  const mixerSelected = selected === "M";
  return (
    <svg className="splitter-grid" viewBox="0 0 800 196" aria-hidden="true">
      <rect width="800" height="196" fill="var(--qc-screen-background)" />
      {[3, 97].flatMap((y, row) => [
        <g key={`l${row}`}>
          <rect x="8" y={y} width="44" height="77" rx="14" fill="var(--qc-screen-surface-base)" />
          {row === 0 ? (
            <text x="30" y={y + 32} textAnchor="middle" fill="var(--qc-palette-eee)" fontFamily={QC_TYPOGRAPHY.deviceRoute} fontSize="14">
              <tspan x="30">In</tspan>
              <tspan x="30" dy="20">
                1
              </tspan>
            </text>
          ) : (
            <QcScreenVectorGlyph icon="interface.add" x={18} y={y + 27} />
          )}
        </g>,
        <g key={`r${row}`}>
          <rect x="750" y={y} width="44" height="77" rx="14" fill="var(--qc-screen-surface-base)" />
          {row === 0 ? (
            <text x="772" y={y + 30} textAnchor="middle" fill="var(--qc-palette-eee)" fontFamily={QC_TYPOGRAPHY.deviceRoute} fontSize="14">
              <tspan x="772">Multi</tspan>
              <tspan x="772" dy="20">
                Out
              </tspan>
            </text>
          ) : (
            <QcScreenVectorGlyph icon="interface.add" x={760} y={y + 27} />
          )}
        </g>,
      ])}
      <path d="M60 41H740" stroke="var(--qc-palette-bfc1c0)" strokeWidth="2" />
      {[60, 142, 228, 314, 400, 486, 572, 658, 740].map((x) => (
        <circle key={x} cx={x} cy="41" r="5" fill="var(--qc-screen-surface-base)" stroke="var(--qc-white-led)" strokeWidth="2" />
      ))}
      {snapshot.blocks
        .filter((block) => block.row === 0)
        .map((block) => {
          const x = [101, 187, 273, 357, 443, 529, 615, 701][block.column] ?? 101;
          return (
            <g key={block.id}>
              <QcDeviceGlyph referenceOnly block={block} x={x} y={41} />
            </g>
          );
        })}
      <circle cx="60" cy="135" r="18" fill={mixerSelected ? "var(--qc-screen-surface-base)" : "var(--qc-splitter-accent)"} stroke={mixerSelected ? "var(--qc-splitter-accent)" : "none"} strokeWidth="2" />
      <text x="60" y="141" textAnchor="middle" fill={mixerSelected ? "var(--qc-splitter-accent)" : "var(--qc-route-text)"} fontFamily={QC_TYPOGRAPHY.devicePlain} fontSize="18">
        S
      </text>
      <circle cx="740" cy="135" r="18" fill={mixerSelected ? "var(--qc-mixer-accent)" : "var(--qc-screen-surface-base)"} stroke={mixerSelected ? "none" : "var(--qc-mixer-accent)"} strokeWidth="2" />
      <text x="740" y="141" textAnchor="middle" fill={mixerSelected ? "var(--qc-route-text)" : "var(--qc-mixer-accent)"} fontFamily={QC_TYPOGRAPHY.devicePlain} fontSize="18">
        M
      </text>
    </svg>
  );
}

function RoutingEditorHeaderControls() {
  return (
    <>
      <svg viewBox="0 0 230 44" aria-hidden="true">
        <circle cx="18" cy="12" r="6" fill="var(--qc-splitter-accent)" />
        <path d="M18 18v12h87v-12M105 30v-12" fill="none" stroke="var(--qc-palette-eee)" strokeWidth="2" />
        <circle cx="105" cy="30" r="6" fill="var(--qc-mixer-accent)" />
        <circle cx="140" cy="12" r="6" fill="var(--qc-splitter-accent)" />
        <path d="M140 18v12h87v-12M227 30v-12" fill="none" stroke="var(--qc-palette-eee)" strokeWidth="2" />
        <circle cx="227" cy="12" r="6" fill="var(--qc-mixer-accent)" />
      </svg>
      <b className="splitter-scene">
        <span>
          <QcEditorIcon kind="scene-previous" />
        </span>
        <i>A</i>
        <span>
          <QcEditorIcon kind="scene-next" />
        </span>
      </b>
      <em>
        <svg viewBox="0 0 66 43" aria-hidden="true">
          <rect x="23" y="11" width="20" height="20" rx="3" />
          <path d="M27 27V15l6 7 6-7v12" />
        </svg>
      </em>
    </>
  );
}

function CorOsRoutingScreen({ view, snapshot }: { view: "splitter-placement" | "splitter-editor" | "mixer-editor" | "empty-slot"; snapshot: PresetSnapshot }) {
  if (view === "empty-slot")
    return (
      <section className="qc-screen empty-slot-official" aria-label="Empty-slot device browser">
        <nav className="empty-slot-categories">
          {COROS_DEVICE_CATEGORIES.slice(0, 6).map(([label, color]) => (
            <button key={label} style={{ "--device-color": color } as CSSProperties}>
              <i>
                <DeviceCategoryGlyph label={label} />
              </i>
              <span>{label}</span>
            </button>
          ))}
        </nav>
        <section className="empty-slot-grid">
          <header>
            <strong>
              <span>4</span>E
            </strong>
            <h1>QC MCP TEST_2</h1>
            <span className="empty-undo">
              <GridToolbarIcon kind="undo" />
            </span>
            <b>A</b>
            <span className="empty-save">
              <GridToolbarIcon kind="save" />
            </span>
            <span className="empty-more">
              <GridToolbarIcon kind="more" />
            </span>
          </header>
          <div className="empty-mode">
            <GridToolbarIcon kind="mode" />
            <strong>PRESET</strong>
          </div>
          <main>
            <span className="empty-route">
              In
              <br />1
            </span>
            <span className="empty-route is-row-3">
              In
              <br />1
            </span>
            <i className="empty-cable" />
            <i className="empty-cable is-row-3" />
            <i className="empty-add">
              <QcUiIcon kind="add" />
            </i>
            <i className="empty-row is-row-2">
              <QcUiIcon kind="add" />
            </i>
            <i className="empty-row is-row-4">
              <QcUiIcon kind="add" />
            </i>
            <i className="empty-out">
              Multi
              <br />
              Out
            </i>
          </main>
        </section>
      </section>
    );
  const placement = view === "splitter-placement";
  const splitter = view === "splitter-editor" || placement;
  if (splitter)
    return (
      <section className="qc-screen coros-splitter-physical" aria-label={placement ? "Splitter and Mixer placement handles" : "Splitter parameter editor"}>
        <PhysicalRoutingGrid snapshot={snapshot} selected="S" />
        <section className="splitter-panel">
          <header>
            <button>
              <QcUiIcon kind="more" />
            </button>
            <span>
              <strong>Splitter</strong>
            </span>
            <RoutingEditorHeaderControls />
            <button>
              <QcUiIcon kind="check" />
            </button>
          </header>
          <div className="splitter-controls">
            <label>
              <strong>TYPE</strong>
              <QcPillToggle positions={3} index={2} />
              <span className="splitter-toggle">
                <b>
                  Crossover
                  <br />
                  A/B
                  <br />
                  <em>Balance</em>
                </b>
              </span>
            </label>
            <label>
              <strong>STEREO</strong>
              <QcPillToggle index={1} />
              <span className="splitter-toggle">
                <b>
                  Split
                  <br />
                  <em>Normal</em>
                </b>
              </span>
            </label>
            <label>
              <strong>BALANCE</strong>
              <QcRotaryDial className="splitter-knob" value={0.4999} accent={QC_COLORS.reference.splitterAccent} />
              <small>5.0</small>
            </label>
            <label>
              <strong>LEVEL TO A</strong>
              <QcRotaryDial className="splitter-knob angle-a" value={0.7672} accent={QC_COLORS.reference.splitterAccent} />
              <small>
                0.0 <em>dB</em>
              </small>
            </label>
            <label>
              <strong>LEVEL TO B</strong>
              <QcRotaryDial className="splitter-knob angle-b" value={0.7672} accent={QC_COLORS.reference.splitterAccent} />
              <small>
                0.0 <em>dB</em>
              </small>
            </label>
            <label>
              <strong>FREQUENCY</strong>
              <QcRotaryDial className="splitter-knob frequency" value={0.4962} accent={QC_COLORS.reference.splitterAccent} />
              <small>
                400 <em>Hz</em>
              </small>
            </label>
            <label>
              <strong>MODE</strong>
              <QcPillToggle index={1} />
              <span className="splitter-toggle">
                <b>
                  Invert
                  <br />
                  <em>Regular</em>
                </b>
              </span>
            </label>
            <span className="splitter-empty" />
            <span className="splitter-empty" />
            <span className="splitter-empty" />
          </div>
        </section>
      </section>
    );
  return (
    <section className="qc-screen coros-splitter-physical coros-mixer-physical" aria-label="Mixer parameter editor">
      <PhysicalRoutingGrid snapshot={snapshot} selected="M" />
      <section className="splitter-panel mixer-panel">
        <header>
          <button>
            <QcUiIcon kind="more" />
          </button>
          <span>
            <strong>Mixer</strong>
          </span>
          <RoutingEditorHeaderControls />
          <button>
            <QcUiIcon kind="check" />
          </button>
        </header>
        <div className="splitter-controls mixer-controls">
          <label>
            <strong>LEVEL A</strong>
            <QcRotaryDial className="splitter-knob mixer-knob level-a" value={0.7673} accent={QC_COLORS.reference.mixerAccent} />
            <small>
              0.0 <em>dB</em>
            </small>
          </label>
          <label>
            <strong>PAN A</strong>
            <QcRotaryDial className="splitter-knob mixer-knob pan" value={0.5001} accent={QC_COLORS.reference.mixerAccent} />
            <small>C</small>
          </label>
          <label>
            <strong>LEVEL B</strong>
            <QcRotaryDial className="splitter-knob mixer-knob level-b" value={0.7673} accent={QC_COLORS.reference.mixerAccent} />
            <small>
              0.0 <em>dB</em>
            </small>
          </label>
          <label>
            <strong>PAN B</strong>
            <QcRotaryDial className="splitter-knob mixer-knob pan" value={0.5001} accent={QC_COLORS.reference.mixerAccent} />
            <small>C</small>
          </label>
          <label>
            <strong>PHASE</strong>
            <QcPillToggle index={1} />
            <span className="splitter-toggle mixer-toggle">
              <b>
                On
                <br />
                <em>Off</em>
              </b>
            </span>
          </label>
          <label>
            <strong>MIXER LEVEL</strong>
            <QcRotaryDial className="splitter-knob mixer-knob level-a" value={0.7673} accent={QC_COLORS.reference.mixerAccent} />
            <small>
              0.0 <em>dB</em>
            </small>
          </label>
          <span className="splitter-empty" />
          <span className="splitter-empty" />
          <span className="splitter-empty" />
          <span className="splitter-empty" />
        </div>
      </section>
    </section>
  );
}

function pluginLicenseDisplayName(name: string) {
  const separator = name.indexOf(": ");
  return separator < 0 ? name : `${name.slice(0, separator + 2)}${name.slice(separator + 2).replaceAll(" ", "\u00a0")}`;
}

const PLUGIN_GRID_VISUAL_KEY = {
  gate: "utility",
  amp: "amp",
  capture: "capture",
  cab: "cab",
  wave: "modulation",
  cube: "utility",
  dual: "utility",
} as const;

function PluginGridGlyph({ kind }: { kind: keyof typeof PLUGIN_GRID_VISUAL_KEY }) {
  return <DeviceCategoryGlyph label={OFFICIAL_BLOCK_CATEGORY_BY_KEY[PLUGIN_GRID_VISUAL_KEY[kind]].label} />;
}

function GridToolbarIcon({ kind }: { kind: "undo" | "export" | "save" | "more" | "mode" | "refresh" }) {
  if (kind === "undo") return <QcNavigationBarIcon kind="nav-undo" />;
  if (kind === "export" || kind === "save") return <QcNavigationBarIcon kind={kind === "save" ? "nav-save" : "export"} />;
  if (kind === "more") return <QcNavigationBarIcon kind="menu" />;
  return <QcUiIcon kind={kind === "mode" ? "modes" : "refresh"} />;
}

function CorOsOfficialDeviceBrowser({ plugins }: { plugins: boolean }) {
  const rows = plugins ? PLUGIN_BROWSER_ROWS : AMP_MODELS_FIRST_PAGE;
  return (
    <section className={`qc-screen device-browser-official${plugins ? " is-plugins" : ""}`}>
      <main>
        <nav>
          {COROS_DEVICE_CATEGORIES.slice(0, 6).map(([label], index) => (
            <button key={label} className={index === (plugins ? 0 : 1) ? "is-active" : ""}>
              <i>
                <DeviceCategoryGlyph label={label} />
              </i>
            </button>
          ))}
        </nav>
        <section className="device-browser-list">
          <header>
            {plugins ? (
              <>
                <b>Add device</b>
                <i>
                  <QcUiIcon kind="refresh" />
                </i>
              </>
            ) : (
              <>
                <button className="is-active">GUITAR</button>
                <button>BASS</button>
              </>
            )}
          </header>
          {rows.map((name, index) => (
            <button key={name}>
              {plugins ? <b className={index === 4 ? "is-placeholder" : undefined} aria-hidden="true" /> : null}
              {name}
            </button>
          ))}
        </section>
      </main>
      <section className="device-browser-grid">
        <header>
          {plugins ? (
            <>
              <em>3</em>
              <span className="plugin-undo">
                <GridToolbarIcon kind="undo" />
              </span>
              <b>A</b>
              <span className="plugin-save">
                <GridToolbarIcon kind="export" />
              </span>
              <span className="plugin-more">
                <GridToolbarIcon kind="more" />
              </span>
              <strong className="plugin-mode">
                <span className="plugin-mode-matrix">
                  <i>A</i>
                  <i>B</i>
                  <i>C</i>
                  <i>D</i>
                </span>
                <span>SCENE</span>
              </strong>
            </>
          ) : (
            <>
              <span className="amp-undo">
                <GridToolbarIcon kind="undo" />
              </span>
              <b>A</b>
              <span className="amp-save">
                <GridToolbarIcon kind="save" />
              </span>
              <span className="amp-more">
                <QcUiIcon kind="more" />
              </span>
              <i className="amp-status" />
            </>
          )}
        </header>
        {!plugins && (
          <div className="amp-mode">
            <QcModeGlyph mode="PRESET" />
            <span>PRESET</span>
          </div>
        )}
        <main>
          {plugins && (
            <>
              {(["gate", "amp", "capture", "cab", "wave", "cube", "dual"] as const).map((kind, index) => (
                <span key={kind} className={`grid-block b${index + 1}`}>
                  <PluginGridGlyph kind={kind} />
                </span>
              ))}
            </>
          )}
          {plugins ? (
            <>
              <i>
                <QcUiIcon kind="add" />
              </i>
              <i>
                Row
                <br />3
              </i>
              <i>
                <QcUiIcon kind="add" />
              </i>
              <i>
                Multi
                <br />
                Out
              </i>
              <i>
                <QcUiIcon kind="add" />
              </i>
            </>
          ) : (
            <>
              <i>
                <QcUiIcon kind="add" />
              </i>
              <i>
                Multi
                <br />
                Out
              </i>
              <i>
                <QcUiIcon kind="add" />
              </i>
              <i>
                <QcUiIcon kind="add" />
              </i>
              <i>
                <QcUiIcon kind="add" />
              </i>
            </>
          )}
        </main>
      </section>
    </section>
  );
}

function PluginModelGlyph({ name, kind }: { name: string; kind: "amp" | "cab" | "drive" }) {
  const category = kind === "cab" ? "CAB" : kind === "drive" ? "OVERDRIVE" : "AMP";
  const blockKind = kind === "drive" ? "utility" : kind;
  const block: GridBlock = {
    id: `plugin-${name}`,
    name,
    kind: blockKind,
    category,
    row: 0,
    column: 0,
    bypassed: false,
  };
  // Same 70px item box as the grid and gig tiles: the row's own 70-square holds
  // a 64 tile plus its 3px margin, which the unit paints black over the lit row
  // - device-browser-plugin-models has the border at 421..484 inside a black
  // 418..487. Drawing the tile at 70 also fed the 40-unit category glyph a
  // 43.75 box, and crispEdges drops rows at 1.09 scale.
  return (
    <svg viewBox="0 0 70 70" aria-hidden="true">
      <QcDeviceGlyph block={block} x={35} y={35} size={64} />
    </svg>
  );
}

// `device-favorites` used to be in this union with a branch of its own - the
// FAVORITES/RECENT tab strip and five starred rows - and the dispatcher has
// always routed that name to CorOsCaptureLibrary, which is the surface both
// device-favorites and device-recents render from.
// Two components claiming one screen name is how the branch went unnoticed; the
// name belongs to one of them, and this is not it.
function CorOsDeviceBrowserFixture({ view }: { view: "device-search" | "plugin-folders" | "plugin-list" | "plugin-list-ready" | "plugin-list-reference" | "plugin-list-unsaved" | "plugin-models" | "plugin-locked" | "plugin-refresh" | "plugin-refresh-ready" }) {
  if (view === "plugin-folders")
    return (
      <section className="qc-screen plugin-folders-official" aria-label="Plugin folders">
        <header>
          <button>
            <QcUiIcon kind="previous" />
          </button>
          <button className="plugin-folder-category">
            <DeviceCategoryGlyph label="Plugins" />
            <span>Plugins</span>
            <small>
              <QcUiIcon kind="down" />
            </small>
          </button>
          <span />
          <button>
            <DirectoryIcon kind="arrange" />
          </button>
          <button>
            <DirectoryIcon kind="search" />
          </button>
          <button>
            <DirectoryIcon kind="done" />
          </button>
        </header>
        <main>
          <nav>
            {/* Every row of this nav is the same folder outline. */}
            <button>
              <b>
                <DirectoryIcon kind="folder-outline" />
              </b>
              <span>
                <small>Plugins</small>{PLUGIN_PARALLAX_X}
              </span>
            </button>
            {["Artists", PLUGIN_FOLDER_NEURAL_DSP, "User"].map((label) => (
              <button key={label}>
                <b>
                  <DirectoryIcon kind="folder-outline" />
                </b>
                <span>{label}</span>
                {label === "User" && (
                  <i>
                    <QcUiIcon kind="more" />
                  </i>
                )}
              </button>
            ))}
          </nav>
          <section>
            <button>
              <span>
                Default<small>Bass</small>
              </span>
              <i>D</i>
              <b>
                <GridToolbarIcon kind="export" />
              </b>
            </button>
          </section>
        </main>
      </section>
    );
  const plugins = view.startsWith("plugin-");
  // The same browser draws over three different presets. The Grid
  // behind it is part of the state, not decoration.
  const pluginBrowser = view.startsWith("plugin-list") || view.startsWith("plugin-refresh");
  const refreshing = view === "plugin-refresh" || view === "plugin-refresh-ready";
  const underlay = view === "plugin-list-reference" ? "reference" : view === "plugin-list-ready" || view === "plugin-refresh-ready" ? "ready" : view === "plugin-list-unsaved" ? "unsaved" : "add";
  const pluginModels = view === "plugin-models" || view === "plugin-locked";
  const lockedPlugin = view === "plugin-locked";
  const pluginModelRows: ReadonlyArray<readonly [string, "amp" | "cab" | "drive"]> = lockedPlugin
    ? LOCKED_PLUGIN_MODEL_ROWS
    : PLUGIN_MODEL_ROWS;
  // The category is annotated rather than inferred: the list's colours are the
  // panel's rendering rule, and the one reachable state's three rows are all
  // overdrives, so inference narrows the type until every other colour test
  // reads as a comparison that cannot hold.
  const rows: ReadonlyArray<readonly [string, "AMP" | "CAB" | "MODULATION" | "REVERB" | "OVERDRIVE", boolean]> | undefined =
    view === "device-search"
      ? DEVICE_SEARCH_ROWS
      : undefined;
  return (
    <section className={`qc-screen coros-browser-fixture${pluginBrowser ? ` is-physical-plugin-list is-underlay-${underlay}` : pluginModels ? ` is-physical-plugin-models${lockedPlugin ? " is-physical-plugin-locked" : ""}` : ""}`} aria-label={view.replaceAll("-", " ")}>
      <div className="browser-fixture-panel">
        <nav>
          {COROS_DEVICE_CATEGORIES.slice(0, 8).map(([label, color], index) => (
            <button key={label} className={(plugins ? index === 0 : index === 4) ? "is-active" : ""} style={{ "--device-color": color } as CSSProperties}>
              <i>
                <DeviceCategoryGlyph label={label} />
              </i>
            </button>
          ))}
        </nav>
        <main>
          <header>
            <strong>{refreshing ? PLUGIN_REFRESH_NOTICE : view.startsWith("plugin-list") ? "Add device" : plugins ? "Plugins" : "Search devices"}</strong>
            <button className={refreshing ? "is-refreshing" : ""}>
              <GridToolbarIcon kind="refresh" />
            </button>
          </header>
          {view === "device-search" && (
            <div className="browser-search">
              <span>⌕</span>
              <b>{DEVICE_SEARCH_QUERY_CHIEF}</b>
              <button>×</button>
            </div>
          )}
          {pluginModels ? (
            <div className="browser-result-list is-plugin-licenses">
              {PLUGIN_LICENSES.map(([name, available]) => {
                const selected = name === (lockedPlugin ? PLUGIN_ARCHETYPE_CORY_WONG : PLUGIN_ARCHETYPE_PLINI);
                return (
                  <button key={name} className={`${available ? "is-licensed" : "is-locked"}${selected ? " is-selected" : ""}`}>
                    {!available && (
                      <i className="plugin-license-lock">
                        <PluginLockIcon />
                      </i>
                    )}
                    <span>
                      <strong>{name}</strong>
                    </span>
                    {selected && (
                      <em className="plugin-model-next">
                        <QcUiIcon kind="down" />
                      </em>
                    )}
                  </button>
                );
              })}
            </div>
          ) : pluginBrowser ? (
            <div className="browser-result-list is-plugin-licenses">
              {PLUGIN_LICENSES.map(([name, available]) => (
                <button key={name} className={available ? "is-licensed" : "is-locked"}>
                  {!available && (
                    <i className="plugin-license-lock">
                      <PluginLockIcon />
                    </i>
                  )}
                  <span>
                    <strong>{name}</strong>
                  </span>
                </button>
              ))}
            </div>
          ) : (
            <div className="browser-result-list">
              {rows?.map(([name, category]) => (
                <button key={name}>
                  <i
                    style={
                      {
                        "--result-color": category === "AMP" ? "var(--qc-palette-ff424c)" : category === "CAB" ? "var(--qc-palette-7257ff)" : category === "MODULATION" ? "var(--qc-palette-a95cff)" : category === "REVERB" ? "var(--qc-palette-35b9ff)" : "var(--qc-palette-ff7900)",
                      } as CSSProperties
                    }
                  >
                    {category === "AMP" ? "▭" : category === "CAB" ? "⊙" : "∿"}
                  </i>
                  <span>
                    <strong>{name}</strong>
                    <small>{category}</small>
                  </span>
                  <b>★</b>
                </button>
              ))}
            </div>
          )}
        </main>
        {pluginModels && (
          <>
            <header className="plugin-model-header">
              <strong>{lockedPlugin ? PLUGIN_LICENSE_NOT_FOUND : "Add device"}</strong>
              <button>
                <GridToolbarIcon kind="refresh" />
              </button>
              <button>
                <QcUiIcon kind="close" />
              </button>
            </header>
            <section className="plugin-model-list">
              {pluginModelRows.map(([name, kind]) => (
                <button key={name}>
                  <i className={kind === "cab" ? "is-cab" : kind === "drive" ? "is-drive" : ""}>
                    <PluginModelGlyph name={name} kind={kind} />
                  </i>
                  {lockedPlugin && (
                    <i className="plugin-model-lock">
                      <PluginLockIcon />
                    </i>
                  )}
                  <span>{name}</span>
                  <em>
                    <DevicePresetGlyph />
                  </em>
                </button>
              ))}
            </section>
          </>
        )}
      </div>
      {pluginBrowser ? (
        <div className={`plugin-grid-underlay${underlay === "reference" ? " is-reference-grid" : ""}`}>
          <header>
            <strong>
              {underlay === "add" || underlay === "reference" ? "4" : underlay === "ready" ? "2" : "32"}
              <span>{underlay === "add" || underlay === "reference" ? "E" : underlay === "ready" ? "F" : "G"}</span>
            </strong>
            <em>{underlay === "add" || underlay === "reference" ? "QC MCP TEST_2" : underlay === "ready" ? "QC MCP TEST" : "Unsaved"}</em>
          </header>
          <main>
            <i className="underlay-input">
              In
              <br />1
            </i>
            {underlay === "unsaved" ? (
              <i className="underlay-plus">
                <QcUiIcon kind="add" />
              </i>
            ) : (
              <i className="underlay-device underlay-gate">
                <DeviceCategoryGlyph label="Utility" />
              </i>
            )}
            {underlay === "reference" && (
              <i className="underlay-plus">
                <QcUiIcon kind="add" />
              </i>
            )}
            {(underlay === "add" || underlay === "ready") && <QcGridAddTile className="underlay-add" />}
            <i className="underlay-row-2">
              <QcUiIcon kind="add" />
            </i>
            <i className="underlay-row-3">
              <QcUiIcon kind="add" />
            </i>
            <i className="underlay-row-4">
              <QcUiIcon kind="add" />
            </i>
          </main>
        </div>
      ) : (
        <div className="browser-grid-ghost">
          <b>3B</b>
          <span>SCENE</span>
          <i>
            <QcUiIcon kind="add" />
          </i>
          <i>
            <QcUiIcon kind="add" />
          </i>
          <i>
            <QcUiIcon kind="add" />
          </i>
        </div>
      )}
    </section>
  );
}

function CorOsLooperEditor() {
  const actions: Array<[string, QcLooperActionGlyphName, string]> = [
    ["DUPLICATE", "duplicate", "A"],
    ["ONE SHOT", "one-shot", "B"],
    ["HALF SPEED", "half-speed", "C"],
    ["PUNCH IN", "punch-in", "D"],
    ["RECORD", "record", "E"],
    ["PLAY", "play", "F"],
    ["REVERSE", "reverse", "G"],
    ["UNDO", "undo", "H"],
  ];
  return (
    <section className="qc-screen coros-looper" aria-label="Looper X editor">
      <header>
        <button aria-label="Open Looper menu">
          <QcUiIcon kind="more" />
        </button>
        <span>
          <small>LOOPER</small>
          <strong>
            {LOOPER_X_TITLE}<QcPresetStackIcon />
          </strong>
        </span>
        <i />
        <button className="looper-params">
          <QcEditorIcon kind="looper" />
          Params
        </button>
        <button className="looper-scene">
          <QcEditorIcon kind="scene-previous" />
          <b>A</b>
          <QcEditorIcon kind="scene-next" />
        </button>
        <button aria-label="Confirm">
          <QcEditorIcon kind="confirm" />
        </button>
      </header>
      <div className="looper-timeline">
        <span>
          {LOOPER_HINT_USE}{" "}
          <b>
            <QcLooperActionGlyph kind="record" />
          </b>{" "}
          {LOOPER_HINT_START_RECORDING}
        </span>
        <span>
          {LOOPER_HINT_USE}{" "}
          <b className="looper-close-caret">
            <QcUiIcon kind="close-caret" />
          </b>{" "}
          {LOOPER_HINT_CLOSE_VIEW}
        </span>
        <em>AVAILABLE 4:43</em>
      </div>
      <div className="looper-actions">
        {actions.map(([label, glyph, key]) => (
          <button key={label}>
            <small>{label}</small>
            <strong>
              <QcLooperActionGlyph kind={glyph} />
            </strong>
            <b>{key}</b>
          </button>
        ))}
      </div>
    </section>
  );
}

// `device-search` is not one of these. It is the device browser's own search
// state, and listing it here was what let the dispatcher route it away from
// CorOsDeviceBrowserFixture.
type CaptureLibraryView = "device-favorites" | "device-recents" | "device-browser-neural-capture" | "device-search-entry" | "device-search-suggestions" | "device-search-results";

function CaptureLibraryRail() {
  return (
    <nav className="capture-library-rail">
      {COROS_DEVICE_CATEGORIES.slice(0, 6).map(([label], index) => (
        <button key={label} className={index === 2 ? "is-active" : ""}>
          <i>
            <DeviceCategoryGlyph label={label} />
          </i>
        </button>
      ))}
    </nav>
  );
}

function CaptureLibraryKeyboard({ query = "" }: { query?: string }) {
  const rows = [
    ["q", "w", "e", "r", "t", "y", "u", "i", "o", "p"],
    ["a", "s", "d", "f", "g", "h", "j", "k", "l"],
    ["Shift", "z", "x", "c", "v", "b", "n", "m", "Backspace"],
    ["123", ",", "Space", ".", query ? "Search" : "Done"],
  ];
  const shifted = ["", "", "", "", "", "", "", "(", ")"];
  return (
    <section className="capture-search-keyboard">
      <header>
        <button aria-label="Close search">
          <QcUiIcon kind="close" />
        </button>
        <button className={query ? "is-ready" : ""} aria-label="Run search">
          <QcDirectoryIcon kind="search" />
        </button>
      </header>
      <h1 className={query ? "is-query" : ""}>
        <span>{query || "Search for ..."}</span>
        <i />
      </h1>
      <small>{query ? "Suggestions" : "Recently searched"}</small>
      {query ? (
        <div className="capture-search-suggestion">
          <i>
            <QcLibraryIcon kind="capture-header" />
          </i>{" "}
          {CLOUD_CAPTURE_SEARCH_RESULT}
        </div>
      ) : (
        <div className="capture-recent-searches">
          <button>Clear all</button>
        </div>
      )}
      <main>
        {rows.map((row, rowIndex) => (
          <div key={rowIndex}>
            {row.map((key, keyIndex) => (
              <button key={key} className={key === "Space" ? "is-space" : key === "Shift" ? "is-shift" : key === "Backspace" ? "is-backspace" : key === "Search" || key === "Done" ? "is-done" : key === "123" ? "is-numeric" : ""}>
                {rowIndex < 2 && <small>{rowIndex === 0 ? (keyIndex + 1) % 10 : shifted[keyIndex]}</small>}
                {key === "Backspace" ? <QcUiIcon kind="backspace" /> : key === "Shift" ? <QcScreenGlyph kind="shift" /> : key}
              </button>
            ))}
          </div>
        ))}
      </main>
    </section>
  );
}

function CorOsCaptureLibrary({ view }: { view: CaptureLibraryView }) {
  if (view === "device-search-entry") return <CaptureLibraryKeyboard />;
  if (view === "device-search-suggestions") return <CaptureLibraryKeyboard query="gary" />;
  if (view === "device-search-results")
    return (
      <section className="qc-screen capture-search-results">
        <header>
          <button aria-label="Search">
            <QcDirectoryIcon kind="search" />
          </button>
          <i />
          <button>
            <QcDirectoryIcon kind="grid" /> (0)
          </button>
          <button className="is-active">
            <QcLibraryIcon kind="capture-header" /> (1)
          </button>
          <button>
            <SettingsDeviceModelIcon kind="ir" /> (0)
          </button>
          <i />
          <button aria-label="Filter">
            <QcDirectoryIcon kind="filter" />
          </button>
          <button aria-label="Arrange">
            <QcDirectoryIcon kind="arrange" />
          </button>
          <i />
          <button aria-label="Done">
            <QcDirectoryIcon kind="done" />
          </button>
        </header>
        <main>
          <h2>
            DEVICE DIRECTORIES{" "}
            <b>
              <SearchSectionChevron />
            </b>
          </h2>
          <article>
            <strong>{CLOUD_CAPTURE_SEARCH_RESULT}</strong>
            <small>{CLOUD_CAPTURE_AUTHOR}</small>
            <em>J</em>
          </article>
          <h2>
            DOWNLOADS{" "}
            <b>
              <SearchSectionChevron />
            </b>
          </h2>
          <p>No results</p>
        </main>
      </section>
    );
  const recent = view === "device-recents";
  const library = view === "device-browser-neural-capture";
  const captures = FACTORY_CAPTURE_ROWS;
  return (
    <section className={`qc-screen capture-library-browser${recent ? " is-recent" : library ? "" : " is-favorites"}`}>
      <CaptureLibraryRail />
      <header>
        <span>Add device</span>
        {!recent && (
          <button aria-label="Filter">
            <QcDirectoryIcon kind="filter" />
          </button>
        )}
        {!recent && (
          <button aria-label="Arrange">
            <QcDirectoryIcon kind="arrange" />
          </button>
        )}
        <button aria-label="Search">
          <QcDirectoryIcon kind="search" />
        </button>
        <button aria-label="Close">
          <QcUiIcon kind="close" />
        </button>
      </header>
      <aside>
        <button className={!recent && !library ? "is-active" : ""}>
          <b>
            <QcLibraryIcon kind="heart" />
          </b>
          <span>Favorites</span>
        </button>
        <button className={recent ? "is-active" : ""}>
          <b>
            <QcLibraryIcon kind="clock" />
          </b>
          <span>Recent</span>
        </button>
        <button>
          <b>
            <QcDirectoryIcon kind="download" />
          </b>
          <span>Downloads</span>
        </button>
        <button className={library ? "is-active" : ""}>
          <b>
            <QcLibraryIcon kind="capture-library" />
          </b>
          <span>Captures Library</span>
          <small>2127</small>
        </button>
        <i />
        {/* The child rows carry the outline folder, not the solid
            silhouette, on the rows below the rule. */}
        {["Factory Captures V1", "Factory Captures V2", "My Captures"].map((label) => (
          <button key={label}>
            <b>
              <QcDirectoryIcon kind="folder-outline" />
            </b>
            <span>{label}</span>
          </button>
        ))}
      </aside>
      <main>
        {library ? (
          <div className="capture-library-rows">
            {captures.map((name) => (
              <button key={name}>
                <span>
                  {name}
                  <small>{FACTORY_CAPTURE_AUTHOR}</small>
                </span>
                <b>4</b>
              </button>
            ))}
            <aside className="qc-jump-index">
              {["#", "•", "A", "•", "I", "•", "R", "•", "Z"].map((label, index) => (
                <span key={`${label}-${index}`}>{label}</span>
              ))}
            </aside>
          </div>
        ) : (
          <QcScreenVectorGlyph icon="library.capture-mark" className="capture-library-placeholder" />
        )}
      </main>
    </section>
  );
}

function CorOsDevicePresetScreen({ save = false, view = "factory" }: { save?: boolean; view?: "factory" | "user" | "actions" | "official-actions" | "official-factory" }) {
  const officialActions = view === "official-actions";
  const officialFactory = view === "official-factory";
  const categories = officialActions ? COROS_DEVICE_CATEGORIES.slice(1, 7) : COROS_DEVICE_CATEGORIES.slice(0, 6);
  const devices = officialActions || officialFactory ? AMP_MODELS_FIRST_PAGE : OVERDRIVE_MODELS_FIRST_PAGE;
  const presets = officialActions ? USER_DEVICE_PRESETS : officialFactory ? FACTORY_AMP_DEVICE_PRESETS : FACTORY_OVERDRIVE_DEVICE_PRESETS;
  if (save) return <CorOsKeyboardScreen />;
  const user = view === "user" || officialActions;
  return (
    <section className={`qc-screen coros-device-presets is-physical is-${officialActions ? "official-actions" : officialFactory ? "official-factory" : view}`} aria-label="Virtual Device preset browser">
      <nav>
        {categories.map(([label, color], index) => (
          <button key={label} className={index === (officialActions ? 0 : officialFactory ? 1 : 4) ? "is-active" : ""} style={{ "--device-color": color } as CSSProperties}>
            <i>
              <DeviceCategoryGlyph label={label} />
            </i>
          </button>
        ))}
      </nav>
      <main>
        <section>
          <header>
            <button className="is-active">GUITAR</button>
            <button>BASS</button>
          </header>
          {devices.map((name, index) => (
            <button key={name} className={index === (officialActions ? 1 : officialFactory ? 3 : 0) ? "is-active" : ""}>
              {!officialActions && !officialFactory && index === 0 && (
                <i className="device-pin">
                  <QcUiIcon kind="pin" />
                </i>
              )}
              <span>{name}</span>
              <b>
                <DevicePresetGlyph />
              </b>
            </button>
          ))}
        </section>
        <section>
          <header>
            <button className={user ? "" : "is-active"}>FACTORY</button>
            <button className={user ? "is-active" : ""}>USER</button>
            <span />
            <button className="preset-close">
              <QcUiIcon kind="close" />
            </button>
            {officialFactory && (
              <button className="preset-confirm">
                <QcUiIcon kind="check" />
              </button>
            )}
          </header>
          {(officialActions || !user) &&
            presets.map((name, index) => (
              <button key={name}>
                <span className={officialFactory && index === 1 ? "is-italic" : ""}>
                  {name}
                  <small>{officialActions ? "" : index ? "" : "Default"}</small>
                </span>
                <b>
                  <QcUiIcon kind="more" />
                </b>
              </button>
            ))}
        </section>
      </main>
      {(view === "actions" || officialActions) && (
        <aside className="device-preset-actions">
          <button>Set as Default</button>
          <button>Edit Name</button>
          <button>Overwrite</button>
          <button>Delete</button>
        </aside>
      )}
    </section>
  );
}

// The EQ band's power mark and `editor.bypass` are the same POWER_SYMBOL_PATH,
// so the two names are one drawing already. The expression chooser's bypass
// mark is the same ring again.
function ExpressionPowerIcon() {
  return <QcUiIcon kind="power-toggle" />;
}

function ExpressionChooser({ trim }: { trim: boolean }) {
  const tiles: Array<[string, boolean]> = [
    ["NOISE REDUCTION", false],
    ["BYPASS", !trim],
  ];
  return (
    <section className={`qc-screen expression-bypass-official${trim ? " is-trim" : ""}`} aria-label={trim ? "Expression parameter assignment" : "Expression bypass assignment"}>
      <header>
        <button>
          <QcUiIcon kind="close" />
        </button>
        <button>Expression 1</button>
        <button>Expression 2</button>
        <button>
          <QcEditorIcon kind="save" />
        </button>
      </header>
      <p>
        {EXPRESSION_CHOOSER_BODY[0]}
        <br />
        {EXPRESSION_CHOOSER_BODY[1]}
      </p>
      {trim ? (
        <main className="expression-switch-panel is-trim">
          <section className="trim-hint">
            <span>
              {EXPRESSION_TRIM_HINT[0]}{" "}
              <b>
                <QcExpressionAssignmentLink />
              </b>{" "}
              {EXPRESSION_TRIM_HINT[1]}
            </span>
            <small>{EXPRESSION_TRIM_DETAIL}</small>
          </section>
          <section className="switch-delay">
            <span>MIN RANGE</span>
            <b>0.00 %</b>
            <QcRotaryDial className="expression-rotary is-unlit" value={0} pointer="var(--qc-palette-6b716b)" />
          </section>
          <section className="switch-delay">
            <span>MAX RANGE</span>
            <b>100 %</b>
            <QcRotaryDial className="expression-rotary" value={1} pointer="var(--qc-palette-6b716b)" />
          </section>
        </main>
      ) : (
        <main className="expression-switch-panel">
          <section>
            <button>
              <ExpressionPowerIcon />
            </button>
          </section>
          <section>
            <span>SWITCH ON</span>
            <label>
              <QcPillToggle positions={3} index={0} />
              <b>Heel-Toe</b>
              <small>
                Switch
                <br />
                Stop
              </small>
            </label>
          </section>
          <section>
            <span>INVERT RANGE</span>
            <label>
              <QcPillToggle index={1} />
              <QcPillToggleLabels labels={["On", "Off"]} index={1} />
            </label>
          </section>
          <section className="switch-delay">
            <span>SWITCH DELAY</span>
            <b>600 ms</b>
            <QcRotaryDial className="expression-rotary" value={0.3256} />
          </section>
          <section className="switch-latch">
            <span>LATCH EMULATION</span>
            <label>
              <QcPillToggle index={1} />
              <QcPillToggleLabels labels={["On", "Off"]} index={1} />
            </label>
          </section>
        </main>
      )}
      <div className="expression-parameter-grid">
        {tiles.map(([label, assigned]) => (
          <section key={label} className={assigned ? "is-assigned-tile" : ""}>
            <span>
              {label}
              <b>
                <QcExpressionAssignmentLink assigned={assigned} />
              </b>
            </span>
            <button className={assigned ? "is-assigned" : ""}>{assigned ? "ASSIGNED" : "ASSIGN"}</button>
          </section>
        ))}
      </div>
    </section>
  );
}

// The Grid, its title and the editor action bar are one screen on the unit,
// shared across the block context menu and the scene/stomp assignment
// screens, which differ only in what sits over it.
function PhysicalEditorUnderlay({ slot, letter, title, scene = "A", category, device, showPresetStack = false, blocks = 0, blockGlyphs = [], output = ["Multi", "Out"], fit = false, expression = true, bypassed = false, mode = "STOMP", children }: { slot: string; letter: string; title: string; scene?: string; category?: string; device?: string; showPresetStack?: boolean; blocks?: number; blockGlyphs?: string[]; output?: [string, string]; fit?: boolean; expression?: boolean; bypassed?: boolean; mode?: "STOMP" | "PRESET"; children?: ReactNode }) {
  return (
    <div className={`physical-grid-underlay${fit ? " is-long-title" : ""}`}>
      <div className="underlay-grid">
        <header>
          <strong>
            <span>{slot}</span>
            {letter}
          </strong>
          <h1>{title}</h1>
          <nav>
            <i>
              <GridToolbarIcon kind="undo" />
            </i>
            <b>{scene}</b>
            <i>
              <GridToolbarIcon kind="save" />
            </i>
            <i>
              <QcUiIcon kind="more" />
            </i>
          </nav>
          {/* No 24x24 wrapper: the stomp mark is a 26-unit vector, and squeezing
              it into 24 with crisp edges dropped it to 18x16 against the unit's
              23x21. The glyph sizes itself from the shared stylesheet instead. */}
          <em>
            <ModeGlyph mode={mode} />
            {mode}
          </em>
        </header>
        <main>
          <span className="underlay-route">
            In
            <br />1
          </span>
          <i className="underlay-cable" />
          {Array.from({ length: blocks }, (_, index) => (
            <i key={index} className={`underlay-block is-block-${index + 1}`}>
              {blockGlyphs[index] && <DeviceCategoryGlyph label={blockGlyphs[index]} />}
            </i>
          ))}
          <span className="underlay-output">
            {output[0]}
            <br />
            {output[1]}
          </span>
        </main>
      </div>
      {category && (
        <button className="underlay-editor-more">
          <QcUiIcon kind="more" />
        </button>
      )}
      {category && (
        <span className="underlay-editor-label">
          <small>{category}</small>
          <strong>{device}{showPresetStack && <i className="underlay-preset-mark"><QcPresetStackIcon /></i>}</strong>
        </span>
      )}
      <nav className="underlay-editor-bar">
        {expression && (
          <button className="editor-expression">
            <QcEditorIcon kind="footswitch" />
            <small>?</small>
          </button>
        )}
        <QcSceneSelector className="editor-scene" letter={scene} />
        <i className="editor-divider" />
        {/* State-based, the way the unit draws it: the power symbol while the
            block is active (block-context-bottom and stomp-assignment both put
            20x20 of it at 675,216) and the barred ring while it is bypassed
            (block-context and scene-assignment draw 24x18 at 673,217). */}
        <button className="editor-bypass" aria-label={bypassed ? "Activate device" : "Mute"} aria-pressed={bypassed}>
          <QcEditorIcon kind={bypassed ? "bypass-muted" : "bypass"} />
        </button>
        <button className="editor-confirm">
          <QcEditorIcon kind="confirm" />
        </button>
      </nav>
      {children}
    </div>
  );
}

/*
 * This used to hold a second, unconditional `return` after the one below - a
 * whole `coros-assignment is-expression` screen with an EXPRESSION PEDAL
 * ASSIGNMENT header, two pedal cards and a BYPASS ASSIGN / SWAP MIN-MAX /
 * REMOVE footer. Nothing could ever reach it: the first return is
 * unconditional, and the prop union is the two assignment names, neither of
 * which the dead screen was for. It has been unreachable since the commit that
 * introduced it - expression-parameter and expression-bypass are the only
 * expression screens there are, and ExpressionChooser answers both. It is
 * gone rather than wired up, along with the .expression-pedals rules that
 * only it could paint.
 */
function CorOsAssignmentScreen({ view }: { view: "stomp-assignment" | "scene-assignment" }) {
  const scene = view === "scene-assignment";
  const parameters: Array<[string, string, number]> = scene
    ? [["NOISE REDUCTION", "17.3 %", 0.1877]]
    : [
        ["GAIN", "0.0 dB", 0.5],
        ["BASS", "6.5", 0.65],
        ["MID", "5.0", 0.5],
        ["TREBLE", "5.0", 0.5],
        ["VOLUME", "14.9 dB", 0.8091],
      ];
  return (
    <section className={`qc-screen coros-assignment is-${scene ? "scene" : "stomp"}`} aria-label={view.replaceAll("-", " ")}>
      <PhysicalEditorUnderlay slot="4" letter={scene ? "E" : "B"} title={scene ? "QC MCP TEST_2*" : "Demo Acoustic Layer"} scene={scene ? "A" : "F"} category={scene ? "UTILITY" : "NEURAL CAPTURE"} device={scene ? MODEL_ADAPTIVE_GATE : MODEL_STUDIO_CAB} blocks={scene ? 1 : 2} blockGlyphs={scene ? ["Utility"] : ["Utility", "Neural Capture"]} output={scene ? ["Multi", "Out"] : ["Row", "3/4"]} fit={!scene} bypassed={scene}>
        <div className="assignment-parameters">
          {Array.from({ length: 5 }, (_, index) => parameters[index]).map((parameter, index) => (
            <section key={index}>
              {parameter && (
                <>
                  <span>{parameter[0]}</span>
                  {scene && (
                    <em>
                      A B<br />C D
                    </em>
                  )}
                  <QcRotaryDial className={`assignment-rotary${scene ? " is-scene-muted" : ""}`} value={parameter[2]} />
                  <strong>{parameter[1]}</strong>
                </>
              )}
            </section>
          ))}
        </div>
      </PhysicalEditorUnderlay>
      {!scene && (
        <div className="assignment-stomp-message">
          <aside className="assignment-stomp-dialog">
            <h1>Assign footswitch</h1>
            <p>{ASSIGN_FOOTSWITCH_PROMPT}</p>
            <div className="assignment-stomp-latch">
              <QcFootswitchAssignmentMode mode="latching" selected />
              <QcFootswitchAssignmentMode mode="momentary" />
            </div>
            <footer>
              <button>CANCEL</button>
              <button className="is-primary">UNASSIGN</button>
            </footer>
          </aside>
        </div>
      )}
    </section>
  );
}

function BlockContextIcon({ kind }: { kind: "change" | "copy" | "paste" | "reset" | "save" | "expression" | "bypass" | "model-update" | "model-downgrade" | "remove" }) {
  return <QcEditorIcon kind={kind === "bypass" ? "mute" : kind} />;
}

type GridPopupView = "capture-type" | "grid-context-menu" | "grid-context-menu-favorite" | "grid-context-menu-bottom" | "input-route-selector" | "input-route-selector-top" | "output-route-selector" | "output-route-selector-top" | "grid-scene-selector";

type PopupCell = {
  label: string;
  caption?: boolean;
  icon?: ReactNode;
  note?: string;
  active?: boolean;
};

// The preset menu is one table, captions included; its different states
// differ only in how far it is scrolled.
const GRID_MENU_CELLS: Array<[string, string]> = [
  ["FILE", ""],
  ["Create New", "create-new"],
  ["Save as...", "save-as"],
  ["Edit Details", "edit"],
  ["Copy Scene A", "copy"],
  ["Swap Scene A", "change"],
  ["Preset MIDI Out", "midi"],
  ["Add to favorites", "favorite"],
  ["Delete Preset", "delete"],
  ["QUAD CORTEX", ""],
  ["New Neural Capture", "capture"],
  ["Modes Configuration", "modes"],
  ["Tempo", "tempo"],
  ["CPU Monitor", "cpu"],
  ["Settings", "settings"],
];

const ROUTE_INPUT_CELLS: Array<[string, string]> = [
  ["MONO", ""],
  ["Input 1", "mono"],
  ["Input 2", "mono"],
  ["Return 1", "fx"],
  ["Return 2", "fx"],
  ["USB input 5", "USB"],
  ["USB input 6", "USB"],
  ["USB input 7", "USB"],
  ["USB input 8", "USB"],
  ["STEREO", ""],
  ["Input 1/2", "stereo"],
  ["Return 1/2", "fx"],
  ["USB input 5/6", "usb"],
  ["USB input 7/8", "usb"],
  ["OTHER", ""],
  ["Prev. Row", "mono"],
  ["Not In Use", "add"],
];

const ROUTE_OUTPUT_CELLS: Array<[string, string]> = [
  ["STEREO", ""],
  ["Multiple Outputs", "⇉"],
  ["Output 1/2", "⇉"],
  ["Output 3/4", "⇉"],
  ["Send 1/2", "FX"],
  ["USB Output 3/4", "USB"],
  ["USB Output 5/6", "USB"],
  ["USB Output 7/8", "USB"],
  ["MONO", ""],
  ["Output 1", "→"],
  ["Output 2", "→"],
  ["Output 3", "→"],
  ["Output 4", "→"],
  ["Send 1", "FX"],
  ["Send 2", "FX"],
  ["USB Output 3", "USB"],
  ["USB Output 4", "USB"],
  ["USB Output 5", "USB"],
  ["USB Output 6", "USB"],
  ["USB Output 7", "USB"],
  ["USB Output 8", "USB"],
  ["OTHER", ""],
  ["Row 3", "↩"],
  ["Row 4", "↩"],
  ["Row 3/4", "↩"],
  ["Not In Use", "+"],
];

function GridMenuIcon({ kind }: { kind: string }) {
  if (kind === "copy" || kind === "change") return <QcEditorIcon kind={kind as "copy" | "change"} />;
  return <QcUiIcon kind={kind as Parameters<typeof QcUiIcon>[0]["kind"]} />;
}

// Grid, popup box and scroll offset for each view.
const GRID_POPUP_STATE: Record<
  GridPopupView,
  {
    slot: string;
    letter: string;
    tone: string;
    name: string;
    mode: "STOMP" | "PRESET";
    blocks: Array<[number, string, string]>;
    highlight?: "input" | "output";
    kind: "menu" | "input" | "output" | "scene" | "none";
    offset: number;
  }
> = {
  "capture-type": {
    slot: "2",
    letter: "F",
    tone: "var(--qc-palette-6b55ff)",
    name: "QC MCP TEST",
    mode: "PRESET",
    blocks: [[0, "var(--qc-palette-2df36a)", "Utility"]],
    kind: "none",
    offset: 0,
  },
  "grid-context-menu": {
    slot: "32",
    letter: "H",
    tone: "var(--qc-palette-ff2421)",
    name: "pyquadcortex scratch",
    mode: "STOMP",
    blocks: [
      [0, "var(--qc-utility-mark)", "Utility"],
      [1, "var(--qc-palette-ff7100)", "Overdrive"],
      [2, "var(--qc-context-capture-muted)", "Modulation"],
      [3, "var(--qc-palette-ff2421)", "Amp"],
      [4, "var(--qc-palette-6b55ff)", "Cab"],
      [5, "var(--qc-palette-16dfcc)", "Equalizer"],
      [6, "var(--qc-palette-16dfcc)", "Reverb"],
    ],
    kind: "menu",
    offset: 0,
  },
  "grid-context-menu-favorite": {
    slot: "5",
    letter: "C",
    tone: "var(--qc-palette-ff7100)",
    name: "Demo",
    mode: "PRESET",
    blocks: [
      [1, "var(--qc-palette-16dfcc)", "Reverb"],
      [2, "var(--qc-palette-ff2421)", "Amp"],
    ],
    kind: "menu",
    offset: 420,
  },
  "grid-context-menu-bottom": {
    slot: "2",
    letter: "F",
    tone: "var(--qc-palette-6b55ff)",
    name: "QC MCP TEST_2",
    mode: "PRESET",
    blocks: [[0, "var(--qc-palette-2df36a)", "Utility"]],
    kind: "menu",
    offset: 480,
  },
  "input-route-selector": {
    slot: "32",
    letter: "H",
    tone: "var(--qc-palette-ff2421)",
    name: "pyquadcortex scratch",
    mode: "STOMP",
    blocks: [
      [0, "var(--qc-utility-mark)", "Utility"],
      [1, "var(--qc-palette-ff7100)", "Overdrive"],
      [2, "var(--qc-context-capture-muted)", "Modulation"],
      [3, "var(--qc-palette-ff2421)", "Amp"],
      [4, "var(--qc-palette-6b55ff)", "Cab"],
      [5, "var(--qc-palette-16dfcc)", "Equalizer"],
      [6, "var(--qc-palette-16dfcc)", "Reverb"],
    ],
    highlight: "input",
    kind: "input",
    offset: 480,
  },
  "input-route-selector-top": {
    slot: "5",
    letter: "C",
    tone: "var(--qc-palette-ff7100)",
    name: "Demo",
    mode: "PRESET",
    blocks: [
      [1, "var(--qc-palette-16dfcc)", "Reverb"],
      [2, "var(--qc-palette-ff2421)", "Amp"],
    ],
    highlight: "input",
    kind: "input",
    offset: 0,
  },
  "output-route-selector": {
    slot: "32",
    letter: "H",
    tone: "var(--qc-palette-ff2421)",
    name: "pyquadcortex scratch",
    mode: "STOMP",
    blocks: [
      [0, "var(--qc-utility-mark)", "Utility"],
      [1, "var(--qc-palette-ff7100)", "Overdrive"],
      [2, "var(--qc-context-capture-muted)", "Modulation"],
      [3, "var(--qc-palette-ff2421)", "Amp"],
      [4, "var(--qc-palette-6b55ff)", "Cab"],
      [5, "var(--qc-palette-16dfcc)", "Equalizer"],
      [6, "var(--qc-palette-16dfcc)", "Reverb"],
    ],
    highlight: "output",
    kind: "output",
    offset: 1152,
  },
  "output-route-selector-top": {
    slot: "5",
    letter: "C",
    tone: "var(--qc-palette-ff7100)",
    name: "Demo",
    mode: "PRESET",
    blocks: [
      [1, "var(--qc-palette-16dfcc)", "Reverb"],
      [2, "var(--qc-palette-ff2421)", "Amp"],
    ],
    highlight: "output",
    kind: "output",
    offset: 0,
  },
  "grid-scene-selector": {
    slot: "32",
    letter: "H",
    tone: "var(--qc-palette-ff2421)",
    name: "pyquadcortex scratch",
    mode: "STOMP",
    blocks: [
      [0, "var(--qc-utility-mark)", "Utility"],
      [1, "var(--qc-palette-ff7100)", "Overdrive"],
      [2, "var(--qc-context-capture-muted)", "Modulation"],
      [3, "var(--qc-palette-ff2421)", "Amp"],
      [4, "var(--qc-palette-6b55ff)", "Cab"],
      [5, "var(--qc-palette-16dfcc)", "Equalizer"],
      [6, "var(--qc-palette-16dfcc)", "Reverb"],
    ],
    kind: "scene",
    offset: 0,
  },
};

/**
 * Where CorOS parks the route panel's scroll thumb.
 *
 * The track is 408px, which is the 420px panel inset 6px at each end, and the
 * thumb starts at y 36. The thumb's own length is that track scaled by 406,
 * not by the panel.
 */
function routeScrollbar(kind: string, offset: number, rows: number) {
  if (kind !== "input" && kind !== "output") return undefined;
  const content = rows * 60;
  return { top: `${6 + (offset * 408) / content}px`, height: `${(408 * 406) / content}px` };
}

function CorOsGridPopup({ view }: { view: GridPopupView }) {
  const state = GRID_POPUP_STATE[view];
  const scenes = ["Default scene", "Scene B", "Scene C", "Scene D", "Scene E", "Scene F", "Scene G", "Scene H"];
  // The Grid's first row has no previous row to take from, so its input
  // picker ends "Not In Use" straight after "USB input 7/8" with no OTHER
  // caption between. ROUTE_INPUT_CELLS still carries both, because the picker
  // has to cover every port the protocol declares.
  const inputRows = ROUTE_INPUT_CELLS.filter(([label]) => label !== "OTHER" && label !== "Prev. Row");
  const source = state.kind === "menu" ? GRID_MENU_CELLS : state.kind === "input" ? inputRows : ROUTE_OUTPUT_CELLS;
  const cells: PopupCell[] =
    state.kind === "scene"
      ? scenes.map((label, index) => ({ label, active: index === 0 }))
      : source.map(([label, icon]) => ({
          label,
          caption: !icon,
          icon: state.kind === "menu" ? icon ? <GridMenuIcon kind={icon} /> : undefined : icon ? <QcRouteGlyph side={state.kind as "input" | "output"} label={label} /> : undefined,
          note: label === "Multiple Outputs" ? "1/2 + 3/4 + USB 3/4" : undefined,
          active: label === "Input 1" || label === "Multiple Outputs",
        }));
  return (
    <section className={`qc-screen coros-grid-popup is-${state.kind}`} aria-label={view.replaceAll("-", " ")}>
      <div className="popup-grid">
        <header>
          <strong>
            <span>{state.slot}</span>
            <b style={{ color: state.tone }}>{state.letter}</b>
          </strong>
          <h1
            className={state.name.length > 15 ? undefined : "is-large"}
            style={{
              fontSize: `${state.name.length > 15 ? 40 : 68}px`,
              marginTop: state.name.length > 15 ? "11px" : "1px",
            }}
          >
            {state.name}
          </h1>
          <nav>
            <i>
              <GridToolbarIcon kind="undo" />
            </i>
            <QcSceneBadge letter="A" tone={state.kind === "scene" ? "inactive" : "active"} />
            <i>
              <GridToolbarIcon kind="save" />
            </i>
            <i>
              <QcUiIcon kind="more" />
            </i>
          </nav>
          <em>
            <ModeGlyph mode={state.mode} />
            {state.mode}
          </em>
        </header>
        <main>
          <span className={`popup-route${state.highlight === "input" ? " is-open" : ""}`}>
            In
            <br />1
          </span>
          <i className="popup-cable" />
          {state.blocks.map(([index, tone, label]) => (
            <i
              key={index}
              className={`popup-block${view === "capture-type" ? " is-capture-wave" : ""}`}
              style={{
                left: `${69 + Number(index) * 86}px`,
                borderColor: tone,
                color: tone,
              }}
            >
              {view === "capture-type" ? <QcScreenVectorGlyph icon="block.capture-wave" /> : <DeviceCategoryGlyph label={label as string} />}
            </i>
          ))}
          <span className={`popup-output${state.highlight === "output" ? " is-open" : ""}`}>
            Multi
            <br />
            Out
          </span>
          {[1, 2, 3].map((row) => (
            <span key={row} className="popup-plus" style={{ top: `${13 + row * 94}px` }}>
              <QcUiIcon kind="add" />
            </span>
          ))}
          {[1, 2, 3].map((row) => (
            <span key={`r${row}`} className="popup-plus is-right" style={{ top: `${13 + row * 94}px` }}>
              <QcUiIcon kind="add" />
            </span>
          ))}
        </main>
      </div>
      {state.kind !== "scene" && state.kind !== "none" && <i className="popup-scrim" />}
      {state.kind !== "none" && (
        <aside className="popup-panel">
          <div className="popup-list" style={{ top: `${-state.offset}px` }}>
            {cells.map((cell, index) => (
              <div key={`${cell.label}-${index}`} className={`popup-cell${cell.caption ? " is-caption" : ""}${cell.active ? " is-active" : ""}`}>
                {state.kind === "scene" ? <QcSceneBadge className="scene-badge" letter={String.fromCharCode(65 + index)} /> : cell.icon}
                <span>
                  {cell.label}
                  {cell.note && <small>{cell.note}</small>}
                </span>
              </div>
            ))}
          </div>
          <i className="popup-scrollbar" style={routeScrollbar(state.kind, state.offset, cells.length)} />
        </aside>
      )}
    </section>
  );
}

function CorOsBlockContext({ bottom = false }: { bottom?: boolean }) {
  const rows: Array<[Parameters<typeof BlockContextIcon>[0]["kind"], string, string]> = [
    ["change", "Change device", ""],
    ["copy", "Copy device", ""],
    ["paste", "Paste device", "is-disabled"],
    ["reset", "Reset to defaults", ""],
    // The reverb the scrolled frame has open carries no saveable edit, and the
    // unit greys "Save Current Parameters as..." for it - block-context, whose
    // gate does, draws the same row live.
    ["save", "Save Current Parameters as...", bottom ? "is-disabled" : ""],
    ["expression", "Assign expression pedal", ""],
    ["bypass", "Mute/bypass", ""],
    ["model-update", "Model update available", "is-disabled"],
    ["model-downgrade", "Change to legacy version", "is-disabled"],
    ["remove", "Remove block from the grid", ""],
  ];
  /* This preset's reverb does not sit where editor-ambience's does. */
  const reverb: Array<[string, string, number]> = [
    ["MIX", "12.0 %", 0.1353],
    ["SIZE", "Med", 0.5],
    ["PRE DELAY", "20.0 ms", 0.5725],
    ["DAMPING", "50 %", 0.5],
    ["HIGH PASS", "80 Hz", 0.2269],
  ];
  return (
    <section className={`qc-screen coros-block-context${bottom ? " is-bottom" : ""}`} aria-label="Block contextual actions">
      {bottom ? (
        <PhysicalEditorUnderlay slot="5" letter="C" title="Demo" category="REVERB" device={MODEL_AMBIENCE} blocks={2} blockGlyphs={["Utility", "Reverb"]} expression={false}>
          <div className="assignment-parameters">
            {reverb.map(([label, value, dial]) => (
              <section key={label}>
                <span>{label}</span>
                <QcRotaryDial className="assignment-rotary" value={dial} />
                <strong>{value}</strong>
              </section>
            ))}
          </div>
          <div className="assignment-parameters is-row-2">
            {[["LOW PASS", "6000 Hz"]].map(([label, value]) => (
              <section key={label}>
                <span>{label}</span>
                <QcRotaryDial className="assignment-rotary" value={0.4866} />
                <strong>{value}</strong>
              </section>
            ))}
            {[0, 1, 2, 3].map((index) => (
              <section key={`empty-${index}`} />
            ))}
          </div>
        </PhysicalEditorUnderlay>
      ) : (
        <PhysicalEditorUnderlay slot="4" letter="E" title="QC MCP TEST_2" category="UTILITY" device={MODEL_ADAPTIVE_GATE} blocks={1} blockGlyphs={["Utility"]} bypassed>
          <div className="assignment-parameters">
            <section>
              <span>NOISE REDUCTION</span>
              <QcRotaryDial className="assignment-rotary" value={0.1877} />
              <strong>17.3 %</strong>
            </section>
            {[0, 1, 2, 3].map((index) => (
              <section key={`empty-${index}`} />
            ))}
          </div>
        </PhysicalEditorUnderlay>
      )}
      <i className="block-context-scrim" />
      {/* Scrolled to the bottom, the unit keeps the menu box where it is and
          moves its contents; shifting the box itself lifted its top edge off
          the screen and covered thirty rows of dimmed Grid that should show. */}
      <aside>
        {rows.map(([kind, label, className], index) => (
          <button
            key={label}
            className={`${className}${index === 3 ? " has-gap" : ""}`}
            style={bottom && index === 0 ? { marginTop: "-180px" } : undefined}
          >
            <span>
              <BlockContextIcon kind={kind} />
            </span>
            {label}
          </button>
        ))}
      </aside>
    </section>
  );
}

const COROS_INPUT_MONO_ROUTES = ["Input 1", "Input 2", "Return 1", "Return 2", "USB input 5", "USB input 6", "USB input 7", "USB input 8"];
const COROS_INPUT_ROUTES = ["Input 1/2", "Return 1/2", "USB input 5/6", "USB input 7/8", "Not In Use"];
const COROS_OUTPUT_ROUTES = ["Multi Out", "Output 1/2", "Output 3/4", "Output 1", "Output 2", "Output 3", "Output 4", "Send 1", "Send 2", "Send 1/2", "USB Output 3", "USB Output 4", "USB Output 3/4", "USB Output 5", "USB Output 6", "USB Output 7", "USB Output 8", "OTHER", "Row 3", "Row 4", "Row 3/4", "Not In Use"];

function DevicePresetGlyph() {
  return <QcPresetStackIcon />;
}

function PluginLockIcon() {
  return <QcUiIcon kind="plugin-lock" />;
}

function RouteSymbol({ label }: { label: string }) {
  if (label.startsWith("USB"))
    return (
      <span className="route-symbol">
        <QcScreenGlyph kind="route-usb" />
      </span>
    );
  if (label.startsWith("Return")) return <span className="route-symbol route-fx">FX</span>;
  if (label === "Not In Use")
    return (
      <span className="route-symbol route-unused">
        <QcUiIcon kind="add" />
      </span>
    );
  if (label.startsWith("Input"))
    return (
      <span className="route-symbol route-stereo-input">
        <QcScreenGlyph kind="route-input" />
      </span>
    );
  if (label.startsWith("Row"))
    return (
      <span className="route-symbol">
        <QcScreenGlyph kind="route-row" />
      </span>
    );
  return (
    <span className="route-symbol">
      <QcScreenGlyph kind="route-output" />
    </span>
  );
}

const DEVICE_CATEGORIES = [
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
] as const;
/** The categories the browser badges as new. */
const DEVICE_NEW_CATEGORIES = new Set(["Delay", "Reverb", "Compressor", "Pitch", "Morph"]);

/* CorOS fits the preset name to the space the header leaves it, and the browser
   frames read that rule most clearly: on device-browser-base "2F QC MCP TEST"
   prints at the bank's own 69px, on device-browser-root "32H pyquadcortex
   scratch" at 39.9px, and grid-base prints that same string at that same size
   with no browser over it at all. Both fall out of one rule - the name shrinks
   until its advance reaches PRESET_TITLE_RIGHT_EDGE, and never grows past the
   bank - so the size is derived from the string here rather than pinned per
   screen. The baseline rises with the shrink by 0.375 of the size lost, which
   is the 11px device-browser-root shows at 39.9 and the 0 device-browser-base
   shows at 69. */
function CorOsReferenceDeviceBrowser({ snapshot, view }: { snapshot: PresetSnapshot; view: "reference-device-browser-root" | "reference-device-browser-top" | "reference-device-browser-models" | "reference-device-browser-models-clean" | "device-browser-middle-deep" | "device-browser-middle-reverb" | "device-browser-base" }) {
  const models = view === "reference-device-browser-models" || view === "reference-device-browser-models-clean";
  const newCategories = new Set(["Delay", "Reverb", "Compressor", "Pitch", "Morph"]);
  // device-browser-middle-deep opens on Compressor and -reverb on Cab: the
  // same list, scrolled seven and three of its 78px rows.
  // device-browser-base is the same list at its foot: nineteen categories,
  // the last six on screen.
  const base = view === "device-browser-base";
  // device-browser-top is not a third state of this screen, it is a crossing
  // of two: its left half matches device-browser-base's - "2F QC MCP TEST"
  // over one block - and its right half matches the reference device
  // browser's category list at the top of its scroll. Rendering it as root
  // entire put the wrong preset and the wrong chain under the wash across a
  // whole half-view.
  const top = view === "reference-device-browser-top";
  const scrolled = view === "device-browser-middle-deep" ? 7 : view === "device-browser-middle-reverb" ? 3 : base ? 13 : 0;
  const middle = scrolled > 0 && !base;
  // The views do not land on whole rows: the list runs on by 8px in the two
  // middle views and 4px in the last one.
  const scrollPx = models ? 0 : scrolled * 78 + (middle ? 9 : base ? 5 : 1);
  // Both middle views show 3C over a four-row grid.
  const gridSnapshot: PresetSnapshot = base || top
    ? {
        ...snapshot,
        presetLocation: "2F",
        presetName: "QC MCP TEST",
        presetPosition: 13,
        routes: [
          {
            row: 0,
            inputId: 1,
            outputId: 0,
            input: "In 1",
            output: "",
            splitMuted: false,
          },
          {
            row: 1,
            inputId: 0,
            outputId: 0,
            input: "",
            output: "",
            splitMuted: false,
          },
          {
            row: 2,
            inputId: 0,
            outputId: 0,
            input: "",
            output: "",
            splitMuted: false,
          },
          {
            row: 3,
            inputId: 0,
            outputId: 0,
            input: "",
            output: "",
            splitMuted: false,
          },
        ],
        blocks: [
          {
            id: "base-capture",
            name: "Neural Capture",
            kind: "capture",
            category: "Neural Capture",
            row: 0,
            column: 0,
          },
        ],
      }
    : middle
      ? {
          ...snapshot,
          presetLocation: "3C",
          presetName: PRESET_12_STRING_BASS,
          routes: [
            {
              row: 0,
              inputId: 0,
              outputId: 0,
              input: "In 1",
              output: "",
              splitMuted: false,
            },
            {
              row: 1,
              inputId: 0,
              outputId: 0,
              input: "In 1",
              output: "",
              splitMuted: false,
            },
            {
              row: 2,
              inputId: 0,
              outputId: 0,
              input: "Prev. Row",
              output: "",
              splitMuted: false,
            },
            {
              row: 3,
              inputId: 0,
              outputId: 0,
              input: "In 1",
              output: "",
              splitMuted: false,
            },
          ],
          blocks: [
            {
              id: "c3-0-0",
              name: MODEL_SIMPLE_GATE,
              kind: "utility",
              category: "Utility",
              row: 0,
              column: 0,
            },
            {
              id: "c3-0-1",
              name: MODEL_DIGITAL_DELAY,
              kind: "delay",
              category: "Delay",
              row: 0,
              column: 1,
            },
            {
              id: "c3-0-2",
              name: MODEL_STUDIO_COMP,
              kind: "utility",
              category: "Compressor",
              row: 0,
              column: 2,
            },
            {
              id: "c3-0-3",
              name: MODEL_TRANSPOSE,
              kind: "utility",
              category: "Pitch",
              row: 0,
              column: 3,
            },
            {
              id: "c3-1-0",
              name: MODEL_SIMPLE_GATE,
              kind: "utility",
              category: "Utility",
              row: 1,
              column: 0,
            },
            {
              id: "c3-1-1",
              name: MODEL_DIGITAL_DELAY,
              kind: "delay",
              category: "Delay",
              row: 1,
              column: 1,
            },
            {
              id: "c3-1-2",
              name: MODEL_STUDIO_COMP,
              kind: "utility",
              category: "Compressor",
              row: 1,
              column: 2,
            },
            {
              id: "c3-1-3",
              name: MODEL_TRANSPOSE,
              kind: "utility",
              category: "Pitch",
              row: 1,
              column: 3,
            },
            {
              id: "c3-2-2",
              name: MODEL_STUDIO_COMP,
              kind: "utility",
              category: "Compressor",
              row: 2,
              column: 2,
            },
            {
              id: "c3-2-3",
              name: MODEL_TRANSPOSE,
              kind: "utility",
              category: "Pitch",
              row: 2,
              column: 3,
              bypassed: true,
            },
            {
              id: "c3-3-0",
              name: MODEL_SIMPLE_GATE,
              kind: "utility",
              category: "Utility",
              row: 3,
              column: 0,
            },
            {
              id: "c3-3-2",
              name: MODEL_STUDIO_COMP,
              kind: "utility",
              category: "Compressor",
              row: 3,
              column: 2,
            },
          ],
        }
      : snapshot;
  return (
    <CorOsOfficialGrid referenceOnly snapshot={gridSnapshot} browserChrome screenClassName="coros-device-browser-grid" letterTone={middle ? "var(--qc-palette-ff7100)" : undefined}>
      <DeviceEmptySlotGlyph className={`coros-device-empty-slot${middle ? " is-row-4" : ""}`} />
      <button className="coros-device-dismiss" aria-label="Close device browser" />
      {/* The list is scrolled with a margin, so its thumb is drawn rather than
        left to the browser: the thumb sits at y 13 unscrolled, 86 at three
        rows, 180 at seven and 321 at thirteen - 145 tall throughout. */}
      <section
        className="coros-device-browser"
        aria-label="Virtual Device browser"
        style={
          {
            "--browser-thumb-top": `${13 + scrolled * 23.7}px`,
          } as CSSProperties
        }
      >
        <nav style={{ marginTop: `${-scrollPx}px` }}>
          {DEVICE_CATEGORIES.map(([label, color]) => (
            <button key={label} data-category={label} className={models && label === "Overdrive" ? "is-active" : ""} style={{ "--device-color": color } as CSSProperties}>
              <i>
                <DeviceCategoryGlyph label={label} />
              </i>
              <span>{label === "Equalizer" ? "EQ" : label}</span>
              {DEVICE_NEW_CATEGORIES.has(label) && <b>New</b>}
            </button>
          ))}
        </nav>
        {models && (
          <div className="coros-device-models qc-scroll-rail">
            <header>
              <button className="is-active">GUITAR</button>
              <button>BASS</button>
            </header>
            {OVERDRIVE_MODELS.map((model, index) => (
              <button key={model}>
                <span>
                  {index === 0 ? (
                    <b className="device-model-pin">
                      <QcUiIcon kind="pin" />
                    </b>
                  ) : null}
                  {model}
                </span>
                <i>
                  <DevicePresetGlyph />
                </i>
              </button>
            ))}
          </div>
        )}
      </section>
      {models && view !== "reference-device-browser-models-clean" && (
        <aside className="coros-device-preset-tip">
          <button aria-label="Dismiss Virtual Device Presets tip">
            <QcUiIcon kind="close" />
          </button>
          <strong>VIRTUAL DEVICE PRESETS</strong>
          <span>{VIRTUAL_DEVICE_PRESETS_TIP}</span>
        </aside>
      )}
    </CorOsOfficialGrid>
  );
}

function IconographyAuditFixture() {
  const ui = ["subtract", "up"] as const;
  const editor = ["assignment-expression", "waveform"] as const;
  const hardware = ["power-led", "brand-pulse"] as const;
  return (
    <section className="qc-screen iconography-audit-fixture" aria-label="App-owned iconography audit">
      <main>
        {ui.map((kind) => (
          <figure key={kind} data-icon={kind}>
            <QcUiIcon kind={kind} />
            <figcaption>{kind}</figcaption>
          </figure>
        ))}
        {Array.from({ length: 4 }, (_, index) => (
          <figure key={`unused-${index}`} aria-hidden="true" />
        ))}
        {editor.map((kind) => (
          <figure key={kind} data-icon={kind}>
            <QcEditorIcon kind={kind} />
            <figcaption>{kind}</figcaption>
          </figure>
        ))}
        {hardware.map((kind) => (
          <figure key={kind} data-icon={kind}>
            <QcHardwareIcon kind={kind} />
            <figcaption>{kind}</figcaption>
          </figure>
        ))}
        <figure data-icon="previous">
          <QcUiIcon kind="previous" />
          <figcaption>previous</figcaption>
        </figure>
        <div className="iconography-gap-row">
          {(["arrow-right", "collapse", "power-toggle", "phase-invert", "pin"] as const).map((kind) => (
            <figure key={kind} data-icon={kind}>
              <QcUiIcon kind={kind} />
              <figcaption>{kind}</figcaption>
            </figure>
          ))}
        </div>
      </main>
    </section>
  );
}

type SynthControl = { label: string; value: string; dial?: number; kind?: "toggle" | "select" | "amount"; steps?: number; info?: true };

const SYNTH_MAIN_CONTROLS: SynthControl[] = [
  { label: "MIX", value: "100 %", dial: 1 }, { label: "SENSITIVITY", value: "50 %", dial: .5, info: true },
  { label: "ENV. RETRIGGER", value: "On", kind: "toggle" }, { label: "GATE", value: "-50.0 dB", dial: .333 },
  { label: "S/C SOURCE", value: "Off", kind: "select" }, { label: "GLIDE", value: "0 ms", dial: 0 },
  { label: "OUTPUT", value: "-5.0 dB", dial: .278 }, { label: "DECAY/ATTACK", value: "0 ms", dial: .5 },
  { label: "ENV. SUSTAIN", value: "100 %", dial: 1 }, { label: "MIDI CH.", value: "Off", kind: "select", info: true }
];

const SYNTH_OSC_CONTROLS: SynthControl[] = [
  { label: "OSC 1 STEPS", value: "0 st", dial: .5 }, { label: "OSC 1 DETUNE", value: "0 %", dial: .5 },
  { label: "UNISON SOURCE", value: "OSC 1 + 2", kind: "select" }, { label: "OSC 2 DETUNE", value: "0 %", dial: .5 },
  { label: "OSC 2 STEPS", value: "0 st", dial: .5 }, { label: "OSC 1 LEVEL", value: "0.0 dB", dial: .5 },
  { label: "UNI. WIDTH", value: "100 %", dial: 1 }, { label: "UNI. AMOUNT", value: "2", kind: "amount", dial: .25, steps: 5 },
  { label: "UNI. DETUNE", value: "5 %", dial: .2 }, { label: "OSC 2 LEVEL", value: "0.0 dB", dial: .5 }
];

/* The device draws the unit in its own muted role beside a full-white number,
   so the readout is split rather than coloured as one string. */
function SynthReadout({ value }: { value: string }) {
  const unit = /^(.*) (%|dB|ms|st)$/.exec(value);
  return <strong>{unit ? <>{unit[1]} <i>{unit[2]}</i></> : value}</strong>;
}

function SynthControlCell({ control, accent }: { control: SynthControl; accent: string }) {
  return <div className={`synth-control synth-control-${control.kind ?? "dial"}`}>
    <span>{control.label}</span>
    {control.info && <i className="synth-info" />}
    {control.kind === "toggle" ? <div className="synth-toggle"><i className="synth-toggle-track"><b /></i><small>On</small><em>Off</em></div>
      : control.kind === "select" ? <div className="synth-select">{control.value}<QcUiIcon kind="down" /></div>
      : control.kind === "amount" ? <QcRotaryDial value={control.dial ?? 0} mode="step" steps={control.steps ?? 5} accent={accent} />
      : <QcRotaryDial value={control.dial ?? 0} accent={accent} />}
    {control.kind !== "select" && control.kind !== "toggle" && <SynthReadout value={control.value} />}
  </div>;
}

function SynthFooter({ page }: { page: "main" | "oscillator" }) {
  const pages = ["GLOBAL & AMP", "ARPEGGIATOR", "OSCILLATOR", "FILTER"];
  return <footer className="synth-pages">{pages.map((label) => <div key={label} className={(page === "oscillator" ? label === "OSCILLATOR" : label === "GLOBAL & AMP") ? "selected" : ""}>
    {label === "OSCILLATOR" ? <QcEditorIcon kind="waveform" /> : label === "FILTER" ? <QcSynthFooterIcon kind="filter" /> : label === "ARPEGGIATOR" ? <QcLibraryIcon kind="impulse-response" /> : <QcSynthFooterIcon kind="global-amp" />}
    <span>{label}</span>
  </div>)}</footer>;
}

function SynthWaveSelector() {
  const waves: QcSynthWaveformName[] = ["sine", "triangle", "saw", "square", "pulse", "white-noise", "pink-noise"];
  return <section className="synth-wave-selector">
    <div className="synth-osc-bank osc-one"><div className="synth-osc-label"><span>OSC 1</span><b>TRI</b><i /></div>{waves.map((wave) => <div key={`one-${wave}`} className={wave === "triangle" ? "selected" : ""}><QcSynthWaveformGlyph kind={wave} /></div>)}</div>
    <div className="synth-osc-bank osc-two"><div className="synth-osc-label"><span>OSC 2</span><b>SAW</b><i /></div>{waves.map((wave) => <div key={`two-${wave}`} className={wave === "saw" ? "selected" : ""}><QcSynthWaveformGlyph kind={wave} /></div>)}</div>
  </section>;
}

function CorOsOverlordSynth({ page }: { page: "main" | "oscillator" }) {
  const controls = page === "main" ? SYNTH_MAIN_CONTROLS : SYNTH_OSC_CONTROLS;
  return <section className={`coros-overlord-synth page-${page}`} style={{ "--synth-accent": QC_COLORS.browserCategory.synth } as CSSProperties}>
    <header><button className="synth-more"><QcUiIcon kind="more" /></button><div className="synth-title"><small>SYNTH</small><b>{MODEL_OVERLORD_SYNTH}</b><QcPresetStackIcon /></div><QcSceneSelector className="synth-scene" letter="A" /><button className="synth-bypass"><QcEditorIcon kind="bypass" /></button><button className="synth-done"><QcUiIcon kind="check" /></button></header>
    {page === "main" ? <section className="synth-envelope"><span>ATTACK</span><i className="synth-envelope-line" /><i className="synth-envelope-node" /></section> : <SynthWaveSelector />}
    <section className="synth-controls">{controls.map((control, index) => {
      const accent = page === "main" ? (["DECAY/ATTACK", "ENV. SUSTAIN"].includes(control.label) ? QC_COLORS.browserCategory.synth : "var(--qc-screen-text-primary)")
        : [0, 1, 5].includes(index) ? QC_COLORS.browserCategory.synth : [3, 4, 9].includes(index) ? "var(--qc-palette-6b55ff)" : "var(--qc-screen-text-primary)";
      return <SynthControlCell key={control.label} control={control} accent={accent} />;
    })}</section>
    <SynthFooter page={page} />
  </section>;
}

export function CorOsScreenFixture({ view, snapshot, gigPresetList, deviceInformation, onClose = () => undefined, onEditScene, onSelectScene, onSelectModeSlot, onRecallGigPreset, onPressGigFootswitch, onEditGigBlock, onCopyGigScene, onSwapGigScene }: { view: CorOsScreenView; snapshot: PresetSnapshot; gigPresetList?: PresetList; deviceInformation?: CorOsDeviceInformation; onClose?: () => void; onEditScene?: (scene: number) => void; onSelectScene?: (scene: number) => void; onSelectModeSlot?: (slot: 0 | 1 | 2) => void; onRecallGigPreset?: (entry: PresetEntry) => void; onPressGigFootswitch?: (index: number) => void; onEditGigBlock?: (blockId: string) => void; onCopyGigScene?: (scene: number) => void; onSwapGigScene?: (scene: number) => void }) {
  if (view === "editor-overlord-synth" || view === "editor-overlord-synth-oscillator") return <CorOsOverlordSynth page={view.endsWith("oscillator") ? "oscillator" : "main"} />;
  if ((view as string) === "iconography-audit") return <IconographyAuditFixture />;
  if (view === "grid-official-brit") return <CorOsOfficialGrid referenceOnly snapshot={snapshot} />;
  if (view === "device-browser-dsp-limited")
    return (
      <CorOsOfficialGrid
        referenceOnly
        browserChrome
        snapshot={{
          ...snapshot,
          presetLocation: "2F",
          presetName: "QC MCP TEST",
          presetPosition: 13,
          mode: "SCENE",
          dirty: true,
          routes: [
            {
              row: 0,
              inputId: 1,
              outputId: 19,
              input: "In 1",
              output: "Multi Out",
              splitMuted: false,
            },
            {
              row: 1,
              inputId: 0,
              outputId: 0,
              input: "",
              output: "",
              splitMuted: false,
            },
            {
              row: 2,
              inputId: 0,
              outputId: 0,
              input: "",
              output: "",
              splitMuted: false,
            },
            {
              row: 3,
              inputId: 0,
              outputId: 0,
              input: "",
              output: "",
              splitMuted: false,
            },
          ],
          blocks: [
            {
              id: "dsp-capture",
              name: "Neural Capture",
              kind: "capture",
              category: "Neural Capture",
              row: 0,
              column: 0,
            },
          ],
        }}
      >
        <i className="coros-dsp-scrim" />
        <DeviceEmptySlotGlyph className="coros-device-empty-slot is-dsp" />
        <section className="coros-dsp-browser" aria-label="Device browser with the models the DSP cannot fit disabled">
          <nav>
            {DEVICE_CATEGORIES.slice(0, 6).map(([label, color], index) => (
              <button key={label} className={index === 1 ? "is-active" : ""} style={{ "--device-color": color } as CSSProperties}>
                <i>
                  <DeviceCategoryGlyph label={label} />
                </i>
              </button>
            ))}
          </nav>
          <div className="coros-dsp-models">
            <header>
              <button className="is-active">GUITAR</button>
              <button>BASS</button>
            </header>
            {DSP_LIMITED_AMPS.map((model, index) => (
              <button key={model} className={index === 0 ? "" : "is-unavailable"}>
                <span>{model}</span>
                <i>
                  <DevicePresetGlyph />
                </i>
              </button>
            ))}
          </div>
        </section>
      </CorOsOfficialGrid>
    );
  if (view === "reference-device-browser-root" || view === "reference-device-browser-top" || view === "reference-device-browser-models" || view === "reference-device-browser-models-clean" || view === "device-browser-middle-deep" || view === "device-browser-middle-reverb" || view === "device-browser-base") return <CorOsReferenceDeviceBrowser snapshot={snapshot} view={view} />;
  if (view.startsWith("fixture-")) return <CorOsRemainingFixture view={view as RemainingFixtureView} />;
  if (view.startsWith("recovery-") || view.startsWith("overlay-")) return <CorOsSystemFixture view={view as SystemFixtureView} />;
  if (view === "tuner-live-enabled") return <CorOsTuner liveTuner onClose={onClose} />;
  if (view.startsWith("gig-official-")) return <CorOsOfficialGig mode={view.replace("gig-official-", "") as OfficialGigMode} />;
  if (view === "device-presets-official") return <CorOsDevicePresetScreen view="official-factory" />;
  if (view === "gig" || view === "gig-live-tuner") return <CorOsGigView snapshot={snapshot} presetList={gigPresetList} liveTuner={view === "gig-live-tuner"} onClose={onClose} onEditScene={onEditScene} onSelectScene={onSelectScene} onSelectModeSlot={onSelectModeSlot} onRecallPreset={onRecallGigPreset} onPressFootswitch={onPressGigFootswitch} onEditBlock={onEditGigBlock} onCopyScene={onCopyGigScene} onSwapScene={onSwapGigScene} />;
  if (view === "gig-view-preset" || view === "gig-view-scene")
    return (
      <CorOsGigView
        snapshot={{
          ...snapshot,
          mode: view === "gig-view-preset" ? "PRESET" : "SCENE",
        }}
        presetList={gigPresetList}
        onClose={onClose}
        onEditScene={onEditScene}
      />
    );
  // gig-view-hybrid shows scene F live, and its two rows are presets over
  // scenes.
  if (view === "gig-view-hybrid")
    return (
      <CorOsGigView
        snapshot={{
          ...snapshot,
          mode: "HYBRID",
          footswitchModes: ["PRESET", "SCENE"],
          presetLocation: "7B",
          presetName: "Demo Acoustic Layer",
          presetPosition: 49,
          activeScene: 5,
          scenes: ["Scene A", "Scene B", "Scene C", "Scene D", "mono", "stereo", "chor", "fx"],
        }}
        presetList={{
          setlistKey: snapshot.setlistKey,
          setlistName: "My Presets",
          currentPosition: 49,
          folders: [],
          presets: [
            {
              position: 48,
              location: "7A",
              name: "Demo Acoustic 1",
              instrument: 0,
            },
            {
              position: 49,
              location: "7B",
              name: "Demo Acoustic Layer",
              instrument: 0,
            },
            {
              position: 50,
              location: "7C",
              name: "Demo 12string Bass",
              instrument: 0,
            },
            {
              position: 51,
              location: "7D",
              name: "Demo Rock Rhythm",
              instrument: 0,
            },
          ],
        }}
        onClose={onClose}
        onEditScene={onEditScene}
      />
    );
  if (view === "tuner") return <CorOsTuner official={typeof window !== "undefined" && new URLSearchParams(window.location.search).get("tunerState") === "official"} onClose={onClose} />;
  if (view === "tempo") return <CorOsTempo bpm={snapshot.tempo} official={typeof window !== "undefined" && new URLSearchParams(window.location.search).get("tempoState") === "official"} onClose={onClose} />;
  if (view === "midi-out") return <CorOsMidiOut onClose={onClose} />;
  if (view === "cpu-monitor") return <CorOsCpuMonitor snapshot={snapshot} onClose={onClose} />;
  if (view === "global-eq" || view === "global-eq-on" || view === "global-eq-enabled" || view === "global-eq-high-pass" || view === "global-eq-filter-type") return <CorOsGlobalEq onClose={onClose} view={view} />;
  if (view.startsWith("io-")) return <CorOsIoSettings initialView={view.slice(3) as IoView} onClose={onClose} />;
  if (view === "power-overlay") return <CorOsPowerOverlay onClose={onClose} />;
  if (view === "device-browser-amp-official" || view === "plugin-devices-official") return <CorOsOfficialDeviceBrowser plugins={view === "plugin-devices-official"} />;
  if (view === "device-preset-actions-official") return <CorOsDevicePresetScreen view="official-actions" />;
  if (view === "modes-official") return <CorOsOfficialModes onClose={onClose} />;
  if (view === "splitter-placement" || view === "splitter-editor" || view === "mixer-editor" || view === "empty-slot") return <CorOsRoutingScreen view={view} snapshot={snapshot} />;
  // `device-search` belongs to the device browser, not the capture library. It
  // used to be listed on the line below and matched there first, so
  // CorOsDeviceBrowserFixture's own `device-search` branch - the "Search
  // devices" panel with the query chip and the three Chief rows - could never
  // run, and `?screen=device-search` drew capture-search-results instead: the
  // same screen `device-search-results` already asks for.
  if (view === "device-search-entry" || view === "device-search-suggestions" || view === "device-search-results" || view === "device-favorites" || view === "device-recents" || view === "device-browser-neural-capture") return <CorOsCaptureLibrary view={view} />;
  if (view === "device-search" || view === "plugin-folders" || view === "plugin-list" || view === "plugin-list-ready" || view === "plugin-list-reference" || view === "plugin-list-unsaved" || view === "plugin-models" || view === "plugin-locked" || view === "plugin-refresh" || view === "plugin-refresh-ready") return <CorOsDeviceBrowserFixture view={view} />;
  if (view === "looper-editor") return <CorOsLooperEditor />;
  if (view === "device-presets" || view === "device-presets-user" || view === "device-preset-actions" || view === "device-preset-save") return <CorOsDevicePresetScreen save={view === "device-preset-save"} view={view === "device-presets-user" ? "user" : view === "device-preset-actions" ? "actions" : "factory"} />;
  if (view === "expression-parameter" || view === "expression-bypass") return <ExpressionChooser trim={view === "expression-parameter"} />;
  if (view === "stomp-assignment" || view === "scene-assignment") return <CorOsAssignmentScreen view={view} />;
  if (view === "block-context" || view === "block-context-bottom") return <CorOsBlockContext bottom={view === "block-context-bottom"} />;
  if (view === "capture-type" || view === "grid-scene-selector" || view.endsWith("-route-selector") || view.endsWith("-route-selector-top") || view.startsWith("grid-context-menu")) return <CorOsGridPopup view={view as GridPopupView} />;
  if (view === "directory-new-folder") return <CorOsDirectoryNameScreen />;
  if (view === "directory-filter") return <CorOsOfficialDirectory view="directory-nested" filter />;
  if (view === "directory-cloud-upload-physical") return <CorOsDirectoryFixture view="directory-cloud-upload" physicalContext currentRowOverride={1} />;
  // Both of these draw the Directory, so they match the `directory-` prefix
  // below and reach CorOsDirectoryFixture, which has no case for either name
  // and falls back to the Presets category - the whole screen wrong. They must
  // be routed before the prefix test, not added to its list, because the view
  // they need is the manual state of another view's name.
  if (view === "directory-captures-official") return <CorOsOfficialDirectory view="directory-captures" manualReference />;
  if (view === "directory-irs-official") return <CorOsOfficialDirectory view="directory-irs" manualReference />;
  if (view.startsWith("directory-")) return (["directory-presets", "directory-captures", "directory-irs", "directory-plugins", "directory-favorites", "directory-search-results", "directory-search-results-physical", "directory-nested", "directory-cloud-upload"] as string[]).includes(view) ? <CorOsOfficialDirectory view={view as OfficialDirectoryView} /> : <CorOsDirectoryFixture view={view as DirectoryFixtureView} />;
  if ((["capture-intro", "capture-monitoring", "capture-connect-out", "capture-connect-input-2", "capture-routing"] as string[]).includes(view)) return <CorOsCaptureConnections view={view as CaptureConnectionView} />;
  if (view.startsWith("capture-")) return <CorOsCaptureFixture view={view as CaptureFixtureView} />;
  if (view.startsWith("settings-")) return <CorOsSettingsFixture view={view as SettingsFixtureView} deviceInformation={deviceInformation} onClose={onClose} />;
  if (view === "modes") return <CorOsModesConfiguration onClose={onClose} />;
  if (view === "save-as") return <CorOsSaveAs onClose={onClose} />;
  if (view === "edit-details") return <CorOsPresetNameEditor snapshot={snapshot} onClose={onClose} />;
  return null;
}
