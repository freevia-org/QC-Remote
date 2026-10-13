import { sceneLetter } from "@qc-remote/core";
import { useEffect, useRef, useState, type FocusEvent } from "react";
import { useDialogFocusTrap } from "./qc-shared-controls";
import { focusUserOpenedTarget } from "./user-open-focus";
import { QcUiIcon } from "./theme-icons";
import { SCENE_COMMAND_PROMPT } from "./device-reference/coros-screen-text";
import "./official-scene-command.css";

export interface SceneCommandOverlayProps {
  /** The scene names to offer as a destination, in device order (A, B, C...). */
  scenes: readonly string[];
  /** The scene the command copies/swaps FROM - always the active scene. */
  sourceScene: number;
  /** false = "Copy Scene X", true = "Swap Scene X". */
  swap: boolean;
  pending: boolean;
  /**
   * The panel is the unit's own dialog: its title, its prompt and CANCEL, nothing else. The destination is a footswitch
   * press on the unit, so no destination list is drawn and the app never picks or confirms one.
   */
  mirrorsUnit?: boolean;
  onSelect(destination: number): void;
  onCancel(): void;
  /** The data-qc-open-target focus returns to when the user dismisses the overlay; none when omitted. */
  returnFocusTarget?: string;
}

/**
 * The live "Copy Scene X"/"Swap Scene X" destination-selection screen,
 * reached from the Grid's own context menu (coros-ui.ts GRID_CONTEXT_MENU,
 * actions "copy-scene"/"swap-scene") - never from an arbitrary source
 * picker, since on the real device the source is always whichever scene is
 * active. Ported from coros-screen-fixtures.tsx's CorOsRemainingFixture
 * "fixture-copy-scene"/"fixture-swap-scene" branch.
 *
 * Only that fixture's <aside> panel is ported (not its `.scene-command-grid`
 * ghost): the ghost is the same live Grid redrawn at reduced opacity, and the
 * real Grid QuadCortexSurface already renders behind this command overlay.
 * Its geometry lives in
 * official-scene-command.css, which this file imports directly.
 *
 * A physical footswitch press selects the destination scene on the real
 * unit; there is no desktop equivalent, so this panel offers a scrollable
 * scene list instead (visual treatment adapted from surface-shell.css's own
 * production .scene-dropdown). That list, and the scrolling, are this app's
 * own affordance; only the panel's box (position/size/color/typography)
 * matches the device screen.
 */
export function CorOsSceneCommand(props: SceneCommandOverlayProps) {
  const { scenes, sourceScene, swap, pending, mirrorsUnit = false, onSelect, onCancel, returnFocusTarget } = props;
  const verb = swap ? "Swap" : "Copy";
  // A modal dialog by the shared rules (qc-shared-controls useDialogFocusTrap):
  // Escape cancels, unless a command is in flight and CANCEL is disabled too;
  // Tab stays inside, on the panel itself while every control is disabled;
  // there is no default answer, so Enter on the panel itself does nothing.
  // Entry focus goes to the first destination, or to the panel while every
  // destination is disabled, and only while focus is still where the opening
  // left it: on the body (the Grid menu item is gone) or on the control that
  // opened it (Gig View's Copy/Swap button). Focus the user moved elsewhere
  // before the frame stays.
  // The user's own dismissal returns focus to the control that opened the
  // overlay: the one the host names (the Grid menu's button, whose menu item
  // is gone), otherwise the opener captured at mount when it is still on
  // screen (Gig View's Copy/Swap button). A close the device drives (the
  // command finished) moves nothing.
  const opener = useRef<Element | null>(null);
  const dismiss = () => {
    if (pending) return;
    const returnTo = opener.current;
    onCancel();
    if (returnFocusTarget) focusUserOpenedTarget(returnFocusTarget);
    else if (returnTo instanceof HTMLElement && returnTo !== document.body) {
      requestAnimationFrame(() => {
        const active = document.activeElement;
        if (returnTo.isConnected && (!active || active === document.body)) returnTo.focus();
      });
    }
  };
  const { dialogRef, onKeyDown } = useDialogFocusTrap<HTMLElement>({
    focusable: "button:not([disabled])",
    onCancel: dismiss,
    onEnter: () => undefined
  });
  useEffect(() => {
    const openedFrom = document.activeElement;
    opener.current = openedFrom;
    const frame = requestAnimationFrame(() => {
      const panel = dialogRef.current;
      const active = document.activeElement;
      if (!panel || (active && active !== document.body && active !== openedFrom)) return;
      (panel.querySelector<HTMLElement>(".scene-command-destinations button:not([disabled])") ?? panel).focus();
    });
    return () => cancelAnimationFrame(frame);
  }, []);
  // A command in flight disables every control, and the browser drops focus
  // from the button that just became disabled, so Tab would leave the dialog.
  // The panel takes focus back only when the control that lost it is the
  // dialog's own and was disabled: `owned` is the panel's last focused control,
  // and it is forgotten when focus leaves it any other way (to another
  // element, or to the body while the control was still enabled), so focus
  // the user moved away on purpose is never taken back.
  const owned = useRef<HTMLElement | null>(null);
  const trackFocus = (event: FocusEvent<HTMLElement>) => { owned.current = event.target; };
  const trackBlur = (event: FocusEvent<HTMLElement>) => {
    if (event.relatedTarget || !event.target.matches(":disabled")) owned.current = null;
  };
  useEffect(() => {
    if (!pending) return;
    const panel = dialogRef.current;
    const control = owned.current;
    if (!panel || !control || !panel.contains(control) || !control.matches(":disabled")) return;
    const active = document.activeElement;
    if (!active || active === document.body || active === control) panel.focus();
  }, [pending]);
  return <div className={mirrorsUnit ? "scene-command-overlay is-unit-dialog" : "scene-command-overlay"} role="presentation">
    <div className="scene-command-scrim" aria-hidden="true" />
    <section className={mirrorsUnit ? "scene-command-panel is-unit-dialog" : "scene-command-panel"} role="dialog" aria-modal="true" aria-labelledby="scene-command-title" tabIndex={-1} ref={dialogRef} onKeyDown={onKeyDown} onFocus={trackFocus} onBlur={trackBlur}>
      <h1 id="scene-command-title">{verb} Scene {sceneLetter(sourceScene)}</h1>
      <p>{SCENE_COMMAND_PROMPT}</p>
      {/* Plain buttons in a labelled group: a listbox would promise arrow-key movement this list does not have. */}
      {mirrorsUnit ? null : <div className="scene-command-destinations qc-scroll-rail" role="group" aria-label="Destination scene">
        {scenes.map((scene, index) => index === sourceScene ? null : <button type="button" key={scene} disabled={pending} onClick={() => onSelect(index)}><span>{sceneLetter(index)}</span>{scene}</button>)}
      </div>}
      <footer><button type="button" disabled={pending} onClick={dismiss}>CANCEL</button></footer>
    </section>
  </div>;
}

export interface SceneQuickEditPopoverProps {
  sourceScene: number;
  setSourceScene(scene: number): void;
  scenes: readonly string[];
  label: string;
  setLabel(label: string): void;
  color: number;
  setColor(color: number): void;
  colors: readonly string[];
  pending: boolean;
  saveLabel(): void;
  saveColor(): void;
  onClose(): void;
  /** Render as the device's current 800x480 screen, not as an app overlay. */
  inDeviceDisplay?: boolean;
}

// The row of QWERTY letters, with the superscript digit CorOsSceneLabelDialog
// draws on each one.
const SLD_ROW_1: ReadonlyArray<readonly [string, string]> = [["Q", "1"], ["W", "2"], ["E", "3"], ["R", "4"], ["T", "5"], ["Y", "6"], ["U", "7"], ["I", "8"], ["O", "9"], ["P", "0"]];
const SLD_ROW_2 = ["A", "S", "D", "F", "G", "H", "J", "K", "L"] as const;
const SLD_ROW_3 = ["Z", "X", "C", "V", "B", "N", "M"] as const;

/**
 * The device-accurate scene name/color editor, reached on both hosts exactly
 * like the physical unit: open Gig View and tap SceneTileTools' "Edit" icon
 * on a scene tile. It replaces Gig View inside the 800x480 device display:
 * a header row of a clear (X) button, a red "confirm color" button, 7
 * selectable swatches, and a save (disk) button; a large editable name in
 * the scene's own color with a full-height cursor bar; a scene letter
 * selector (prev/badge/next); and a full QWERTY keyboard with a
 * numeric-superscript top row.
 *
 * Two judgment calls, both deliberate:
 *
 * 1. Colors. The header's 8 button-sized, evenly-pitched slots line up as
 *    [red-with-check, blue, yellow, magenta, green, orange, purple, teal].
 *    The checkmark is a fixed, separately-styled "apply the chosen color"
 *    control (wired to the existing `saveColor`, unchanged), not an
 *    indicator of which color is currently selected, and `colors[0]` (red)
 *    is not offered as a plain selectable swatch on this screen -
 *    `colors.slice(1)` are. This keeps every prop this component already had
 *    working exactly as before; nothing was added to use-scene-workflow.ts
 *    to reach it.
 * 2. The keyboard is real, not a restyled native input: typing keys mutate
 *    `label` directly (still committed only by Save, matching the disk
 *    button and the keyboard's own "Save" key - both call the same
 *    `saveLabel`). Every icon on it (close, apply-color, save,
 *    previous/next, backspace) is the shared registry's existing
 *    `QcUiIcon`, never a one-off character or hand-drawn SVG - the theme
 *    test suite (`screen controls use shared vectors...`) forbids exactly
 *    that shortcut. Two scoped simplifications remain, both because tracing
 *    a *new* device icon asset through the shared vector registry for one
 *    control each is out of proportion here: Shift reuses the registry's existing
 *    "up" selector triangle (there is no registered shift glyph) as a
 *    persistent case toggle. The 123 key switches the same on-screen keyboard
 *    to digits and punctuation; it does not delegate to a platform keyboard.
 */
export function SceneQuickEditPopover(props: SceneQuickEditPopoverProps) {
  const { sourceScene, setSourceScene, scenes, label, setLabel, color, setColor, colors, pending, saveLabel, saveColor, onClose, inDeviceDisplay = false } = props;
  const [shift, setShift] = useState(false);
  const [numeric, setNumeric] = useState(false);
  const previewColor = colors[color] ?? colors[0];
  const insert = (text: string) => { if (pending) return; setLabel((label + text).slice(0, 32)); };
  const backspace = () => { if (pending) return; setLabel(label.slice(0, -1)); };
  const typeLetter = (letter: string) => insert(shift ? letter.toUpperCase() : letter.toLowerCase());
  const canPrev = sourceScene > 0;
  const canNext = sourceScene < scenes.length - 1;
  return <div className={`scene-label-dialog-overlay${inDeviceDisplay ? " is-device-screen" : ""}`} role="presentation">
    {!inDeviceDisplay && <div className="scene-label-dialog-scrim" aria-hidden="true" />}
    <section className="scene-label-dialog-panel" role="dialog" aria-modal="true" aria-label={`Scene ${sceneLetter(sourceScene)} label and color`}>
      <header className="scene-label-dialog-toolbar">
        <button type="button" className="sld-btn sld-clear" disabled={pending} aria-label="Close" onClick={onClose}><QcUiIcon kind="close" /></button>
        <div className="sld-swatches" role="listbox" aria-label="Scene color">
          <button type="button" className="sld-btn sld-confirm" disabled={pending} aria-label="Apply color" onClick={saveColor}><QcUiIcon kind="check" /></button>
          {colors.slice(1).map((hex, i) => { const index = i + 1; return <button type="button" key={hex} role="option" aria-selected={index === color} disabled={pending} className="sld-btn sld-swatch" style={{ backgroundColor: hex }} aria-label={`Color ${index + 1}`} onClick={() => setColor(index)} />; })}
        </div>
        <button type="button" className="sld-btn sld-save" disabled={pending} aria-label="Save name" onClick={saveLabel}><QcUiIcon kind="save-as" /></button>
      </header>
      <div className="scene-label-dialog-name">
        <input maxLength={32} value={label} disabled={pending} aria-label="Scene name" style={{ color: previewColor, caretColor: previewColor }} onChange={(event) => setLabel(event.target.value)} />
      </div>
      <div className="scene-label-dialog-selector">
        <button type="button" className="sld-nav" disabled={pending || !canPrev} aria-label="Previous scene" onClick={() => setSourceScene(sourceScene - 1)}><QcUiIcon kind="previous" /></button>
        <span className="sld-badge">{sceneLetter(sourceScene)}</span>
        <button type="button" className="sld-nav" disabled={pending || !canNext} aria-label="Next scene" onClick={() => setSourceScene(sourceScene + 1)}><QcUiIcon kind="next" /></button>
      </div>
      <div className="scene-label-dialog-keyboard">
        {numeric ? <>
          <div className="sld-kb-row">{"1234567890".split("").map((value) => <button type="button" key={value} className="sld-key" disabled={pending} onClick={() => insert(value)}>{value}</button>)}</div>
          <div className="sld-kb-row sld-kb-row-2">{["-", "_", "/", "\\", "(", ")", "&", "@", "+"].map((value) => <button type="button" key={value} className="sld-key" disabled={pending} onClick={() => insert(value)}>{value}</button>)}</div>
          <div className="sld-kb-row">{["!", "?", ":", ";", "'", '"', "#"].map((value) => <button type="button" key={value} className="sld-key" disabled={pending} onClick={() => insert(value)}>{value}</button>)}<button type="button" className="sld-key sld-key-wide" aria-label="Backspace" disabled={pending} onClick={backspace}><QcUiIcon kind="backspace" /></button></div>
        </> : <>
          <div className="sld-kb-row">{SLD_ROW_1.map(([letter, digit]) => <button type="button" key={letter} className="sld-key" disabled={pending} onClick={() => typeLetter(letter)}><sup>{digit}</sup>{letter}</button>)}</div>
          <div className="sld-kb-row sld-kb-row-2">{SLD_ROW_2.map((letter) => <button type="button" key={letter} className="sld-key" disabled={pending} onClick={() => typeLetter(letter)}>{letter}</button>)}</div>
          <div className="sld-kb-row"><button type="button" className="sld-key sld-key-wide" aria-pressed={shift} aria-label="Shift" disabled={pending} onClick={() => setShift((value) => !value)}><QcUiIcon kind="up" /></button>{SLD_ROW_3.map((letter) => <button type="button" key={letter} className="sld-key" disabled={pending} onClick={() => typeLetter(letter)}>{letter}</button>)}<button type="button" className="sld-key sld-key-wide" aria-label="Backspace" disabled={pending} onClick={backspace}><QcUiIcon kind="backspace" /></button></div>
        </>}
        <div className="sld-kb-row">
          <button type="button" className="sld-key sld-key-wide" aria-pressed={numeric} disabled={pending} onClick={() => setNumeric((value) => !value)}>{numeric ? "ABC" : "123"}</button>
          <button type="button" className="sld-key" disabled={pending} onClick={() => insert(",")}>,</button>
          <button type="button" className="sld-key sld-key-space" disabled={pending} onClick={() => insert(" ")}>Space</button>
          <button type="button" className="sld-key" disabled={pending} onClick={() => insert(".")}>.</button>
          <button type="button" className="sld-key sld-key-wide" disabled={pending} onClick={saveLabel}>Save</button>
        </div>
      </div>
    </section>
  </div>;
}
