import { lazy, Suspense, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { QC_GRID_COLUMNS, QC_GRID_ROWS, QC_OUTPUT_ROUTES, type CpuColumnLoad, type GridBlock, type LaneControl, type LibraryEntry, type PresetEntry, type PresetList, type PresetSnapshot } from "@qc-remote/client";
import { footswitchLeds, QC_PRESET_SLOT_COLORS, routePickerGroup, routePickerLabel, sceneLetter as sceneLabel, type QcSurfaceAction } from "@qc-remote/core";
import type { FormFactorManifest, HardwareControl, SkinManifest } from "@qc-remote/form-factors";
import { QC_BRAND, QC_COLORS, QC_SCREEN_ICON_VECTORS, QC_TYPOGRAPHY } from "@qc-remote/theme";
import { officialBlockVisual } from "./block-visuals";
import { useMenuButton } from "./use-menu-button";
import { masterVolumeAngleDegrees } from "./master-volume-angle";
import { CorOsParameterEditor, type CorOsParameterEditorProps } from "./parameter-editor";
import { CorOsAddDevicePicker, type CorOsAddDevicePickerState } from "./add-device-picker";
import { CorOsExpressionAssignment, type ExpressionAssignmentState } from "./expression-assignment";
import { CorOsLaneControlPanel, type LaneControlPanelState } from "./lane-control-panel";
import { fixtureSnapshot, type CorOsScreenView } from "./coros-screen-fixture-data";
import { CorOsCpuMonitorPanel } from "./coros-cpu-monitor";
import type { CorOsDeviceInformation } from "./coros-reference-settings-pane";
import { QcDeviceGlyph } from "./device-glyph";
import { sceneDisplayLabel } from "./scene-display-label";
import { clientPointToGrid, createGridTapMachine, exceedsDragThreshold, GRID_LONG_PRESS_MS, gridClickIsPointerEcho, gridDropTarget, resolveGridCellFromPoint, resolveGridGestureRelease, type GridCellHit, type GridDropTarget } from "./grid-drag-geometry";
import { parameterEditorAccent, parameterEditorControlSlots, parameterEditorPageSize } from "./parameter-model";
import { SceneQuickEditPopover, type SceneQuickEditPopoverProps } from "./scene-editor";
import { QcHardwareIcon, QcModeGlyph, QcRouteGlyph, QcScreenHeaderGlyph, QcScreenVectorGlyph, QcUiIcon } from "./theme-icons";
import { QC_HOLD_DELAY_MS, useHoldGesture } from "./use-hold-gesture";
import { hardwareRotaryPixelsPerStep, useRotaryGesture } from "./use-rotary-gesture";
import { CorOsDirectory } from "./coros-directory";
import type { PresetDirectoryState } from "./directory-library-model";
import { GRID_CONTEXT_MENU, gridBlocksByRow, mixAnchorX, openSplitPath, PRESET_LOCATION_TRACKING, presetTitleLayout, presetTitlePresentation, rejoinSplitPath, routedPortIsPlugged, rowHasVisibleSignalRail, splitAnchorX, type CorOsContextAction } from "./coros-ui";
// The Grid path never loads the screen fixtures, so the shared element library
// is imported here too - and first, for the same reason it is imported first
// there: it is the base the screen files refine, not a patch applied over them.
import "./qc-screen-elements.css";
import "./surface-shell.css";
import "./live-surface.css";
import "./reference-parameter-editor.css";
import "./qc-device-typography.css";
import "./display-frame.css";

const CorOsScreenFixture = lazy(() => import("./coros-screen-fixtures").then((module) => ({ default: module.CorOsScreenFixture })));

export type { CorOsContextAction } from "./coros-ui";
export type { CorOsScreenView } from "./coros-screen-fixture-data";
export type { PresetDirectoryState } from "./directory-library-model";

export type HardwareAction = QcSurfaceAction;

export interface CorOsRoutingPickerState {
  row: number;
  /** "split"/"mix" is the split-placement flow's own picker - same surface,
   *  a plain column list instead of the input/output port groups below. */
  side: "input" | "output" | "split" | "mix";
  options: readonly (readonly [number, string])[];
  value: number;
  disabled: boolean;
  onSelect: (value: number) => void;
  onClose: () => void;
}

export interface CorOsSavePresetState {
  open: boolean;
  name: string;
  disabled: boolean;
  onNameChange: (name: string) => void;
  onSave: () => void;
  onCancel: () => void;
}

interface QuadCortexSurfaceProps {
  formFactor: FormFactorManifest;
  snapshot: PresetSnapshot;
  selectedBlockId?: string;
  skin: SkinManifest;
  onAction: (action: HardwareAction) => void;
  onOpenPreset: () => void;
  onUndo: () => void;
  canUndo: boolean;
  undoLabel?: string;
  onSave: () => void;
  onOpenRouting: (row: number, side: "input" | "output") => void;
  onRefresh: () => void;
  presetDirectory?: PresetDirectoryState;
  gigPresetList?: PresetList;
  routingPicker?: CorOsRoutingPickerState;
  /** The live Add device browser over the Grid, opened on an empty cell. */
  addDevicePicker?: CorOsAddDevicePickerState;
  expressionAssignment?: ExpressionAssignmentState;
  laneControlEditor?: LaneControlPanelState;
  savePreset?: CorOsSavePresetState;
  parameterEditor?: CorOsParameterEditorProps;
  onContextAction?: (action: CorOsContextAction, returnFocusTarget?: "grid-menu") => void;
  /** Commands which are visible in the device menu but cannot be performed
   * safely by this host. Disabled rows remain visible and explain why. */
  disabledContextActions?: Partial<Record<CorOsContextAction | "save-as", string>>;
  screenView?: CorOsScreenView;
  /** A stateful production screen rendered inside the physical display.
   * Visual corpus fixtures remain available through screenView, but live
   * screens use this slot so every enabled element owns a semantic action. */
  screenContent?: ReactNode;
  /** The CPU Monitor, which the unit lays over its normal Grid rather than
   * replacing it. The Grid underneath is the same one the surface always
   * draws (title, port labels, blocks); this adds the readout panel and each
   * block's load fill. Ignored while a screenView or screenContent replaces
   * the Grid. */
  cpuMonitor?: { cpuLoad: number; cpuColumns: readonly CpuColumnLoad[]; returnFocusTarget?: "grid-menu"; onClose: () => void };
  deviceInformation?: CorOsDeviceInformation;
  /** "hardware" draws the unit's panel around the display; "display" draws the
   * display alone. The display itself - its 5:3 shape, its 800x480 canvas and
   * how that is scaled - is identical in both (display-frame.css); a host only
   * chooses how much of the unit to show and what box to offer it. */
  presentation?: "hardware" | "display";
  onCloseScreen?: () => void;
  onOpenGigView?: () => void;
  onOpenIoView?: () => void;
  onEditScene?: (scene: number) => void;
  onRecallGigPreset?: (entry: PresetEntry) => void;
  onCopyGigScene?: (scene: number) => void;
  onSwapGigScene?: (scene: number) => void;
  /** Forward a gesture made on a device-owned screen (currently Tuner and
   * I/O Settings) to the same 800x480 coordinates on the physical QC. */
  onDeviceScreenGesture?: (x: number, y: number, toX: number, toY: number, kind: "tap" | "long-tap" | "swipe") => void;
  sceneEditor?: Omit<SceneQuickEditPopoverProps, "inDeviceDisplay">;
  onMasterVolumeInteractionStart?: () => void;
  onMasterVolumeInteractionEnd?: () => void;
}

export function QcHardwareSwitch({ role, label, ariaLabel, active, assigned = false, accent, compact = false, pulseBpm, pulseEpochMs, readout, holdDelayMs = QC_HOLD_DELAY_MS, doubleTapDelayMs = 300, onTap, onDoubleTap, onHold, onContactChange, onAction }: {
  role: string; label: ReactNode; ariaLabel?: string; active?: boolean; assigned?: boolean; accent?: string; compact?: boolean; pulseBpm?: number; pulseEpochMs?: number; readout?: string;
  holdDelayMs?: number; doubleTapDelayMs?: number;
  onTap?: () => void; onDoubleTap?: () => void; onHold?: () => void; onContactChange?: (pressed: boolean) => void;
  onAction: (action: HardwareAction) => void;
}) {
  const hideValueTimer = useRef<number | undefined>(undefined);
  const pendingTapTimer = useRef<number | undefined>(undefined);
  // A pointer press and a keyboard press are separate gestures, each with its own hold.
  const rotated = useRef(false);
  const keyboardPressed = useRef(false);
  const pointerHold = useHoldGesture({ delayMs: holdDelayMs, onHold });
  const keyboardHold = useHoldGesture({ delayMs: holdDelayMs, onHold });
  const [encoderValue, setEncoderValue] = useState(50);
  const [showValue, setShowValue] = useState(false);
  const [pressed, setPressed] = useState(false);
  const rotate = (delta: number) => {
    setEncoderValue((current) => Math.max(0, Math.min(100, current + delta)));
    setShowValue(true);
    if (hideValueTimer.current !== undefined) window.clearTimeout(hideValueTimer.current);
    hideValueTimer.current = window.setTimeout(() => setShowValue(false), 900);
    onAction({ kind: "rotate", role, delta });
  };
  const emitTap = () => {
    if (onTap) onTap();
    else {
      onAction({ kind: "switch", role, phase: "press" });
      onAction({ kind: "switch", role, phase: "release" });
    }
  };
  const completeTap = () => {
    if (!onDoubleTap) { emitTap(); return; }
    if (pendingTapTimer.current !== undefined) {
      window.clearTimeout(pendingTapTimer.current);
      pendingTapTimer.current = undefined;
      onDoubleTap();
      return;
    }
    pendingTapTimer.current = window.setTimeout(() => {
      pendingTapTimer.current = undefined;
      emitTap();
    }, doubleTapDelayMs);
  };
  useEffect(() => () => {
    if (hideValueTimer.current !== undefined) window.clearTimeout(hideValueTimer.current);
    if (pendingTapTimer.current !== undefined) window.clearTimeout(pendingTapTimer.current);
  }, []);
  const encoder = useRotaryGesture({
    pixelsPerStep: hardwareRotaryPixelsPerStep,
    preventPointerDefault: true,
    onPointerStart: (event) => {
      rotated.current = false;
      setPressed(true);
      onContactChange?.(true);
      pointerHold.start(undefined, event);
    },
    onRotate: (steps, source) => {
      if (source === "drag") {
        // A press that has become a hold no longer turns the encoder, and
        // turning it first means the press is neither a hold nor a tap.
        if (pointerHold.fired()) return;
        rotated.current = true;
        pointerHold.cancel();
      }
      rotate(steps);
    },
    onPointerEnd: (_event, cancelled) => {
      pointerHold.cancel();
      setPressed(false);
      onContactChange?.(false);
      if (!cancelled && !rotated.current && !pointerHold.fired()) completeTap();
    }
  });
  const keyboardDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    encoder.onKeyDown(event);
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    if (event.repeat) return;
    setPressed(true);
    onContactChange?.(true);
    keyboardPressed.current = true;
    keyboardHold.start();
  };
  const keyboardUp = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    const pressedByKey = keyboardPressed.current;
    keyboardPressed.current = false;
    keyboardHold.cancel();
    setPressed(false);
    onContactChange?.(false);
    if (pressedByKey && !keyboardHold.fired()) completeTap();
  };
  const tempoPeriodMs = pulseBpm ? 60_000 / pulseBpm : undefined;
  const led = useRef<HTMLSpanElement>(null);
  // `animation-delay` is measured from the animation's own start time, not from
  // the moment the style lands, so a delay computed against the device's beat
  // origin drifts by however long the lamp had already been animating - the
  // lamp then ran at the right BPM but off the device's beat. Anchoring the
  // animation's `startTime` to that origin puts iteration zero on the device's
  // beat and keeps every later beat locked to it.
  useEffect(() => {
    const node = led.current;
    if (!node || tempoPeriodMs === undefined || pulseEpochMs === undefined) return;
    if (typeof node.getAnimations !== "function") return;
    const originMs = pulseEpochMs - performance.timeOrigin;
    for (const animation of node.getAnimations({ subtree: true })) {
      if ((animation as CSSAnimation).animationName !== "qc-tempo-led-pulse") continue;
      if (animation.startTime === originMs) continue;
      try { animation.startTime = originMs; } catch { /* A finished animation cannot be re-anchored. */ }
    }
  }, [active, pulseEpochMs, tempoPeriodMs]);
  const accessibleLabel = ariaLabel ?? (typeof label === "string" ? label : role);
  return <button
    className={`hardware-switch${active ? " is-active" : ""}${pressed ? " is-pressed" : ""}${assigned ? " is-assigned" : ""}${compact ? " is-compact" : ""}${pulseBpm ? " is-tempo-pulse" : ""}`}
    style={{ "--switch-accent": accent ?? "var(--accent)", "--tempo-period": tempoPeriodMs ? `${tempoPeriodMs}ms` : undefined } as CSSProperties}
    aria-label={`${accessibleLabel} encoder footswitch; encoder ${encoderValue} percent`} aria-pressed={Boolean(active || pressed)}
    title={`${accessibleLabel}: tap to press${onDoubleTap ? "; double-tap for its alternate action" : ""}${onHold ? "; hold for its hold action" : ""}; drag vertically, use the mouse wheel, or press arrow keys to rotate`}
    onPointerDown={encoder.onPointerDown}
    onPointerMove={encoder.onPointerMove}
    onPointerUp={encoder.onPointerUp} onPointerCancel={encoder.onPointerCancel} onKeyDown={keyboardDown} onKeyUp={keyboardUp} onBlur={() => { if (keyboardPressed.current) { keyboardHold.cancel(); keyboardPressed.current = false; setPressed(false); onContactChange?.(false); } }} onWheel={encoder.onWheel}
  >
    <span ref={led} className="switch-led" aria-hidden="true" />
    <span className="switch-ring" aria-hidden="true"><span className="switch-cap" /><span className={`rotation-readout${showValue || readout !== undefined ? " is-visible" : ""}`}>{readout ?? encoderValue}</span></span>
    <span className="switch-label">{label}</span>
  </button>;
}

export function QcMasterVolumeKnob({ value, readout, onAction, onInteractionStart, onInteractionEnd }: { value: number; readout?: string; onAction: (action: HardwareAction) => void; onInteractionStart?: () => void; onInteractionEnd?: () => void }) {
  const knob = useRotaryGesture({
    pixelsPerStep: hardwareRotaryPixelsPerStep,
    preventPointerDefault: true,
    onRotate: (delta) => onAction({ kind: "rotate", role: "master-volume", delta }),
    onPointerStart: () => onInteractionStart?.(),
    onPointerEnd: () => onInteractionEnd?.()
  });
  // Fitted to 16 photographs of the physical knob; see master-volume-angle.ts.
  const angle = masterVolumeAngleDegrees(value);
  return <button className="volume-knob" role="slider" aria-orientation="vertical" style={{ "--volume-angle": `${angle}deg` } as CSSProperties} aria-label="Master volume knob" aria-valuenow={value} aria-valuemin={0} aria-valuemax={100} aria-valuetext={`${value} percent`} onPointerDown={knob.onPointerDown} onPointerMove={knob.onPointerMove} onPointerUp={knob.onPointerUp} onPointerCancel={knob.onPointerCancel} onKeyDown={knob.onKeyDown} onWheel={knob.onWheel}><span className="volume-pointer" />{readout !== undefined && <span className="rotation-readout is-visible">{readout}</span>}</button>;
}

function MasterVolume({ value, onAction, onInteractionStart, onInteractionEnd }: { value: number; onAction: (action: HardwareAction) => void; onInteractionStart?: () => void; onInteractionEnd?: () => void }) {
  return <div className="master-volume">
    <button className="power-button" aria-label="Power and lock menu" onClick={() => onAction({ kind: "switch", role: "power", phase: "release" })}><QcHardwareIcon kind="power-led" className="power-icon" /></button>
    <QcMasterVolumeKnob value={value} onAction={onAction} onInteractionStart={onInteractionStart} onInteractionEnd={onInteractionEnd} />
    <strong>VOLUME</strong>
  </div>;
}

/** The Grid drawing's own units: the canvas viewBox the hit layer is laid over. */
const GRID_DRAWING = { width: 800, height: 480 } as const;

/** One pointer gesture on one block; lives in a ref so pointer moves never re-render by themselves. */
interface GridBlockGesture {
  pointerId: number;
  block: GridBlock;
  startClientX: number;
  startClientY: number;
  startRow: number;
  startColumn: number;
  phase: "pressed" | "dragging";
  longPressFired: boolean;
  longPressTimer?: ReturnType<typeof setTimeout>;
}

/** What the dragged block and its drop cell look like right now; the only drag state that renders. */
interface GridDragPreview {
  blockId: string;
  dx: number;
  dy: number;
  over: GridCellHit;
  target: GridDropTarget;
}

/**
 * One hold-vs-tap gesture on an input/output rail label (the element the
 * manual's INPUT & OUTPUT BLOCKS section calls an input or output block: a
 * tap reassigns its source, and a tap-and-hold opens its parameter editor).
 * Mirrors
 * `GridBlockGesture` above - a ref so pointer moves never re-render by
 * themselves - but there is no drag destination here, only the two outcomes
 * the manual describes.
 */
interface RailPortGesture {
  pointerId: number;
  row: number;
  side: "input" | "output";
  startClientX: number;
  startClientY: number;
  longPressFired: boolean;
  longPressTimer?: ReturnType<typeof setTimeout>;
}

/** The manual's own trigger for splitting a row into a parallel path is a
 * tap-and-hold on any empty slot (see rowActionStops' own comment below) -
 * not a dedicated on-screen marker, which is what this used
 * to draw (an always-visible round "+" button at the first split column, on
 * every even row with no existing split - invented, and not part of the
 * device's own UI; see the removed splitPlacementMark). A hold on an already-empty
 * cell's own add-block hit target now carries this instead, the same
 * tap-vs-hold split beginRailGesture already does for Input Gate/Lane Output. */
interface SplitPlacementGesture {
  pointerId: number;
  row: number;
  column: number;
  startClientX: number;
  startClientY: number;
  longPressFired: boolean;
  longPressTimer?: ReturnType<typeof setTimeout>;
}

/**
 * Where the Grid's contextual menu was left scrolled, kept for the life of the
 * app rather than of the Grid component (the Grid unmounts under Settings and
 * every other full screen). The unit does not reset it: walked on the device,
 * the menu opened at its bottom (Delete Preset ... Settings) on two successive
 * opens from the Grid, the second after Settings had been used and closed - the
 * list had been scrolled down to reach Settings the first time. The corpus's
 * top frame (grid-context-menu.png, FILE first) is the first open of a session.
 * Whether the unit also forgets it on a preset change or after a timeout is not
 * established by any frame.
 */
let gridMenuScrollTop = 0;
// The drawn thumb (remaining-fixtures-fixes.css, .coros-screen-menu::after) follows the menu's real scroll
// geometry. It is drawn only while the menu overflows and no scrollbar takes layout width.
// KNOWN LIMITATION (unmeasured): the nativeBar test below only detects a scrollbar that takes layout width.
// An overlay native scrollbar takes none and may still paint its own thumb, so a duplicate thumb is possible
// until it is measured on Windows (compare the thumb strip with and without the ::after, or read
// offsetWidth/clientWidth and the computed ::-webkit-scrollbar / scrollbar-width of .coros-screen-menu).
const syncGridMenuThumb = (menu: HTMLElement) => {
  const overflow = menu.scrollHeight - menu.clientHeight;
  const nativeBar = menu.offsetWidth - menu.clientWidth - 2 * menu.clientLeft > 0;
  if (overflow <= 0 || nativeBar) { menu.removeAttribute("data-qc-thumb"); return; }
  menu.style.setProperty("--qc-menu-scroll-top", `${menu.scrollTop}px`);
  menu.style.setProperty("--qc-menu-thumb-ratio", String(menu.clientHeight / menu.scrollHeight));
  menu.style.setProperty("--qc-menu-thumb-progress", String(Math.min(1, Math.max(0, menu.scrollTop / overflow))));
  menu.setAttribute("data-qc-thumb", "");
};
// Module-level and identity-stable, like the two functions it replaces, so React attaches it once per menu
// element. The observer lives in the returned ref cleanup (React 19), so every attached menu owns its own
// observer and no instance can disconnect another's; a ref callback invoked with null has nothing to clean.
const restoreGridMenuScroll = (element: HTMLDivElement | null) => {
  if (!element) return undefined;
  element.scrollTop = gridMenuScrollTop;
  syncGridMenuThumb(element);
  // The menu is cqw-sized, so a screen rescale changes clientHeight without any scroll event.
  if (typeof ResizeObserver === "undefined") return undefined;
  const observer = new ResizeObserver(() => syncGridMenuThumb(element));
  observer.observe(element);
  return () => observer.disconnect();
};
const rememberGridMenuScroll = (event: { currentTarget: HTMLDivElement }) => { gridMenuScrollTop = event.currentTarget.scrollTop; syncGridMenuThumb(event.currentTarget); };

type GridActions = Pick<QuadCortexSurfaceProps,
  | "onAction"
  | "onOpenPreset"
  | "onUndo"
  | "canUndo"
  | "undoLabel"
  | "onSave"
  | "onOpenRouting"
  | "onRefresh"
  | "presetDirectory"
  | "routingPicker"
  | "addDevicePicker"
  | "savePreset"
  | "onContextAction"
  | "disabledContextActions"
>;

function CorOsGrid({ snapshot, presetSlotAccent, selectedBlockId, gridActions, cpuColumns }: Pick<QuadCortexSurfaceProps, "snapshot" | "selectedBlockId"> & { presetSlotAccent: string; gridActions: GridActions; cpuColumns?: readonly CpuColumnLoad[] }) {
  const { onAction, onOpenPreset, onUndo, canUndo, undoLabel, onSave, onOpenRouting, onRefresh, presetDirectory, routingPicker, addDevicePicker, savePreset, onContextAction, disabledContextActions } = gridActions;
  const [sceneMenuOpen, setSceneMenuOpen] = useState(false);
  const [screenMenuOpen, setScreenMenuOpen] = useState(false);
  const [modeMenuOpen, setModeMenuOpen] = useState(false);
  // One keyboard model for the three Grid menus (APG menu button): see use-menu-button.ts.
  // onOpen keeps them exclusive on every opening path, keyboard included.
  const sceneMenu = useMenuButton({ open: sceneMenuOpen, setOpen: setSceneMenuOpen, onOpen: () => { setScreenMenuOpen(false); setModeMenuOpen(false); } });
  const modeMenu = useMenuButton({ open: modeMenuOpen, setOpen: setModeMenuOpen, onOpen: () => { setSceneMenuOpen(false); setScreenMenuOpen(false); } });
  const gridMenu = useMenuButton({ open: screenMenuOpen, setOpen: setScreenMenuOpen, onOpen: () => { setSceneMenuOpen(false); setModeMenuOpen(false); } });
  const [, setDeviceFontReady] = useState(false);
  const actionsLayer = useRef<HTMLDivElement>(null);
  const blockGesture = useRef<GridBlockGesture | null>(null);
  const railGesture = useRef<RailPortGesture | null>(null);
  const splitPlacementGesture = useRef<SplitPlacementGesture | null>(null);
  const modeHoldTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const modeHoldFired = useRef(false);
  const lastPointerReleaseAt = useRef(Number.NEGATIVE_INFINITY);
  const tapMachine = useRef(createGridTapMachine());
  const tapTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const [dragPreview, setDragPreview] = useState<GridDragPreview | null>(null);
  useEffect(() => () => {
    clearTimeout(blockGesture.current?.longPressTimer);
    clearTimeout(railGesture.current?.longPressTimer);
    clearTimeout(splitPlacementGesture.current?.longPressTimer);
    clearTimeout(modeHoldTimer.current);
    clearTimeout(tapTimer.current);
    tapMachine.current.cancel();
  }, []);
  useEffect(() => {
    let active = true;
    if (typeof document === "undefined" || !document.fonts) return undefined;
    void Promise.all([
      document.fonts.load(`800 68px ${QC_TYPOGRAPHY.devicePlain}`, snapshot.presetLocation),
      document.fonts.load(`800 68px ${QC_TYPOGRAPHY.devicePlain}`, snapshot.presetName)
    ]).then(() => { if (active) setDeviceFontReady(true); });
    return () => { active = false; };
  }, [snapshot.presetLocation, snapshot.presetName]);
  const routingPickerIsPort = routingPicker?.side === "input" || routingPicker?.side === "output";
  const routePickerGroups = routingPicker && routingPickerIsPort ? (routingPicker.side === "input" ? ["MONO", "STEREO", ""] : ["STEREO", "MONO", "OTHER"]).map((name) => ({
    name,
    options: routingPicker.options.filter(([value]) => routePickerGroup(routingPicker.side as "input" | "output", value) === name)
  })).filter((group) => group.options.length) : [];
  const runGridMenuAction = (action: (typeof GRID_CONTEXT_MENU)[number]["action"], returnFocusTarget?: "grid-menu") => {
    setScreenMenuOpen(false);
    if (action === "save-as") onSave();
    else onContextAction?.(action, returnFocusTarget);
  };
  const columns = [101, 187, 272, 357, 443, 529, 615, 701];
  const rowY = [147, 241, 335, 429];
  const screenBlocks = snapshot.blocks.filter((block) => block.row >= 0 && block.row < QC_GRID_ROWS && block.column >= 0 && block.column < QC_GRID_COLUMNS);
  const tabBlocksByRow = gridBlocksByRow(screenBlocks);
  const gridLayout = { columns, rows: rowY };
  const cellOccupied = (row: number, column: number) => screenBlocks.some((block) => block.row === row && block.column === column);
  const gridPoint = (event: PointerEvent<HTMLElement>) => {
    const rect = actionsLayer.current?.getBoundingClientRect();
    if (!rect?.width || !rect.height) return undefined;
    return { rect, point: clientPointToGrid(event.clientX, event.clientY, rect, GRID_DRAWING) };
  };
  // Pointer gestures on a block. A plain tap is left to the button's own click,
  // so Enter and Space keep selecting from the keyboard; the gesture only marks
  // the pointer consumed when it did something else with it.
  const beginBlockGesture = (event: PointerEvent<HTMLButtonElement>, block: GridBlock) => {
    if (event.button !== 0 || blockGesture.current) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture?.(event.pointerId);
    const gesture: GridBlockGesture = { pointerId: event.pointerId, block, startClientX: event.clientX, startClientY: event.clientY, startRow: block.row, startColumn: block.column, phase: "pressed", longPressFired: false };
    gesture.longPressTimer = setTimeout(() => {
      if (blockGesture.current !== gesture || gesture.phase !== "pressed") return;
      // CorOS lifts the block into move mode on a hold (corpus: long_amp walk
      // frame): it follows the pointer, empty cells show drop slots and a trash
      // zone appears. It does not toggle bypass - a double tap does that.
      gesture.longPressFired = true;
      gesture.phase = "dragging";
      setDragPreview({ blockId: block.id, dx: 0, dy: 0, over: { row: gesture.startRow, column: gesture.startColumn, inBounds: true }, target: "none" });
    }, GRID_LONG_PRESS_MS);
    blockGesture.current = gesture;
  };
  const trackBlockGesture = (event: PointerEvent<HTMLButtonElement>) => {
    const gesture = blockGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    const dx = event.clientX - gesture.startClientX;
    const dy = event.clientY - gesture.startClientY;
    event.preventDefault();
    if (gesture.phase === "pressed") {
      if (!exceedsDragThreshold(dx, dy)) return;
      clearTimeout(gesture.longPressTimer);
      gesture.phase = "dragging";
    }
    const mapped = gridPoint(event);
    if (!mapped) return;
    const over = resolveGridCellFromPoint(mapped.point.x, mapped.point.y, gridLayout);
    setDragPreview({
      blockId: gesture.block.id,
      dx: dx * GRID_DRAWING.width / mapped.rect.width,
      dy: dy * GRID_DRAWING.height / mapped.rect.height,
      over,
      target: gridDropTarget(over, { row: gesture.startRow, column: gesture.startColumn }, cellOccupied)
    });
  };
  const endBlockGesture = (event: PointerEvent<HTMLButtonElement>, cancelled = false) => {
    const gesture = blockGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    clearTimeout(gesture.longPressTimer);
    blockGesture.current = null;
    setDragPreview(null);
    if (event.currentTarget.hasPointerCapture?.(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    lastPointerReleaseAt.current = performance.now();
    if (cancelled) return;
    const mapped = gridPoint(event);
    const outcome = resolveGridGestureRelease({
      blockId: gesture.block.id,
      phase: gesture.phase,
      longPressFired: gesture.longPressFired,
      startRow: gesture.startRow,
      startColumn: gesture.startColumn,
      over: mapped && resolveGridCellFromPoint(mapped.point.x, mapped.point.y, gridLayout),
      occupied: cellOccupied
    });
    if (outcome.action) onAction(outcome.action);
    if (!outcome.consumed) registerTap(gesture.block.id);
  };
  // A plain tap waits out the double-tap window before it selects; a second
  // tap on the same block inside it toggles bypass instead and never selects.
  const registerTap = (blockId: string) => {
    const machine = tapMachine.current;
    clearTimeout(tapTimer.current);
    for (const action of machine.tap(blockId, performance.now())) onAction(action);
    const until = machine.pendingUntil();
    if (until === undefined) return;
    tapTimer.current = setTimeout(() => {
      for (const action of machine.expire()) onAction(action);
    }, Math.max(0, until - performance.now()));
  };
  const sceneLetter = sceneLabel(snapshot.activeScene);
  const presetBank = snapshot.presetLocation.slice(0, -1);
  const presetSlot = snapshot.presetLocation.slice(-1);
  const presetLocation = `${presetBank}${presetSlot}`;
  const titlePresentation = presetTitlePresentation(snapshot.presetName, snapshot.dirty);
  const presetTitle = titlePresentation.text;
  const measureHeaderText = (text: string, italic = false) => {
    if (typeof document === "undefined") return text.length * 40;
    const context = document.createElement("canvas").getContext("2d");
    if (!context) return text.length * 40;
    context.font = `${italic ? "italic " : ""}800 68px ${QC_TYPOGRAPHY.devicePlain}`;
    return context.measureText(text).width;
  };
  // The block's rendered advance: the font's own plus the unit's tracking,
  // which CSS applies after the last glyph as well as between them.
  const presetLocationWidth = measureHeaderText(presetLocation) + PRESET_LOCATION_TRACKING * presetLocation.length;
  // The official header reserves a clear gutter before Undo. Derive the title
  // width from its actual start so multi-digit banks cannot push it underneath
  // the icon while retaining the QC's natural inline bank/slot/name flow.
  const presetTitleWidthAtFullSize = measureHeaderText(presetTitle, titlePresentation.italic);
  const {
    start: presetTitleStart,
    maxWidth: presetTitleMaxWidth,
    fontSize: presetTitleFontSize,
    squeeze: squeezePresetTitle,
    baseline: presetTitleBaseline
  } = presetTitleLayout(presetLocationWidth, presetTitleWidthAtFullSize);
  // The header's mode chip is centred, not left-anchored: the unit puts the
  // 24-unit PRESET box at (656,60) on capture-type and the 26-unit STOMP/SCENE
  // box at (655,59) on grid-base and the route selectors — one centre, two
  // sizes — so a fixed translate can only ever serve one of them.
  const modeGlyphVector = QC_SCREEN_ICON_VECTORS[`mode.${snapshot.mode.toLowerCase()}` as keyof typeof QC_SCREEN_ICON_VECTORS];
  const modeGlyphOrigin = { x: 668 - modeGlyphVector.width / 2, y: 72 - modeGlyphVector.height / 2 };
  const routes = rowY.map((_, row) => snapshot.routes.find((route) => route.row === row));
  const displayInput = (row: number) => {
    const input = routes[row]?.input;
    if (input && input !== "Internal") return input;
    if (row === 2 && ["Row 3", "Rows 3/4"].includes(routes[0]?.output ?? "")) return "Prev. Row";
    return "+";
  };
  const displayOutput = (row: number) => !routes[row]?.output || routes[row]?.output === "Internal" ? "+" : routes[row]?.output;
  // INPUT GATE CONTROL/LANE OUTPUT CONTROL, per the manual's INPUT & OUTPUT
  // BLOCKS section: Input Gate Control exists for every assigned input block,
  // and Lane Output Control for every assigned output block unless that output
  // is routed to another Row. The qualifying condition is the row's *port* being configured, independent of
  // whether the row's chain has an actual device block placed - so this reuses
  // displayInput/displayOutput's own "is this port assigned" check (both
  // return the "+" rail glyph exactly when unassigned) rather than deriving a
  // second one from `routes[row]` by hand.
  const laneControlAvailable = (row: number, control: LaneControl) => {
    if (control === "inputGate") return displayInput(row) !== "+";
    if (displayOutput(row) === "+") return false;
    // Outputs routed to another Row are excluded: QC_OUTPUT_ROUTES marks Row 3/
    // Row 4/Row 3/4 with group "OTHER" - the same bucket "Internal" (already
    // excluded above) sits in - so this reuses that table's own grouping
    // instead of repeating the three route labels by hand.
    return QC_OUTPUT_ROUTES.find((route) => route.label === routes[row]?.output)?.group !== "OTHER";
  };
  const connectionMark = (side: "input" | "output", row: number) => {
    const routeId = side === "input" ? routes[row]?.inputId : routes[row]?.outputId;
    if (!routedPortIsPlugged(side, routeId, snapshot.ioPorts)) return null;
    const x = side === "input" ? 19 : 759;
    return <path key={`${side}-connection-${row}`} d={`M${x} ${rowY[row] - 33}h22`} stroke={QC_COLORS.device.connectionMark} strokeWidth="3" strokeLinecap="round" />;
  };
  const routeLines = (label: string | undefined) => {
    const value = label ?? "+";
    const words = value.split(" ");
    return words.length > 1 ? [words[0], words.slice(1).join(" ")] : [value];
  };
  // The unit draws one add mark and `interface.add` owns it: a 22x22 cross of
  // 2-unit bars inside the 24-unit asset box. The box origin is therefore the
  // port centre less 12. This used to be two stroked paths with round caps,
  // in their own colour rather than the registry's `reference.utilityMark`.
  const railMark = (x: number, y: number) => <QcScreenVectorGlyph icon="interface.add" x={x - 12} y={y - 12} />;
  const railLabel = (label: string | undefined, x: number, y: number) => {
    const lines = routeLines(label);
    if (label === "+") return railMark(x, y);
    const firstY = y - (lines.length - 1) * 8.5;
    return <text x={x} y={firstY} fill={QC_COLORS.reference.routeText} stroke="none" fontFamily={QC_TYPOGRAPHY.deviceRoute} fontWeight="400" fontSize="14.5">{lines.map((line, index) => <tspan key={`${line}-${index}`} x={x} dy={index ? 17 : 0}>{line}</tspan>)}</text>;
  };
  // A port item keeps three pixels of screen colour around it, exactly as a block
  // tile does (TILE_MARGIN in device-glyph.tsx). grid-base fills the input rail to
  // x 51 and leaves 52..54 black before the cable starts at 55; the far side stops
  // the cable at 746 and opens the output rail at 750.
  const rowRail = (row: number) => rowHasVisibleSignalRail(tabBlocksByRow[row].length, routes[row])
    ? <path key={`row-${row}`} d={`M55 ${rowY[row]}H747`} />
    : null;
  const routeToken = (kind: "S" | "M", x: number, y: number, row: number) => {
    const color = kind === "S" ? QC_COLORS.category.equalizer : QC_COLORS.category.synth;
    const node = kind === "S" ? "splitter" : "mixer";
    const selected = selectedBlockId === `routing-${row}-${node}`;
    return <g key={`${kind}-${row}`}>
      {selected && <circle cx={x} cy={y} r="18" fill="none" stroke={QC_COLORS.reference.primaryText} strokeWidth="2" />}
      <circle cx={x} cy={y} r="15" fill={QC_COLORS.reference.screen} stroke="none" />
      <circle cx={x} cy={y} r="13" fill={color} stroke="none" />
      <text x={x} y={y + 5.5} textAnchor="middle" fill={QC_COLORS.reference.primaryText} stroke="none" fontFamily={QC_TYPOGRAPHY.devicePlain} fontWeight="700" fontSize="16">{kind}</text>
    </g>;
  };
  const splitPath = (row: number) => {
    const route = routes[row];
    if (route?.splitColumn === undefined || row >= rowY.length - 1) return null;
    const splitX = splitAnchorX(route.splitColumn);
    const rejoins = route.mixColumn !== undefined && route.mixColumn >= 0;
    const mixX = rejoins ? mixAnchorX(route.mixColumn!) : 748;
    return <g key={`split-${row}`} fill="none" stroke={QC_COLORS.device.splitPath} strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
      <path d={openSplitPath(splitX, rowY[row], rowY[row + 1])} />
      {rejoins && <path d={rejoinSplitPath(mixX, rowY[row], rowY[row + 1])} />}
      {routeToken("S", splitX, rowY[row], row)}
      {rejoins && routeToken("M", mixX, rowY[row], row)}
    </g>;
  };
  // Input Gate/Lane Output are real, per-row controls (the CorOS 1.4.0
  // release notes add an Input Gate to every input block; the CorOS 1.3.0
  // notes add Mute and Solo to the output lane blocks), and the manual's
  // INPUT & OUTPUT BLOCKS section settles both their trigger gesture and their
  // availability: a tap on an input or output block reassigns its source, a
  // tap-and-hold opens its parameter editor, Input Gate Control exists for
  // every assigned input block, and Lane Output Control for every assigned
  // output block unless that output is routed to another Row. So
  // the live trigger is a hold on the row's existing input/output rail label -
  // the same element a plain tap already opens the port picker on, below -
  // not a separate tap target elsewhere on the row. beginRailGesture below
  // wires that up with GRID_LONG_PRESS_MS, the same threshold
  // beginBlockGesture already uses for hold-to-bypass, not a second invented
  // timing. laneControlAvailable above (next to displayInput/displayOutput,
  // which it reuses) is what beginRailGesture and rowActionStops below both
  // gate on.
  //
  // The Grid draws NO marker on the rail chip while a lane control is open:
  // the device dims the whole Grid behind the panel and leaves the "In 1" /
  // "Multi Out" chips as dimmed text. An earlier build stamped a bypassed
  // Utility tile at the chip centre (a misreading: the dim tile in
  // input-gate-control is the row's first BLOCK, drawn by the block layer),
  // which covered the chip text with a stray glyph. Nothing may draw over the
  // rail chips.
  // A tap on an input/output rail label assigns a different source
  // (onOpenRouting, below - unchanged); a tap-and-hold instead opens its
  // parameter editor, per the manual passage quoted above. Mirrors
  // beginBlockGesture/trackBlockGesture/endBlockGesture's own hold-vs-tap
  // machinery, including their shared `lastPointerReleaseAt` ref and the
  // `gridClickIsPointerEcho` guard on the fallback `onClick` (for keyboard
  // activation, which has no preceding pointerdown/up) - so the native click
  // that follows every real pointer release does not also fire, the same way
  // it already does not for a block.
  const laneControlOfSide = (side: "input" | "output"): LaneControl => side === "input" ? "inputGate" : "laneOutput";
  const beginRailGesture = (event: PointerEvent<HTMLButtonElement>, row: number, side: "input" | "output") => {
    if (event.button !== 0 || railGesture.current) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    const gesture: RailPortGesture = { pointerId: event.pointerId, row, side, startClientX: event.clientX, startClientY: event.clientY, longPressFired: false };
    // No parameter editor to open when the port is unassigned (the manual
    // offers these controls only on assigned input/output blocks) - so only an
    // assigned port's hold is even armed; an unassigned port's rail label
    // keeps behaving exactly like a plain tap target, same as before this
    // change.
    if (laneControlAvailable(row, laneControlOfSide(side))) {
      gesture.longPressTimer = setTimeout(() => {
        if (railGesture.current !== gesture) return;
        gesture.longPressFired = true;
        onAction({ kind: "select-lane-control", row, control: laneControlOfSide(side) });
      }, GRID_LONG_PRESS_MS);
    }
    railGesture.current = gesture;
  };
  const trackRailGesture = (event: PointerEvent<HTMLButtonElement>) => {
    const gesture = railGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId || gesture.longPressFired || !gesture.longPressTimer) return;
    if (exceedsDragThreshold(event.clientX - gesture.startClientX, event.clientY - gesture.startClientY)) {
      clearTimeout(gesture.longPressTimer);
      gesture.longPressTimer = undefined;
    }
  };
  const endRailGesture = (event: PointerEvent<HTMLButtonElement>, cancelled = false) => {
    const gesture = railGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    clearTimeout(gesture.longPressTimer);
    railGesture.current = null;
    if (event.currentTarget.hasPointerCapture?.(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    // A hold already dispatched select-lane-control above; a cancelled
    // gesture (pointercancel) dispatches nothing at all. A plain tap is left
    // to onClick so Android's synthetic click cannot land on the picker that
    // pointerup has just mounted and immediately dismiss it.
    if (cancelled || gesture.longPressFired) lastPointerReleaseAt.current = performance.now();
  };
  // Same tap-vs-hold shape as beginRailGesture/trackRailGesture/
  // endRailGesture just above: a hold dispatches place-split, a plain tap
  // falls through to the cell's own onClick (add-block-cell), exactly what
  // it already did before this row could also be held.
  const beginSplitPlacementGesture = (event: PointerEvent<HTMLButtonElement>, row: number, column: number) => {
    if (event.button !== 0 || splitPlacementGesture.current) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    const gesture: SplitPlacementGesture = { pointerId: event.pointerId, row, column, startClientX: event.clientX, startClientY: event.clientY, longPressFired: false };
    gesture.longPressTimer = setTimeout(() => {
      if (splitPlacementGesture.current !== gesture) return;
      gesture.longPressFired = true;
      onAction({ kind: "place-split", row });
    }, GRID_LONG_PRESS_MS);
    splitPlacementGesture.current = gesture;
  };
  const trackSplitPlacementGesture = (event: PointerEvent<HTMLButtonElement>) => {
    const gesture = splitPlacementGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId || gesture.longPressFired || !gesture.longPressTimer) return;
    if (exceedsDragThreshold(event.clientX - gesture.startClientX, event.clientY - gesture.startClientY)) {
      clearTimeout(gesture.longPressTimer);
      gesture.longPressTimer = undefined;
    }
  };
  const endSplitPlacementGesture = (event: PointerEvent<HTMLButtonElement>, cancelled = false) => {
    const gesture = splitPlacementGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    clearTimeout(gesture.longPressTimer);
    splitPlacementGesture.current = null;
    if (event.currentTarget.hasPointerCapture?.(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    // A hold already dispatched place-split above; a cancelled gesture
    // dispatches nothing. A plain tap is left to the cell's onClick so the
    // chooser is mounted after, not during, Android's pointer release.
    if (cancelled || gesture.longPressFired) lastPointerReleaseAt.current = performance.now();
  };
  const renderBlock = (block: GridBlock) => {
    const cx = columns[block.column];
    const cy = rowY[block.row];
    const selected = selectedBlockId === block.id;
    const drag = dragPreview?.blockId === block.id ? dragPreview : undefined;
    return <g key={block.id} className={drag ? `coros-grid-drag-glyph${drag.target === "remove" ? " is-removing" : ""}` : undefined} transform={drag ? `translate(${drag.dx} ${drag.dy})` : undefined}>
      <QcDeviceGlyph block={block} x={cx} y={cy} selected={selected} cpuLoad={cpuColumns?.[block.row * QC_GRID_COLUMNS + block.column]?.load} cpuCore={cpuColumns?.[block.row * QC_GRID_COLUMNS + block.column]?.core} />
    </g>;
  };
  // Input Gate/Lane Output no longer get a separate tap target here: the
  // manual's own trigger is a hold on the row's input/output rail label
  // (beginRailGesture, wired to that label below), not a second element
  // elsewhere on the row.
  const rowActionStops = (row: number) => {
    const route = routes[row];
    const stops = [...tabBlocksByRow[row].map((block) => {
      const drag = dragPreview?.blockId === block.id ? dragPreview : undefined;
      return {
        key: block.id,
        x: columns[block.column],
        element: <button
          key={block.id}
          className={`coros-vector-block-hit${drag ? " is-dragging" : ""}${drag?.target === "remove" ? " is-removing" : ""}`}
          style={{ left: `${(columns[block.column] + (drag?.dx ?? 0)) / 8}%`, top: `${(rowY[block.row] + (drag?.dy ?? 0)) / 4.8}%` }}
          title={`Row ${block.row + 1}, ${block.name}`} aria-label={`Row ${block.row + 1}, ${block.name}`} aria-pressed={selectedBlockId === block.id}
          onPointerDown={(event) => beginBlockGesture(event, block)}
          onPointerMove={trackBlockGesture}
          onPointerUp={(event) => endBlockGesture(event)}
          onPointerCancel={(event) => endBlockGesture(event, true)}
          draggable={false}
          onDragStart={(event) => event.preventDefault()}
          onContextMenu={(event) => { if (blockGesture.current) event.preventDefault(); }}
          onClick={() => { if (!gridClickIsPointerEcho(performance.now(), lastPointerReleaseAt.current)) onAction({ kind: "select-block", blockId: block.id }); }}
        />
      };
    })];
    if (route?.splitColumn !== undefined && row < rowY.length - 1) {
      const splitX = splitAnchorX(route.splitColumn);
      const splitId = `routing-${row}-splitter`;
      stops.push({
        key: splitId,
        x: splitX,
        element: <button key={splitId} className="coros-vector-route-node-hit" style={{ left: `${splitX / 8}%`, top: `${rowY[row] / 4.8}%` }} title={`Open row ${row + 1} Splitter parameters`} aria-label={`Row ${row + 1} Splitter parameters`} aria-pressed={selectedBlockId === splitId} onClick={() => onAction({ kind: "select-routing-node", row, node: "splitter" })} />
      });
      if (route.mixColumn !== undefined && route.mixColumn >= 0) {
        const mixX = mixAnchorX(route.mixColumn);
        const mixId = `routing-${row}-mixer`;
        stops.push({
          key: mixId,
          x: mixX,
          element: <button key={mixId} className="coros-vector-route-node-hit" style={{ left: `${mixX / 8}%`, top: `${rowY[row] / 4.8}%` }} title={`Open row ${row + 1} Mixer parameters`} aria-label={`Row ${row + 1} Mixer parameters`} aria-pressed={selectedBlockId === mixId} onClick={() => onAction({ kind: "select-routing-node", row, node: "mixer" })} />
        });
      }
    }
    // A split always spans a row and the row below it (the former Branch
    // <select> dialog's own gating, route.row % 2 === 0), so only an even row
    // with no existing split can start one.
    const splitPlacementEligible = route?.splitColumn === undefined && row % 2 === 0 && row < rowY.length - 1;
    for (const column of columns.keys()) {
      if (tabBlocksByRow[row].some((block) => block.column === column)) continue;
      const cellId = `add-${row}-${column}`;
      const dropTarget = dragPreview?.target === "move" && dragPreview.over.row === row && dragPreview.over.column === column;
      // The whole-preset-empty tile at (0,0) is drawn from its own fixed rect
      // (66,119, 64x64 -> centre 98,151 - see the `!screenBlocks.length`
      // block below), not from `columns`/`rowY`: a different reference frame
      // than the shared per-cell geometry, never reconciled. Match the hit
      // region to what is actually drawn there instead of the general grid.
      const isEmptyPresetTile = row === 0 && column === 0 && !screenBlocks.length;
      const hitX = isEmptyPresetTile ? 98 : columns[column];
      const hitY = isEmptyPresetTile ? 151 : rowY[row];
      // A tap on an empty cell adds a block, same as always. On an eligible
      // row the manual's own path to splitting it is a tap-and-hold on any
      // empty slot - not a dedicated on-screen marker, which is what
      // this cell used to draw one
      // of alongside itself (an always-visible round "+" at the first split
      // column, on every even row - invented, and not part of the device's
      // own UI). beginSplitPlacementGesture below wires that hold in directly,
      // the same tap-vs-hold split beginRailGesture already does for Input
      // Gate/Lane Output.
      stops.push({
        key: cellId,
        x: hitX,
        element: <button
          key={cellId}
          className="coros-vector-add-hit"
          data-drop-target={dropTarget || undefined}
          style={{ left: `${hitX / 8}%`, top: `${hitY / 4.8}%` }}
          title={splitPlacementEligible ? `Add a block at row ${row + 1}, column ${column + 1}; tap and hold to split row ${row + 1} into a parallel path` : `Add a block at row ${row + 1}, column ${column + 1}`}
          aria-label={splitPlacementEligible ? `Add a block at row ${row + 1}, column ${column + 1}; tap and hold to split row ${row + 1} into a parallel path` : `Add a block at row ${row + 1}, column ${column + 1}`}
          onPointerDown={splitPlacementEligible ? (event) => beginSplitPlacementGesture(event, row, column) : undefined}
          onPointerMove={splitPlacementEligible ? trackSplitPlacementGesture : undefined}
          onPointerUp={splitPlacementEligible ? (event) => endSplitPlacementGesture(event) : undefined}
          onPointerCancel={splitPlacementEligible ? (event) => endSplitPlacementGesture(event, true) : undefined}
          onClick={() => {
            if (splitPlacementEligible && gridClickIsPointerEcho(performance.now(), lastPointerReleaseAt.current)) return;
            onAction({ kind: "add-block-cell", row, column });
          }}
        ><span aria-hidden="true" /></button>
      });
    }
    return stops.sort((left, right) => left.x - right.x).map((stop) => stop.element);
  };
  // Block move mode, as the unit draws it while a block is lifted: a red trash
  // zone over the header controls, a drop slot in every empty cell (the lifted
  // block's own cell included), and - when the lifted block's lane has an empty
  // lane below it - the dotted parallel path that lane would open.
  const liftedBlock = dragPreview ? screenBlocks.find((block) => block.id === dragPreview.blockId) : undefined;
  const pairTop = liftedBlock ? liftedBlock.row - (liftedBlock.row % 2) : 0;
  const showDottedPath = Boolean(liftedBlock) && routes[pairTop]?.splitColumn === undefined && pairTop < rowY.length - 1 && !screenBlocks.some((block) => block.row === pairTop + 1);
  const moveModeOverlay = dragPreview ? <g className="coros-grid-move-mode" aria-hidden="true">
    <rect className="move-trash-zone" x="610" y="32" width="182" height="44" rx="8" />
    <g className="move-trash-glyph" transform="translate(691 41)"><path d="M3 5H15M6 5V2H12V5M4 7.5L5 22H13L14 7.5M7.5 10.5V18.5M10.5 10.5V18.5" /></g>
    {showDottedPath && <path className="move-dotted-path" d={`M61 ${rowY[pairTop]}V${rowY[pairTop + 1] - 5}Q61 ${rowY[pairTop + 1]} 66 ${rowY[pairTop + 1]}H735Q740 ${rowY[pairTop + 1]} 740 ${rowY[pairTop + 1] - 5}V${rowY[pairTop]}`} />}
    {rowY.flatMap((y, row) => columns.map((x, column) => (cellOccupied(row, column) && !(liftedBlock && liftedBlock.row === row && liftedBlock.column === column)) ? null : <rect key={`slot-${row}-${column}`} className="move-drop-slot" x={x - 8} y={y - 8} width="16" height="16" rx="4" />))}
  </g> : null;
  return <div className="qc-screen coros-vector-screen" aria-label="CorOS Grid">
    <svg className="coros-vector-canvas" viewBox="0 0 800 480" role="img" aria-label={`${snapshot.presetLocation} ${snapshot.presetName}, ${snapshot.mode} mode`}>
      <rect width="800" height="480" fill={QC_COLORS.reference.screen} />
      <g transform="matrix(1.012 0 0 1 -2 0)" fontFamily={QC_TYPOGRAPHY.devicePlain} fontWeight="800" fontSize="68">
        <text x="14" y="75"><tspan fill="var(--qc-grid-white-ink)" letterSpacing={PRESET_LOCATION_TRACKING}>{presetBank}</tspan><tspan fill={presetSlotAccent} letterSpacing={PRESET_LOCATION_TRACKING}>{presetSlot}</tspan></text>
        <text
          className={`preset-title${titlePresentation.italic ? " is-dirty" : ""}${titlePresentation.dimmed ? " is-unsaved" : ""}`}
          x={presetTitleStart}
          y={presetTitleBaseline}
          fill={titlePresentation.dimmed ? QC_COLORS.reference.unsaved : "var(--qc-grid-white-ink)"}
          fontSize={presetTitleFontSize}
          transform={titlePresentation.italic ? `translate(${presetTitleStart} ${presetTitleBaseline}) skewX(-12) translate(${-presetTitleStart} ${-presetTitleBaseline})` : undefined}
          textLength={squeezePresetTitle ? presetTitleMaxWidth : undefined}
          lengthAdjust={squeezePresetTitle ? "spacingAndGlyphs" : undefined}
        >{presetTitle}</text>
      </g>
      {!dragPreview && <>
      <QcScreenHeaderGlyph kind="nav-undo" />
      <QcScreenHeaderGlyph kind="nav-save" />
      <rect x="656" y="12" width="25" height="25" rx="3" fill={sceneMenuOpen ? QC_COLORS.reference.sceneControlMuted : QC_COLORS.reference.sceneBadge} /><text x="668.5" y="33" textAnchor="middle" fill={QC_COLORS.device.panel} fontFamily={QC_TYPOGRAPHY.devicePlain} fontWeight="800" fontSize="22">{sceneLetter}</text>
      <QcScreenHeaderGlyph kind="menu" />
      <g transform={`translate(${modeGlyphOrigin.x} ${modeGlyphOrigin.y})`} color="var(--qc-grid-white-ink)"><QcModeGlyph mode={snapshot.mode} /></g><text x="692" y="80" fill="var(--qc-grid-white-ink)" fontFamily={QC_TYPOGRAPHY.devicePlain} fontWeight="800" fontSize="21.5">{snapshot.mode}</text>
      </>}
      <g fill={QC_COLORS.reference.routePill} stroke={QC_COLORS.reference.screen} strokeWidth="1.2" fontFamily={QC_TYPOGRAPHY.deviceRoute} textAnchor="middle">
        {/* Not a mirror of the input rail: grid-base fills 750..793 on all four rows,
            against 8..51 on the input side, and 109..184 down - 44 by 76, with no
            border at all. Every edge of it steps straight from the screen to the
            pill colour, so the screen-coloured hairline this used to carry only
            ever ate the pill's own outermost pixel. */}
        {rowY.flatMap((y, row) => [<rect key={`in-${row}`} x="8" y={y - 38} width="44" height="76" rx="15" stroke="none" />, <rect key={`out-${row}`} x="750" y={y - 38} width="44" height="76" rx="15" stroke="none" />])}
        {rowY.flatMap((_, row) => [connectionMark("input", row), connectionMark("output", row)])}
        {/* Each port's contents sit on the centre of its own pill, and the two
            pills are not mirrored: the input fills x 8..51 and the output
            750..793. So the centres are 29.5 and 771.5, which the mark's box
            and the route text's `textAnchor="middle"` both round to 30 and 772,
            not 770, which put both two pixels left. */}
        {rowY.map((y, row) => <g key={`rails-${row}`}>{railLabel(displayInput(row), 30, y)}{railLabel(displayOutput(row), 772, y)}</g>)}
      </g>
      <g fill="none" stroke={QC_COLORS.reference.routeRail} strokeWidth="2">{rowY.map((_, row) => rowRail(row))}</g>
      {rowY.map((_, row) => splitPath(row))}
      {!screenBlocks.length && <g aria-label="Empty device slot">
        <rect x="66" y="119" width="64" height="64" rx="14" fill={QC_COLORS.reference.routePill} stroke={QC_COLORS.reference.screen} strokeWidth="1.2" />
        {railMark(98, 151)}
      </g>}
      {dragPreview && moveModeOverlay}
      <g>{(dragPreview ? [...screenBlocks].sort((left, right) => Number(left.id === dragPreview.blockId) - Number(right.id === dragPreview.blockId)) : screenBlocks).map(renderBlock)}</g>
    </svg>
    <div ref={actionsLayer} className="coros-vector-actions" aria-label="Grid controls">
      <button className="vector-action-hit preset-title-hit" title="Open device Directory" aria-label={`Open preset Directory; current preset ${snapshot.presetLocation} ${snapshot.presetName}`} onClick={() => { setSceneMenuOpen(false); setScreenMenuOpen(false); setModeMenuOpen(false); onOpenPreset(); }} />
      <button className="vector-action-hit undo-hit" title={canUndo ? `Undo ${undoLabel ?? "last action"}` : "Nothing to undo"} aria-label={canUndo ? `Undo ${undoLabel ?? "last action"}` : "Nothing to undo"} onClick={onUndo} />
      <button className="vector-action-hit scene-hit" aria-label="Select scene" {...sceneMenu.triggerProps} onClick={() => { setScreenMenuOpen(false); setModeMenuOpen(false); sceneMenu.toggle(); }} />
      <button className="vector-action-hit save-hit" title="Save preset to Quad Cortex" aria-label="Save preset to Quad Cortex" onClick={onSave} />
      <button className="vector-action-hit more-hit" title="Grid menu" aria-label="Open Grid menu" data-qc-open-target="grid-menu" {...gridMenu.triggerProps} onClick={() => { setSceneMenuOpen(false); setModeMenuOpen(false); gridMenu.toggle(); }} />
      <button
        className="vector-action-hit mode-hit"
        title="Mode menu"
        aria-label={`Open mode menu; current mode ${snapshot.mode}`}
        {...modeMenu.triggerProps}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          modeHoldFired.current = false;
          event.currentTarget.setPointerCapture?.(event.pointerId);
          clearTimeout(modeHoldTimer.current);
          modeHoldTimer.current = setTimeout(() => {
            modeHoldTimer.current = undefined;
            modeHoldFired.current = true;
            setSceneMenuOpen(false);
            setModeMenuOpen(false);
            runGridMenuAction("modes-configuration");
          }, GRID_LONG_PRESS_MS);
        }}
        onPointerUp={(event) => {
          clearTimeout(modeHoldTimer.current);
          modeHoldTimer.current = undefined;
          if (event.currentTarget.hasPointerCapture?.(event.pointerId)) event.currentTarget.releasePointerCapture?.(event.pointerId);
        }}
        onPointerCancel={() => {
          clearTimeout(modeHoldTimer.current);
          modeHoldTimer.current = undefined;
        }}
        onClick={() => {
          if (modeHoldFired.current) {
            modeHoldFired.current = false;
            return;
          }
          setSceneMenuOpen(false);
          setScreenMenuOpen(false);
          modeMenu.toggle();
        }}
      />
      {rowY.map((_, row) => <div key={`line-hits-${row}`} role="group" aria-label={`Signal line ${row + 1}`}>
        {/* Tap assigns a different source (onOpenRouting via onClick, unchanged);
            tap-and-hold opens Input Gate/Lane Output instead - see the manual
            passage summarised above. beginRailGesture/
            trackRailGesture/endRailGesture do the real work on the pointer
            events; onClick is the same gridClickIsPointerEcho-guarded fallback
            a block's own button below uses, for activation with no preceding
            pointer (keyboard Enter/Space). */}
        <button
          className="vector-route-hit input-route-hit"
          style={{ top: `${(108 + row * 94) / 4.8}%` }}
          aria-label={`Edit row ${row + 1} input; tap and hold to open Input Gate`}
          title={`Row ${row + 1} input - tap to change, tap and hold for Input Gate`}
          onPointerDown={(event) => beginRailGesture(event, row, "input")}
          onPointerMove={trackRailGesture}
          onPointerUp={(event) => endRailGesture(event)}
          onPointerCancel={(event) => endRailGesture(event, true)}
          onClick={() => { if (!gridClickIsPointerEcho(performance.now(), lastPointerReleaseAt.current)) onOpenRouting(row, "input"); }}
        />
        {rowActionStops(row)}
        <button
          className="vector-route-hit output-route-hit"
          style={{ top: `${(108 + row * 94) / 4.8}%` }}
          aria-label={`Edit row ${row + 1} output; tap and hold to open Lane Output`}
          title={`Row ${row + 1} output - tap to change, tap and hold for Lane Output`}
          onPointerDown={(event) => beginRailGesture(event, row, "output")}
          onPointerMove={trackRailGesture}
          onPointerUp={(event) => endRailGesture(event)}
          onPointerCancel={(event) => endRailGesture(event, true)}
          onClick={() => { if (!gridClickIsPointerEcho(performance.now(), lastPointerReleaseAt.current)) onOpenRouting(row, "output"); }}
        />
      </div>)}
    </div>
    {sceneMenuOpen && <><button type="button" className="scene-menu-backdrop" aria-label="Close scene menu" tabIndex={-1} onClick={() => sceneMenu.close(true)} /><div className="scene-dropdown vector-scene-dropdown" {...sceneMenu.menuProps} aria-label="Scenes">{snapshot.scenes.map((scene, index) => <button key={index} className={snapshot.activeScene === index ? "is-active" : ""} {...sceneMenu.itemProps(() => onAction({ kind: "select-scene", scene: index }))}><span>{sceneLabel(index)}</span>{sceneDisplayLabel(scene, index)}</button>)}</div></>}
    {modeMenuOpen && <><button type="button" className="scene-menu-backdrop" aria-label="Close mode menu" tabIndex={-1} onClick={() => modeMenu.close(true)} /><div className="scene-dropdown vector-mode-dropdown" {...modeMenu.menuProps} aria-label="Modes">{(snapshot.modeSlots ?? (["PRESET", "SCENE", "STOMP"] as const).map((mode, slot) => ({ slot: slot as 0 | 1 | 2, label: mode, mode }))).map((entry) => <button key={`${entry.slot}-${entry.label}`} className={snapshot.mode === entry.mode ? "is-active" : ""} {...modeMenu.itemProps(() => onAction({ kind: "select-mode-slot", slot: entry.slot }))}>{entry.label}</button>)}</div></>}
    {screenMenuOpen && <><button type="button" className="coros-screen-dimmer" aria-label="Close Grid contextual menu" tabIndex={-1} onClick={() => gridMenu.close(true)} /><div className="coros-screen-menu qc-scroll-rail" id={gridMenu.menuId} onKeyDown={gridMenu.menuProps.onKeyDown} role="menu" aria-label="Grid contextual menu" ref={restoreGridMenuScroll} onScroll={rememberGridMenuScroll}>
      <div className="context-menu-heading">FILE</div>
      {GRID_CONTEXT_MENU.slice(0, 8).map((item) => { const reason = item.action === "save-as" ? undefined : disabledContextActions?.[item.action]; return <button key={item.label} className={"danger" in item && item.danger ? "context-danger" : ""} title={reason} {...gridMenu.itemProps(() => runGridMenuAction(item.action, gridMenu.triggerRef.current?.dataset.qcOpenTarget === "grid-menu" ? "grid-menu" : undefined), { unavailable: Boolean(reason), opensSurface: true, focusTarget: GRID_MENU_FOCUS_TARGETS.has(item.action) ? item.action : undefined })}><span className="context-menu-icon"><QcUiIcon kind={item.icon === "midi" ? "menu-midi" : item.icon} /></span>{item.label}</button>; })}
      <div className="context-menu-section">QUAD CORTEX</div>
      {GRID_CONTEXT_MENU.slice(8).map((item) => { const reason = disabledContextActions?.[item.action]; return <button key={item.label} title={reason} {...gridMenu.itemProps(() => runGridMenuAction(item.action, gridMenu.triggerRef.current?.dataset.qcOpenTarget === "grid-menu" ? "grid-menu" : undefined), { unavailable: Boolean(reason), opensSurface: true, focusTarget: GRID_MENU_FOCUS_TARGETS.has(item.action) ? item.action : undefined })}><span className="context-menu-icon"><QcUiIcon kind={item.icon === "midi" ? "menu-midi" : item.icon} /></span>{item.label}</button>; })}
    </div></>}
    {routingPicker && <>
      <button className="coros-route-picker-dismiss" aria-label="Close route selection" onClick={routingPicker.onClose} />
      <svg className="coros-route-focus-layer" viewBox="0 0 800 480" aria-hidden="true">
        <rect width="800" height="480" fill={QC_COLORS.device.focusOverlay} fillOpacity=".27" />
        {routingPickerIsPort
          ? <>
            {/* The focus pill is its own port's pill, not a mirror of the input's:
                input-route-selector outlines x 8 and 51, output-route-selector 750
                and 793, so both sides start where their rail rect starts. 748 put
                the whole ring two pixels left of the one the unit draws. */}
            <rect x={routingPicker.side === "input" ? 8 : 750} y={rowY[routingPicker.row] - 39} width="44" height="78" rx="15" fill={QC_COLORS.device.routeFocus} stroke={QC_COLORS.reference.screen} strokeWidth="1.5" />
            <g textAnchor="middle">{railLabel(routingPicker.side === "input" ? displayInput(routingPicker.row) : displayOutput(routingPicker.row), routingPicker.side === "input" ? 30 : 772, rowY[routingPicker.row])}</g>
          </>
          // The split-placement flow edits the whole row, not one port pill -
          // there is no single port this picker names, so the row's own
          // signal rail is what indicates what is being changed.
          : <path d={`M55 ${rowY[routingPicker.row]}H747`} fill="none" stroke={QC_COLORS.device.routeFocus} strokeWidth="4" />}
      </svg>
      <section className={`coros-route-picker is-${routingPicker.side}`} aria-label={`Row ${routingPicker.row + 1} ${routingPicker.side === "split" ? "split column" : routingPicker.side === "mix" ? "rejoin column" : routingPicker.side} selection`}>
        <div className="coros-route-options qc-scroll-rail" role="listbox" aria-label={routingPickerIsPort ? `${routingPicker.side === "input" ? "Input" : "Output"} routes` : routingPicker.side === "split" ? "Split columns" : "Rejoin columns"}>
          {(() => {
            if (routingPicker.side !== "input" && routingPicker.side !== "output") return routingPicker.options.map(([value, label]) => <button key={value} role="option" aria-selected={value === routingPicker.value} disabled={routingPicker.disabled} onClick={() => routingPicker.onSelect(value)}><span>{label}</span></button>);
            // A plain local binding, not routingPicker.side read again inline:
            // TypeScript does not carry a narrowed object-property type across
            // a closure boundary (routePickerGroups.map's own callback), only
            // a narrowed local const survives into one.
            const side = routingPicker.side;
            return routePickerGroups.map((group) => <div className="coros-route-group" role="group" aria-label={group.name || "Unassigned"} key={group.name || "unassigned"}>
              {group.name && <strong>{group.name}</strong>}
              {group.options.map(([value, label]) => <button key={value} role="option" aria-selected={value === routingPicker.value} disabled={routingPicker.disabled} onClick={() => routingPicker.onSelect(value)}><QcRouteGlyph side={side} label={label} /><span>{routePickerLabel(side, label)}{side === "output" && value === 19 && <small>1/2 + 3/4 + USB 3/4</small>}</span></button>)}
            </div>);
          })()}
        </div>
      </section>
    </>}
    {addDevicePicker && <CorOsAddDevicePicker picker={addDevicePicker} cellCenter={{ x: columns[addDevicePicker.column], y: rowY[addDevicePicker.row] }} />}
    {savePreset?.open && <section className="coros-save-preset" aria-label={`Save preset to ${snapshot.presetLocation}`}>
      <header><button type="button" onClick={savePreset.onCancel} disabled={savePreset.disabled}>CANCEL</button><strong>SAVE PRESET</strong><button type="button" className="is-primary" onClick={savePreset.onSave} disabled={savePreset.disabled || !savePreset.name.trim()}>SAVE</button></header>
      <div className="coros-save-location"><span>DESTINATION</span><strong>{snapshot.setlistName}</strong><b>{snapshot.presetLocation}</b></div>
      <label className="coros-save-name"><span>PRESET NAME</span><input autoFocus maxLength={80} value={savePreset.name} disabled={savePreset.disabled} onChange={(event) => savePreset.onNameChange(event.target.value)} /></label>
      <div className="coros-save-keyboard" aria-label="Preset name keyboard">
        {["QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM"].map((row) => <div key={row}>{[...row].map((letter) => <button type="button" key={letter} disabled={savePreset.disabled || savePreset.name.length >= 80} onClick={() => savePreset.onNameChange(`${savePreset.name}${letter}`)}>{letter}</button>)}</div>)}
        <div className="keyboard-actions"><button type="button" aria-label="Backspace" disabled={savePreset.disabled || !savePreset.name} onClick={() => savePreset.onNameChange(savePreset.name.slice(0, -1))}><QcUiIcon kind="backspace" /></button><button type="button" className="space-key" disabled={savePreset.disabled || savePreset.name.length >= 80} onClick={() => savePreset.onNameChange(`${savePreset.name} `)}>SPACE</button></div>
      </div>
    </section>}
    {presetDirectory?.open && <CorOsDirectory snapshot={snapshot} directory={presetDirectory} />}
  </div>;
}

function controlByRole(controls: HardwareControl[], role: string) { return controls.find((control) => control.role === role); }

// Grid menu destinations whose labelled root takes focus when the user opens
// them from the menu (data-qc-open-target, user-open-focus.ts). Save as and
// Edit Details focus their own name input instead.
const GRID_MENU_FOCUS_TARGETS = new Set<string>(["settings", "tempo", "modes-configuration", "preset-midi-out", "cpu-monitor"]);

export function QuadCortexSurface({ formFactor, snapshot, selectedBlockId, skin, onAction, onOpenPreset, onUndo, canUndo, undoLabel, onSave, onOpenRouting, onRefresh, presetDirectory, gigPresetList, routingPicker, addDevicePicker, expressionAssignment, laneControlEditor, savePreset, parameterEditor, onContextAction, disabledContextActions, screenView = "grid", screenContent, cpuMonitor, deviceInformation, presentation = "hardware", onCloseScreen, onOpenGigView, onOpenIoView, onEditScene, onRecallGigPreset, onCopyGigScene, onSwapGigScene, onDeviceScreenGesture, sceneEditor, onMasterVolumeInteractionStart, onMasterVolumeInteractionEnd }: QuadCortexSurfaceProps) {
  const scenes = formFactor.controls.filter((control) => control.group === "scene");
  const bankUp = controlByRole(formFactor.controls, "bank:up")!;
  const bankDown = controlByRole(formFactor.controls, "bank:down")!;
  const tempo = controlByRole(formFactor.controls, "tempo")!;
  const leds = footswitchLeds(snapshot);
  const presetSlotIndex = ((snapshot.presetPosition % leds.length) + leds.length) % leds.length;
  // The slot letter is screen ink, so it comes from the slot palette rather
  // than from a lamp: reading it off the LED array coupled what the screen
  // draws to how the footswitches happen to be lit.
  const presetSlotAccent = QC_PRESET_SLOT_COLORS[presetSlotIndex];
  const parameterLeds = parameterEditor ? (() => {
    const editorAccent = parameterEditorAccent(parameterEditor.details.name, parameterEditor.accent);
    const size = parameterEditorPageSize(parameterEditor.details.category, parameterEditor.details.parameters);
    const visible = parameterEditorControlSlots(
      parameterEditor.details.parameters.filter((parameter) => parameter.normalizedValue !== null),
      parameterEditor.details.category,
      parameterEditor.page,
      size
    );
    return Array.from({ length: 10 }, (_, index) => ({
      active: Boolean(visible[index]),
      assigned: Boolean(visible[index]),
      color: /\bcab\b/i.test(parameterEditor.details.category) && (
        (parameterEditor.page === 0 && index >= 5 && index <= 8)
        || (parameterEditor.page === 1 && index === 2)
      ) ? QC_COLORS.category.pitch : editorAccent
    }));
  })() : undefined;
  const parameterLed = (slot: number, fallback: { active: boolean; assigned: boolean; color: string }) => parameterLeds?.[slot] ?? fallback;
  const navigationLedColor = QC_COLORS.hardware.whiteLed;
  const bankDownLed = parameterLed(4, { active: false, assigned: false, color: navigationLedColor });
  const displaySnapshot = fixtureSnapshot(screenView, snapshot);
  const fixtureOnly = screenView !== "grid";
  const screenSwipe = useRef<{ pointerId: number; startX: number; startY: number; height: number; destination: "gig" | "io"; committed: boolean } | null>(null);
  const deviceScreenGesture = useRef<{ pointerId: number; x: number; y: number; bounds: DOMRect; captureTarget: HTMLElement; longTapFired: boolean; longTapTimer?: number } | null>(null);
  useEffect(() => () => {
    const gesture = deviceScreenGesture.current;
    if (gesture?.longTapTimer !== undefined) window.clearTimeout(gesture.longTapTimer);
    if (gesture?.captureTarget.hasPointerCapture?.(gesture.pointerId)) {
      gesture.captureTarget.releasePointerCapture(gesture.pointerId);
    }
  }, []);
  const forwardsDeviceScreenGestures = screenView === "tuner" || screenView.startsWith("io-");
  const gridEdgeGesturesEnabled = screenView === "grid"
    && !sceneEditor
    && !parameterEditor
    && !laneControlEditor
    && !expressionAssignment
    && !presetDirectory?.open
    && !routingPicker
    && !addDevicePicker
    && !savePreset?.open;
  const beginScreenSwipe = (event: PointerEvent<HTMLDivElement>) => {
    if (!gridEdgeGesturesEnabled || event.button !== 0) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    const relativeY = event.clientY - bounds.top;
    // A rendered phone display can be only ~200 px tall. A ten-percent band
    // left barely a fingertip of usable edge and made valid device-style
    // swipes look inert. Keep the gesture edge-based, but make its hit band
    // large enough for touch while preserving the Grid's central controls.
    const destination = relativeY >= bounds.height * 0.82 && onOpenGigView
      ? "gig"
      : relativeY <= bounds.height * 0.18 && onOpenIoView
        ? "io"
        : undefined;
    if (!destination) return;
    // Do not capture on contact: a tap in the edge band still belongs to the
    // Grid control beneath it. Once the movement is clearly the device's
    // vertical edge gesture, capture the pointer so Android cannot turn it
    // into page movement and no underlying button receives the release.
    screenSwipe.current = { pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, height: bounds.height, destination, committed: false };
  };
  const trackScreenSwipe = (event: PointerEvent<HTMLDivElement>) => {
    const swipe = screenSwipe.current;
    if (!swipe || swipe.pointerId !== event.pointerId) return;
    const dx = event.clientX - swipe.startX;
    const dy = event.clientY - swipe.startY;
    const directedDistance = swipe.destination === "gig" ? -dy : dy;
    if (directedDistance < -8 || Math.abs(dx) > Math.max(12, Math.abs(dy) * 1.15)) {
      screenSwipe.current = null;
      return;
    }
    if (!swipe.committed && directedDistance >= 10 && directedDistance > Math.abs(dx) * 1.25) {
      swipe.committed = true;
      event.currentTarget.setPointerCapture?.(event.pointerId);
    }
    if (swipe.committed) event.preventDefault();
  };
  const finishScreenSwipe = (event: PointerEvent<HTMLDivElement>) => {
    const swipe = screenSwipe.current;
    screenSwipe.current = null;
    if (!swipe || swipe.pointerId !== event.pointerId || !swipe.committed) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.currentTarget.hasPointerCapture?.(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    const threshold = Math.max(24, swipe.height * 0.08);
    if (swipe.destination === "gig" && swipe.startY - event.clientY >= threshold) onOpenGigView?.();
    if (swipe.destination === "io" && event.clientY - swipe.startY >= threshold) onOpenIoView?.();
  };
  const canonicalScreenPoint = (clientX: number, clientY: number, bounds: DOMRect) => ({
    x: Math.max(0, Math.min(799, Math.round((clientX - bounds.left) * 800 / bounds.width))),
    y: Math.max(0, Math.min(479, Math.round((clientY - bounds.top) * 480 / bounds.height)))
  });
  const beginDeviceScreenGesture = (event: PointerEvent<HTMLDivElement>) => {
    if (!forwardsDeviceScreenGestures || !onDeviceScreenGesture) return;
    const target = event.target as Element;
    // Native screen controls also have local click handlers that keep the
    // reconstructed screen in sync with the physical tap. Capturing on this
    // wrapper would retarget pointerup/click away from those controls. Bubbled
    // pointer events still reach this wrapper, while the capture target below
    // is the control when present and the wrapper for surface gestures.
    const interactiveControl = target.closest<HTMLElement>("button, a, input, select, textarea, [role='button']");
    // Host workflows close top-level I/O and Tuner screens themselves. Global
    // EQ is nested inside I/O, however: its confirm button only returns the
    // local reconstruction to I/O. The physical QC must receive that tap too,
    // otherwise the two displays immediately disagree.
    const closeButton = target.closest("button[aria-label^='Close']");
    if (closeButton && closeButton.getAttribute("aria-label") !== "Close Global EQ") return;
    const bounds = event.currentTarget.getBoundingClientRect();
    // Capture on the control itself when the gesture starts there. That keeps
    // pointerup/cancel routed through its click handler while still delivering
    // bubbled pointer events here if the pointer leaves the screen.
    const captureTarget = interactiveControl ?? event.currentTarget;
    captureTarget.setPointerCapture?.(event.pointerId);
    const point = canonicalScreenPoint(event.clientX, event.clientY, bounds);
    const gesture = { pointerId: event.pointerId, ...point, bounds, captureTarget, longTapFired: false, longTapTimer: undefined as number | undefined };
    gesture.longTapTimer = window.setTimeout(() => {
      gesture.longTapTimer = undefined;
      gesture.longTapFired = true;
      onDeviceScreenGesture(point.x, point.y, point.x, point.y, "long-tap");
    }, GRID_LONG_PRESS_MS);
    deviceScreenGesture.current = gesture;
  };
  const trackDeviceScreenGesture = (event: PointerEvent<HTMLDivElement>) => {
    const gesture = deviceScreenGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId || gesture.longTapFired || gesture.longTapTimer === undefined) return;
    const point = canonicalScreenPoint(event.clientX, event.clientY, gesture.bounds);
    if (Math.hypot(point.x - gesture.x, point.y - gesture.y) > 8) {
      window.clearTimeout(gesture.longTapTimer);
      gesture.longTapTimer = undefined;
    }
  };
  const finishDeviceScreenGesture = (event: PointerEvent<HTMLDivElement>) => {
    const gesture = deviceScreenGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    deviceScreenGesture.current = null;
    if (gesture.captureTarget.hasPointerCapture?.(event.pointerId)) gesture.captureTarget.releasePointerCapture(event.pointerId);
    if (gesture.longTapTimer !== undefined) window.clearTimeout(gesture.longTapTimer);
    if (!onDeviceScreenGesture) return;
    if (gesture.longTapFired) return;
    const point = canonicalScreenPoint(event.clientX, event.clientY, gesture.bounds);
    const kind = Math.abs(point.x - gesture.x) <= 3 && Math.abs(point.y - gesture.y) <= 3 ? "tap" : "swipe";
    onDeviceScreenGesture(gesture.x, gesture.y, point.x, point.y, kind);
  };
  const cancelDeviceScreenGesture = (event: PointerEvent<HTMLDivElement>) => {
    const gesture = deviceScreenGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    if (gesture?.longTapTimer !== undefined) window.clearTimeout(gesture.longTapTimer);
    if (gesture.captureTarget.hasPointerCapture?.(event.pointerId)) gesture.captureTarget.releasePointerCapture(event.pointerId);
    deviceScreenGesture.current = null;
  };
  const chassisVectorCropStyle = skin.svgAsset ? {
    width: `${skin.svgAsset.sourceWidth / skin.svgAsset.crop.width * 100}%`,
    left: `${-skin.svgAsset.crop.x / skin.svgAsset.crop.width * 100}%`,
    top: `${-skin.svgAsset.crop.y / skin.svgAsset.crop.height * 100}%`
  } as CSSProperties : undefined;
  return <section className={`qc-chassis ${skin.className}${presentation === "display" ? " is-display-only" : ""}`} aria-label={formFactor.displayName}>
    {skin.svgAsset && <div className="chassis-vector-viewport" aria-hidden="true"><img className="chassis-vector-source" src={`${skin.svgAsset.url}#qc-foreground`} alt="" style={chassisVectorCropStyle} /></div>}
    <div className="chassis-edge" aria-hidden="true" />
    <MasterVolume value={snapshot.masterVolume} onAction={onAction} onInteractionStart={onMasterVolumeInteractionStart} onInteractionEnd={onMasterVolumeInteractionEnd} />
    <div className="device-plate"><QcHardwareIcon kind="brand-pulse" className="pulse-mark" /><span>{QC_BRAND.deviceWordmark}</span><small>{QC_BRAND.surfaceCaption}</small></div>
    <div className={`qc-screen-bezel${gridEdgeGesturesEnabled ? " is-grid-edge-gesture-surface" : ""}`} onPointerDownCapture={beginScreenSwipe} onPointerMoveCapture={trackScreenSwipe} onPointerUpCapture={finishScreenSwipe} onPointerCancel={() => { screenSwipe.current = null; }}>{sceneEditor
      ? <div className="qc-screen-fixture-root is-scene-editor"><SceneQuickEditPopover {...sceneEditor} inDeviceDisplay /></div>
      : screenContent
      ? <div className="qc-screen-fixture-root is-canonical-canvas">{screenContent}</div>
      : fixtureOnly
      ? <div className="qc-screen-fixture-root is-canonical-canvas" onPointerDown={beginDeviceScreenGesture} onPointerMove={trackDeviceScreenGesture} onPointerUp={finishDeviceScreenGesture} onPointerCancel={cancelDeviceScreenGesture}><Suspense fallback={null}><CorOsScreenFixture view={screenView} snapshot={displaySnapshot} gigPresetList={gigPresetList} deviceInformation={deviceInformation} onClose={onCloseScreen} onEditScene={onEditScene} onSelectScene={(scene) => onAction({ kind: "select-scene", scene })} onSelectModeSlot={(slot) => onAction({ kind: "select-mode-slot", slot })} onRecallGigPreset={onRecallGigPreset} onPressGigFootswitch={(index) => { const role = `footswitch:${String.fromCharCode(65 + index)}`; onAction({ kind: "switch", role, phase: "press" }); onAction({ kind: "switch", role, phase: "release" }); }} onEditGigBlock={(blockId) => onAction({ kind: "select-block", blockId })} onCopyGigScene={onCopyGigScene} onSwapGigScene={onSwapGigScene} /></Suspense></div>
      : <div className="qc-screen-fixture-root is-live-grid"><CorOsGrid
        snapshot={displaySnapshot}
        presetSlotAccent={presetSlotAccent}
        selectedBlockId={selectedBlockId}
        gridActions={{ onAction, onOpenPreset, onUndo, canUndo, undoLabel, onSave, onOpenRouting, onRefresh, presetDirectory, routingPicker, addDevicePicker, savePreset, onContextAction, disabledContextActions }}
        cpuColumns={cpuMonitor?.cpuColumns}
      />{cpuMonitor && <CorOsCpuMonitorPanel cpuLoad={cpuMonitor.cpuLoad} returnFocusTarget={cpuMonitor.returnFocusTarget} onClose={cpuMonitor.onClose} />}{parameterEditor && <CorOsParameterEditor {...parameterEditor} />}{laneControlEditor && <>
      {/* The manual's own dismissal for Input Gate/Lane Output Control is a
          tap anywhere outside the panel (INPUT & OUTPUT BLOCKS describes
          opening it but not this, though
          every other on-screen editor here - routing, parameter, expression -
          already gets one; this one didn't). Modeled on
          coros-route-picker-dismiss below: a full-bleed backdrop under the
          panel, above the grid, so a tap anywhere else on screen closes it
          the same way the real unit does. */}
      <button className="coros-lane-control-dismiss" aria-label="Close lane control panel" onClick={laneControlEditor.onClose} />
      <CorOsLaneControlPanel state={laneControlEditor} />
    </>}{expressionAssignment && <CorOsExpressionAssignment assignment={expressionAssignment} />}</div>}
    </div>
    <div className="screen-nav-control"><QcUiIcon kind="up" className="nav-arrow nav-arrow-up" /><QcHardwareSwitch role={bankUp.role} label="BANK UP" compact active={Boolean(parameterEditor)} assigned={Boolean(parameterEditor)} accent={navigationLedColor} onAction={onAction} /><QcUiIcon kind="down" className="nav-arrow nav-arrow-down" /></div>
    <div className="footswitch-deck">
      <div className="footswitch-row">{scenes.slice(0, 4).map((control, index) => { const led = parameterLed(index, leds[index]); return <QcHardwareSwitch key={control.id} role={control.role} label={control.label} active={led.active} assigned={led.assigned} accent={led.color} onAction={onAction} />; })}<QcHardwareSwitch role={bankDown.role} label="BANK DOWN" active={bankDownLed.active} assigned={bankDownLed.assigned} accent={bankDownLed.color} onAction={onAction} /></div>
      <div className="mode-bracket" aria-hidden="true"><span><QcUiIcon kind="add" /></span><strong>MODE</strong><span><QcUiIcon kind="subtract" /></span></div>
      <div className="footswitch-row">{scenes.slice(4).map((control, index) => { const led = parameterLed(index + 5, leds[index + 4]); return <QcHardwareSwitch key={control.id} role={control.role} label={control.label} active={led.active} assigned={led.assigned} accent={led.color} onAction={onAction} />; })}<QcHardwareSwitch role={tempo.role} label="TEMPO" active={parameterLeds ? parameterLeds[9].active : snapshot.tempoLedEnabled} assigned={parameterLeds ? parameterLeds[9].assigned : snapshot.tempoLedEnabled} pulseBpm={!parameterLeds && snapshot.tempoLedEnabled ? snapshot.tempo : undefined} pulseEpochMs={!parameterLeds ? snapshot.tempoPulseEpochMs : undefined} accent={parameterLeds ? parameterLeds[9].color : QC_COLORS.device.tempoLed} onAction={onAction} /></div>
      <span className="tuner-hint">TEMPO<br />HOLD: TUNER</span>
    </div>
  </section>;
}
