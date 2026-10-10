import { useState, type CSSProperties, type MouseEvent } from "react";
import { QC_PRESET_SLOT_COLORS } from "@qc-remote/core";
import type { PresetEntry, PresetList, PresetSnapshot } from "@qc-remote/client";
import { officialBlockVisual } from "./block-visuals";
import { gigPresetTileColor, gigSceneTileColor } from "./gig-view-colors";
import { QcDeviceGlyph } from "./device-glyph";
import { sceneDisplayLabel } from "./scene-display-label";
import { QcDirectoryIcon, QcModeGlyph, QcScreenGlyph, QcUiIcon } from "./theme-icons";
import { ModeGlyph } from "./coros-official-grid";
import { QcPillToggle } from "./qc-pill-toggle";
import { QcRotaryDial } from "./qc-rotary-dial";
import { QcSceneBadge, QcSceneSelector } from "./qc-shared-controls";
import { MODEL_PARAMETRIC_8 } from "./device-reference/coros-factory-content";
import { useMenuButton } from "./use-menu-button";

export function SceneTileTools({ scene, onEdit, onCopy, onSwap }: { scene?: number; onEdit?: (scene: number) => void; onCopy?: (scene: number) => void; onSwap?: (scene: number) => void } = {}) {
  const invoke = (callback: ((scene: number) => void) | undefined, event: MouseEvent<HTMLButtonElement>) => {
    event.stopPropagation();
    if (scene !== undefined) callback?.(scene);
  };
  return (
    <span className="gig-scene-tools" aria-hidden={onEdit || onCopy || onSwap ? undefined : "true"}>
      {onEdit && scene !== undefined
        ? <button type="button" aria-label={`Edit Scene ${String.fromCharCode(65 + scene)}`} onClick={(event) => invoke(onEdit, event)}><QcUiIcon kind="edit" /></button>
        : <QcUiIcon kind="edit" />}
      {onSwap && scene !== undefined
        ? <button type="button" aria-label={`Swap Scene ${String.fromCharCode(65 + scene)}`} onClick={(event) => invoke(onSwap, event)}><QcScreenGlyph kind="swap" /></button>
        : <span aria-hidden="true"><QcScreenGlyph kind="swap" /></span>}
      {onCopy && scene !== undefined
        ? <button type="button" aria-label={`Copy Scene ${String.fromCharCode(65 + scene)}`} onClick={(event) => invoke(onCopy, event)}><QcScreenGlyph kind="scene-copy" /></button>
        : <span aria-hidden="true"><QcScreenGlyph kind="scene-copy" /></span>}
    </span>
  );
}

function GigHybridModeGlyph({ modes = ["PRESET", "SCENE"] }: { modes?: Array<"PRESET" | "SCENE" | "STOMP"> }) {
  return (
    <span className="gig-hybrid-mode-glyph" aria-hidden="true">
      <QcModeGlyph mode={modes[0] ?? "PRESET"} />
      <i>+</i>
      <QcModeGlyph mode={modes[1] ?? "SCENE"} />
    </span>
  );
}

interface CorOsGigViewProps {
  snapshot: PresetSnapshot;
  presetList?: PresetList;
  onClose: () => void;
  onEditScene?: (scene: number) => void;
  onSelectScene?: (scene: number) => void;
  onSelectModeSlot?: (slot: 0 | 1 | 2) => void;
  onRecallPreset?: (entry: PresetEntry) => void;
  onPressFootswitch?: (index: number) => void;
  onEditBlock?: (blockId: string) => void;
  onCopyScene?: (scene: number) => void;
  onSwapScene?: (scene: number) => void;
  liveTuner?: boolean;
}

export function CorOsGigView({ snapshot, presetList, onClose, onEditScene, onSelectScene, onSelectModeSlot, onRecallPreset, onPressFootswitch, onEditBlock, onCopyScene, onSwapScene, liveTuner = false }: CorOsGigViewProps) {
  const [modeMenuOpen, setModeMenuOpen] = useState(false);
  const [sceneMenuOpen, setSceneMenuOpen] = useState(false);
  // The APG menu-button model (use-menu-button.ts). Focus enters the menu on
  // open and Tab closes it from the button, so the popovers' DOM position after
  // <header> no longer decides the keyboard order; they stay out of <header>,
  // whose "header button" rules would restyle every item.
  // onOpen keeps the two menus exclusive on every opening path, keyboard included.
  const modeMenu = useMenuButton({ open: modeMenuOpen, setOpen: setModeMenuOpen, onOpen: () => setSceneMenuOpen(false) });
  const sceneMenu = useMenuButton({ open: sceneMenuOpen, setOpen: setSceneMenuOpen, onOpen: () => setModeMenuOpen(false) });
  const gridBlocks = snapshot.blocks.filter((block) => block.column >= 0 && block.column < 8);
  const assignments = Array.from({ length: 8 }, (_, index) => snapshot.blocks.filter((block) => block.footswitch === index).sort((left, right) => (left.footswitchOrder ?? 0) - (right.footswitchOrder ?? 0))[0]);
  const sceneColors = ["var(--qc-palette-ff2421)", "var(--qc-palette-101c21)", "var(--qc-palette-292410)", "var(--qc-palette-291021)", "var(--qc-palette-102818)", "var(--qc-palette-291810)", "var(--qc-palette-181829)", "var(--qc-palette-102821)"];
  const sceneLetterColors = ["var(--qc-palette-e72018)", "var(--qc-palette-082029)", "var(--qc-palette-393408)", "var(--qc-palette-390c29)", "var(--qc-palette-083818)", "var(--qc-palette-391c08)", "var(--qc-palette-181c39)", "var(--qc-palette-083c29)"];
  // The unit's own eight slot letters. All eight sit on the panel's 5/6/5
  // ladders, which is how this palette differs from a near-miss set that
  // does not.
  const letterColors = QC_PRESET_SLOT_COLORS;
  const bankStart = Math.floor(snapshot.presetPosition / 8) * 8;
  const listedPresets = presetList?.setlistKey === snapshot.setlistKey ? presetList.presets : [];
  const presetTiles = Array.from({ length: 8 }, (_, index) => {
    const position = bankStart + index;
    const active = position === snapshot.presetPosition;
    const entry = listedPresets.find((candidate) => candidate.position === position);
    const name = active ? snapshot.presetName : (entry?.name ?? "Unsaved");
    return (
      <button type="button" key={index} className={`gig-preset-tile${active ? " is-active" : ""}`} disabled={!entry || entry.occupied === false} onClick={() => entry && onRecallPreset?.(entry)} style={active ? ({ "--gig-preset-tone": gigPresetTileColor(position) } as CSSProperties) : undefined}>
        <span>
          {Math.floor(position / 8) + 1}
          <b style={{ color: letterColors[index] }}>{String.fromCharCode(65 + index)}</b>
        </span>
        <strong>{name}</strong>
      </button>
    );
  });
  const sceneTiles = Array.from({ length: 8 }, (_, index) => (
    <article
      key={index}
      className={`gig-scene-tile${index === snapshot.activeScene ? " is-active" : ""}`}
      style={
        {
          "--gig-color": sceneColors[index],
          "--gig-letter": sceneLetterColors[index],
          "--gig-active-color": gigSceneTileColor(snapshot.sceneColors?.[index], index),
      } as CSSProperties
      }
      role="button"
      tabIndex={0}
      aria-label={`Select Scene ${String.fromCharCode(65 + index)}, ${sceneDisplayLabel(snapshot.scenes[index], index)}`}
      onClick={() => onSelectScene?.(index)}
      // Only a key on the tile itself selects its scene: Enter or Space on one of
      // its tool buttons bubbles here and must keep its own native activation.
      onKeyDown={(event) => { if (event.target !== event.currentTarget) return; if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onSelectScene?.(index); } }}
    >
      <SceneTileTools scene={index} onEdit={onEditScene} onCopy={onCopyScene} onSwap={onSwapScene} />
      <b>{String.fromCharCode(65 + index)}</b>
      <strong>{sceneDisplayLabel(snapshot.scenes[index], index)}</strong>
    </article>
  ));
  const stompTiles = assignments.map((assigned, index) => {
    const block = assigned ?? gridBlocks[index];
    const color = !block ? "var(--qc-route-glyph-surface)" : block.bypassed ? "var(--qc-palette-101c21)" : officialBlockVisual(block).color;
    return (
      <article key={index} className={`gig-stomp-tile${!block ? " is-empty" : block.bypassed ? " is-bypassed" : ""}`} style={{ "--gig-color": color } as CSSProperties}>
        <button type="button" className="gig-stomp-select" onClick={() => onPressFootswitch?.(index)} aria-label={`Footswitch ${String.fromCharCode(65 + index)}${block ? `, ${block.name}` : ", empty"}`}>
          {block && (
            <span className="gig-device-icon">
              {/* The span is the whole 70px block item, margin included: on
                  gig-view the unit cuts a black 70-square out of the lit stomp
                  tile at x 222..291 and paints the bordered tile inside it at
                  225..288. An 86 viewBox shrank that 64 tile to 57 and put its
                  2px border on 1.6 blended pixels. */}
              <svg viewBox="0 0 70 70" aria-hidden="true">
                <QcDeviceGlyph block={block} x={35} y={35} size={64} />
              </svg>
            </span>
          )}
          <b>{String.fromCharCode(65 + index)}</b>
          {block && <strong className={block.name === MODEL_PARAMETRIC_8 ? "is-compact" : ""}>{block.name}</strong>}
        </button>
        {block && (onEditBlock
          ? <button type="button" className="gig-edit" aria-label={`Edit ${block.name}`} onClick={() => onEditBlock(block.id)}><QcUiIcon kind="edit" /></button>
          : <span className="gig-edit" aria-hidden="true"><QcUiIcon kind="edit" /></span>)}
      </article>
    );
  });
  const tileForMode = (mode: "PRESET" | "SCENE" | "STOMP", index: number) => (mode === "PRESET" ? presetTiles[index] : mode === "SCENE" ? sceneTiles[index] : stompTiles[index]);
  const tiles = snapshot.mode === "HYBRID" ? Array.from({ length: 8 }, (_, index) => tileForMode(snapshot.footswitchModes?.[index < 4 ? 0 : 1] ?? (index < 4 ? "SCENE" : "STOMP"), index)) : Array.from({ length: 8 }, (_, index) => tileForMode(snapshot.mode as Exclude<PresetSnapshot["mode"], "HYBRID" | "UNKNOWN">, index));
  return (
    <section className={`coros-gig-view${liveTuner ? " has-live-tuner" : ""}`} aria-label={liveTuner ? "Gig View with Live Tuner" : "Gig View"}>
      <header>
        <strong>
          {snapshot.presetLocation} {snapshot.presetName}
        </strong>
        <button type="button" className="gig-mode" aria-label={`Select mode; current mode ${snapshot.mode}`} {...modeMenu.triggerProps} onClick={() => { setSceneMenuOpen(false); modeMenu.toggle(); }}>
          {snapshot.mode === "HYBRID" ? (
            <GigHybridModeGlyph modes={snapshot.footswitchModes} />
          ) : (
            <ModeGlyph mode={snapshot.mode} />
          )}
        </button>
        {/* The slot is bar height; the badge inside it is the unit's 24x24
            square. The letter is drawn once, by this element - it used to be a
            text node plus a `content: attr(data-scene)` pseudo-element, and the
            two became two rows of the slot's grid, which put the badge 5px
            below the centre the unit draws it at. */}
        <button type="button" className="gig-scene" aria-label={`Select scene; current scene ${String.fromCharCode(65 + snapshot.activeScene)}`} {...sceneMenu.triggerProps} onClick={() => { setModeMenuOpen(false); sceneMenu.toggle(); }}>
          <QcSceneBadge letter={String.fromCharCode(65 + snapshot.activeScene)} />
        </button>
        <button aria-label="Close Gig View" onClick={onClose}>
          <QcUiIcon kind="check" />
        </button>
      </header>
      {modeMenuOpen && <div className="gig-selector-popover gig-mode-selector" {...modeMenu.menuProps} aria-label="Modes">{(snapshot.modeSlots ?? (["PRESET", "SCENE", "STOMP"] as const).map((mode, slot) => ({ slot: slot as 0 | 1 | 2, label: mode, mode }))).map((entry) => <button type="button" key={`${entry.slot}-${entry.label}`} className={snapshot.mode === entry.mode ? "is-active" : ""} {...modeMenu.itemProps(() => onSelectModeSlot?.(entry.slot))}><QcModeGlyph mode={entry.mode} /><span>{entry.label}</span></button>)}</div>}
      {sceneMenuOpen && <div className="gig-selector-popover gig-scene-selector" {...sceneMenu.menuProps} aria-label="Scenes">{snapshot.scenes.map((scene, index) => <button type="button" key={`${index}-${scene}`} className={snapshot.activeScene === index ? "is-active" : ""} {...sceneMenu.itemProps(() => onSelectScene?.(index))}><QcSceneBadge letter={String.fromCharCode(65 + index)} /><span>{sceneDisplayLabel(scene, index)}</span></button>)}</div>}
      {liveTuner && <div className="live-tuner-strip" aria-label="Live Tuner dormant" />}
      <div className={`gig-tiles is-${snapshot.mode.toLowerCase()}`}>{tiles}</div>
    </section>
  );
}

export function CorOsTuner({ onClose, liveTuner = false, official = false }: { onClose: () => void; liveTuner?: boolean; official?: boolean }) {
  return (
    <section className={`coros-tuner${official ? " tuner-official" : ""}`} aria-label={liveTuner ? "Tuner with Live Tuner enabled" : "Tuner"}>
      <header>
        <span>Tuner</span>
        <button aria-label="Close Tuner" onClick={onClose}>
          <QcUiIcon kind="check" />
        </button>
      </header>
      <div className="tuner-scale">
        <span>-50</span>
        {official && (
          <>
            <em className="tuner-flat">D#/Eb</em>
            <em className="tuner-note">E</em>
            <em className="tuner-sharp">F</em>
            <strong className="tuner-reading">-1.4</strong>
            <small className="tuner-left-arrow">
              <QcUiIcon kind="next" />
            </small>
            <small className="tuner-right-arrow">
              <QcUiIcon kind="previous" />
            </small>
          </>
        )}
        <i />
        <b />
        <span>+50</span>
      </div>
      <footer>
        <section>
          <span>FREQ [Hz]</span>
          <div className="tuner-frequency">
            {/* tuner reads 211.86 degrees here. */}
            <QcRotaryDial value={0.2847} origin={0.5} />
            <strong>{official ? "440.0" : "422.0"}</strong>
          </div>
        </section>
        <section>
          <button>
            INPUT 1{" "}
            <b>
              <QcUiIcon kind="down" />
            </b>
          </button>
        </section>
        <section>
          <button className="tuner-muted">MUTED</button>
        </section>
        <section>
          <span>LIVE TUNER</span>
          <QcPillToggle index={liveTuner || official ? 0 : 1} />
          <span className="tuner-choice">
            <b>Yes</b>
          </span>
          <span className="tuner-choice">
            <b>No</b>
          </span>
        </section>
      </footer>
    </section>
  );
}

export function CorOsTempo({ bpm, onClose, official = false }: { bpm: number; onClose: () => void; official?: boolean }) {
  return (
    <section className={`coros-tempo${official ? " tempo-official" : ""}`} aria-label="Tempo and Metronome">
      <header>
        <span>Tempo</span>
        <QcSceneSelector className="tempo-scene" letter="A" />
        <button aria-label="Close Tempo and Metronome" onClick={onClose}>
          <QcUiIcon kind="check" />
        </button>
      </header>
      <div className="tempo-display">
        <strong>{Math.round(bpm)}</strong>
        <small>BPM = QUARTERS</small>
        <div>
          {[0, 1, 2, 3].map((beat) => (
            <i key={beat} className={beat === (official ? 1 : 0) ? "is-active" : ""} />
          ))}
        </div>
      </div>
      <div className="tempo-controls">
        <section className="tempo-control is-tempo">
          <span>TEMPO</span>
          {/* The tempo, volume and pan dials sit at 160.98, 299.84 and 270.00
              degrees respectively. */}
          <QcRotaryDial className="tempo-dial" value={0.0962} />
          <strong>{Math.round(bpm)}<span className="tempo-unit"> BPM</span></strong>
        </section>
        <section className="tempo-control is-radio">
          <span>
            MODE <b className="tempo-info">i</b>
          </span>
          <QcPillToggle index={official ? 0 : 1} />
          <span className={official ? "is-active" : undefined}>Global</span>
          <span className={official ? undefined : "is-active"}>Preset</span>
        </section>
        <section className="tempo-control is-radio">
          <span>TEMPO LED</span>
          <QcPillToggle index={0} />
          <span className="is-active">On</span>
          <span>Off</span>
        </section>
        <section className="tempo-control is-volume">
          <span>VOLUME</span>
          <QcRotaryDial className="tempo-dial" value={0.6105} />
          <strong>0.0<span className="tempo-unit"> dB</span></strong>
        </section>
        <section className="tempo-control is-mute">
          <button>UNMUTE</button>
        </section>
        <section className="tempo-control is-pan">
          <span>PAN</span>
          <QcRotaryDial className="tempo-dial" value={0.5} origin={0.5} />
          <strong>C</strong>
        </section>
        {[
          ["T/SIGNATURE", "4/4"],
          ["SUBDIVISIONS", "1/4"],
          ["SOUND", "BLIP"],
          ["ROUTING", "MULTI"],
        ].map(([label, value]) => (
          <section className="tempo-control is-select" key={label}>
            <span>{label}</span>
            <button>
              {value}
              <b>
                <QcUiIcon kind="down" />
              </b>
            </button>
          </section>
        ))}
      </div>
    </section>
  );
}

export function CorOsMidiOut({ onClose }: { onClose: () => void }) {
  return (
    <section className="coros-midi-out" aria-label="Preset MIDI Out">
      <header>
        <span>Preset MIDI Out settings</span>
        <button className="midi-trash" aria-label="Clear MIDI assignments">
          <QcDirectoryIcon kind="trash" />
        </button>
        <button aria-label="Close Preset MIDI Out" onClick={onClose}>
          <QcUiIcon kind="check" />
        </button>
      </header>
      <div className="midi-assignment-surface">
        <section className="midi-footswitches">
          <button className="midi-preset-load">
            ON PRESET LOAD
            <br />
            MESSAGE
          </button>
          {["A", "B", "C", "D", "E", "F", "G", "H"].map((label) => (
            <button className="midi-switch" key={label}>
              <i />
              {label}
            </button>
          ))}
        </section>
        <section className="midi-expression">
          <label>
            <span>EXP 1</span>
            <div>
              <i />
            </div>
          </label>
          <label>
            <span>EXP 2</span>
            <div>
              <i />
            </div>
          </label>
        </section>
      </div>
    </section>
  );
}
