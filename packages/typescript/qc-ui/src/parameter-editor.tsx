import { createContext, Fragment, useContext, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type PointerEvent } from "react";
import type { BlockDetails, BlockParameter } from "@qc-remote/client";
import { sceneLetter } from "@qc-remote/core";
import { QC_COLORS } from "@qc-remote/theme";
import { EQ_FILTER_TYPE_LABELS, parameterControlKind, parameterDisplay, parameterEditorAccent, parameterEditorFamily, parameterEditorIsFullScreen, parameterEditorPageCount, parameterEditorPageSize, parameterEditorPageSlots, parameterEditorTabs, parameterNormalizedValue, parameterOptionLabels, parameterRealValue, parameterStep, type ParameterEditorFamily } from "./parameter-model";
import { parameterContextMenuItems, type ParameterEditorContextAction } from "./parameter-menu";
import { QcEditorIcon, QcPresetStackIcon, QcScreenGlyph, QcUiIcon } from "./theme-icons";
import { QcPillToggle } from "./qc-pill-toggle";
import { QcBarSlider } from "./qc-bar-slider";
import { QcRotaryControl } from "./qc-rotary-control";
import { QcRotaryDial } from "./qc-rotary-dial";
import { useHoldGesture } from "./use-hold-gesture";
import { PARAMETER_ROTARY_PIXELS_PER_STEP, PARAMETER_ROTARY_WHEEL_SETTLE_MS, useRotaryValue } from "./use-rotary-gesture";
import { LOOPER_X_ASSIGNMENT_INDEX_NAMES } from "./device-workflow-model";
import { cabMicrophonePlacement, cabMicrophoneValuesAt } from "./cab-microphone-geometry";
import { QcCabMicrophoneTarget, QcEditorFootswitchAssignment, QcFootswitchAssignmentPrompt, QcPillToggleLabels, QcSceneSelector } from "./qc-shared-controls";
import "./official-routing.css";

export type { ParameterEditorContextAction } from "./parameter-menu";

const CAB_REFERENCE_VALUES = [
  { name: "POSITION", display: <>4.0</>, value: 0.4, origin: 0 },
  { name: "DISTANCE", display: <>0.0</>, value: 0, origin: 0 },
  { name: "LEVEL", display: <>0.0 <small>dB</small></>, value: 0.5037, origin: 0 },
  { name: "PAN", display: <>C</>, value: 0.5, origin: 0.5 }
] as const;

export interface CorOsParameterEditorProps {
  details: BlockDetails;
  /** Explicit static reference presentation; live callers must leave this unset. */
  referenceOnly?: boolean;
  /** Host workflow session identity, retiring local dialogs on reconnection/reload. */
  editorContextKey?: unknown;
  /** Passive measured title decoration; absent for live state unless explicitly supplied. */
  titlePresetMark?: boolean;
  drafts: Record<number, number>;
  accent: string;
  activeScene: number;
  scenes: string[];
  bypassed: boolean;
  footswitch?: number;
  routingNode?: "splitter" | "mixer";
  disabled?: boolean;
  page: number;
  onPageChange: (page: number) => void;
  onDraftChange: (parameter: BlockParameter, value: number) => void;
  onCommit: (parameter: BlockParameter, value: number) => void;
  onToggleSceneMode: (parameter: BlockParameter) => void;
  onCommitBatch?: (changes: Array<{ parameter: BlockParameter; value: number }>) => void;
  onToggleBypass: () => void;
  onSceneSelect: (scene: number) => void;
  footswitchAssignmentPending?: boolean;
  onFootswitchAssignmentStart: (pending: boolean) => void;
  /** Unassign the block's current footswitch outright, from the assignment prompt's own UNASSIGN button. */
  onUnassignFootswitch: () => void;
  onContextAction: (action: ParameterEditorContextAction) => void;
  onSaveDevicePreset?: (name: string) => Promise<void> | void;
  onLoadLooperAssignments?: () => Promise<number[]>;
  onSaveLooperAssignments?: (assignments: number[]) => Promise<void> | void;
  clipboardModelId?: number;
  contextActionEnabled?: Partial<Record<ParameterEditorContextAction, boolean>>;
  /** Open the device contextual menu on first render. Used when another
   * device screen links directly to the same native overflow menu. */
  initialMenuOpen?: boolean;
  /** Reports the context menu opening and closing (and closing when the editor goes away), so the unit's can follow. Must be stable. */
  onMenuOpenChange?: (open: boolean) => void;
  onClose: () => void;
}

function clamp(value: number) { return Math.max(0, Math.min(1, value)); }
/** A Cab editor's header names the category "GUITAR CAB" (ukc30-cab and a physical 412 Brit TV); every other category is its own name in capitals. */
function editorCategoryLabel(category: string, deviceName?: string) { if (deviceName === "UK C30 TopBoost") return "GUITAR AMP"; return category.toLowerCase() === "cab" ? "GUITAR CAB" : category.toUpperCase(); }

const ParameterSceneModeContext = createContext<((parameter: BlockParameter) => void) | undefined>(undefined);

/** A device parameter's rotary gesture: one parameter step per 7 px or wheel
 * notch, on the normalised 0..1 scale, settling 55 ms after the wheel. Shared
 * by the block editor's knobs and faders and the splitter/mixer knobs. */
function useParameterRotary(parameter: BlockParameter, value: number, disabled: boolean, onDraftChange: (value: number) => void, onCommit: (value: number) => void) {
  return useRotaryValue({
    value, disabled, onDraftChange, onCommit,
    step: parameterStep(parameter),
    quantize: (next) => { const step = parameterStep(parameter); return clamp(Math.round(clamp(next) / step) * step); },
    pixelsPerStep: PARAMETER_ROTARY_PIXELS_PER_STEP,
    wheelSettleMs: PARAMETER_ROTARY_WHEEL_SETTLE_MS
  });
}

/** A normalised parameter as an ARIA slider range: 0 through 100. */
function parameterSliderRange(normalized: number) {
  return { min: 0, max: 100, value: Math.round(normalized * 100) };
}

function ParameterControl({ parameter, value, slot, accent, disabled, variant = "knob", onDraftChange, onCommit }: {
  parameter: BlockParameter;
  value: number;
  slot: number;
  accent: string;
  disabled?: boolean;
  /** The cab channel strip draws this same control as a horizontal fader. */
  variant?: "knob" | "bar";
  onDraftChange: (parameter: BlockParameter, value: number) => void;
  onCommit: (parameter: BlockParameter, value: number) => void;
}) {
  const toggleSceneMode = useContext(ParameterSceneModeContext);
  const [localValue, setLocalValue] = useState(value);
  useEffect(() => { setLocalValue(value); }, [value]);
  const controlDisabled = Boolean(disabled || !parameter.writable || parameter.enabled === false);
  const gesture = useParameterRotary(parameter, value, controlDisabled, (next) => { setLocalValue(next); onDraftChange(parameter, next); }, (next) => onCommit(parameter, next));
  const update = gesture.set;
  // Holding any of the control's surfaces assigns it to scenes. The hold is
  // cancelled before the touch knob's six-pixel rotation threshold, so a
  // small value adjustment can never mature into a scene-assignment hold.
  const hold = useHoldGesture({ moveTolerancePx: 4, onHold: () => toggleSceneMode?.(parameter) });
  // The EQ band's filter type declares no options of its own; parameterOptionLabels
  // supplies the five the unit offers, so the chooser has something to offer.
  const optionLabels = parameterOptionLabels(parameter);
  const optionIndex = optionLabels.length > 1 ? Math.round(localValue * (optionLabels.length - 1)) : 0;
  const controlKind = parameter.type.trim().toLowerCase();
  const renderedKind = parameterControlKind(parameter);
  const meter = renderedKind === "meter";
  const fader = renderedKind === "fader";
  const ledBacked = controlKind === "floatwithled";
  const rotarySwitch = controlKind === "rotaryswitch";
  const switchControl = renderedKind === "switch";
  const toggleButton = renderedKind === "button";
  const switchOptions = optionLabels.length > 1 ? [...optionLabels] : ["Off", "On"];
  const switchIndex = Math.round(localValue * (switchOptions.length - 1));
  const switchThumbTop = 5 + (switchOptions.length <= 1 ? 0 : (switchOptions.length - 1 - switchIndex) * 47 / (switchOptions.length - 1));
  const rotaryStepCount = rotarySwitch ? Math.max(2, parameter.options.length || parameter.steps || 2) : 0;
  const encoderLabel = ["A", "B", "C", "D", undefined, "E", "F", "G", "H", "T"][slot];
  const display = parameterDisplay(parameter, localValue);
  return <div className={`coros-parameter${meter ? " is-meter" : fader ? " is-fader" : switchControl ? ` is-switch is-${switchOptions.length}-way` : toggleButton ? " is-button" : renderedKind === "select" ? " is-dropdown" : " is-knob"}${rotarySwitch ? " is-rotary-switch" : ""}${ledBacked ? " has-led" : ""}${parameter.sceneMode ? " is-scene" : ""}${controlDisabled ? " is-disabled" : ""}`}
    onPointerDownCapture={(event) => {
      if (event.button !== 0 || controlDisabled || !toggleSceneMode) return;
      hold.start(undefined, { clientX: event.clientX, clientY: event.clientY });
    }}
    onPointerMoveCapture={(event) => hold.move({ clientX: event.clientX, clientY: event.clientY })}
    onPointerUpCapture={() => hold.cancel()}
    onPointerCancelCapture={() => hold.cancel()}
    onClickCapture={(event) => {
      if (!hold.consumeFired()) return;
      event.preventDefault();
      event.stopPropagation();
    }}>
    <span className="parameter-name">{parameter.name}</span>
    {parameter.expression != null && parameter.expression > 0 && <span className="parameter-expression" title={`Assigned to Expression Pedal ${parameter.expression}${parameter.expressionMinimum != null && parameter.expressionMaximum != null ? ` (${Math.round(parameter.expressionMinimum * 100)}–${Math.round(parameter.expressionMaximum * 100)}%)` : ""}`}><QcEditorIcon kind="assignment-expression" /><b>EXP {parameter.expression}</b></span>}
    {meter ? <div className="parameter-gr-meter" role="meter" aria-label={parameter.name} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(localValue * 100)} style={{ "--meter-level": `${localValue * 100}%` } as CSSProperties}><span /></div> : fader ? <QcRotaryControl className="parameter-fader" disabled={controlDisabled} label={`${parameter.name}: ${display}`} {...parameterSliderRange(localValue)} valueText={display} style={{ "--parameter-position": `${localValue * 100}%`, "--parameter-accent": accent } as CSSProperties} gesture={gesture}><span /></QcRotaryControl> : renderedKind === "select" ? <select aria-label={parameter.name} value={optionIndex} disabled={controlDisabled} onChange={(event) => {
      const next = Number(event.target.value) / (optionLabels.length - 1);
      update(next); onCommit(parameter, next);
    }}>{optionLabels.map((option, index) => <option key={`${option}-${index}`} value={index}>{option}</option>)}</select> : switchControl ? <><button className="parameter-switch" disabled={controlDisabled} aria-label={`${parameter.name}: ${parameterDisplay(parameter, localValue)}`} onClick={() => { const nextIndex = (switchIndex + 1) % switchOptions.length; const next = nextIndex / (switchOptions.length - 1); update(next); onCommit(parameter, next); }}><span style={{ top: `${switchThumbTop}%` }} /></button><span className="parameter-switch-labels">{[...switchOptions].reverse().map((option, reverseIndex) => { const index = switchOptions.length - 1 - reverseIndex; const next = index / (switchOptions.length - 1); return <button key={`${option}-${index}`} className={index === switchIndex ? "is-active" : ""} disabled={controlDisabled} onClick={() => { update(next); onCommit(parameter, next); }}>{option}</button>; })}</span></> : toggleButton ? <button className={`parameter-toggle-button${localValue >= .5 ? " is-on" : ""}`} disabled={controlDisabled} aria-pressed={localValue >= .5} onClick={() => { const next = localValue >= .5 ? 0 : 1; update(next); onCommit(parameter, next); }}>{parameter.options[Math.round(localValue * Math.max(0, parameter.options.length - 1))] ?? parameter.name}</button> : <QcRotaryControl className="parameter-knob" disabled={controlDisabled} label={`${parameter.name}: ${display}`} {...parameterSliderRange(localValue)} valueText={display} style={{ "--parameter-angle": `${-135 + localValue * 270}deg`, "--parameter-position": `${localValue * 100}%`, "--parameter-accent": accent } as CSSProperties} gesture={gesture}>{variant === "bar" ? <QcBarSlider value={localValue} origin={parameter.midpointLabel ? .5 : 0} /> : <QcRotaryDial value={localValue} mode={rotarySwitch ? "step" : "arc"} steps={rotaryStepCount || 2} accent={accent} />}</QcRotaryControl>}
    {ledBacked && <i className={`parameter-signal-led${(parameter.ledValue ?? 0) > .02 ? " is-active" : ""}`} aria-hidden="true" />}
    <strong className="parameter-value">{parameterDisplay(parameter, localValue)}</strong>
    <small className="parameter-encoder">{encoderLabel ?? <QcUiIcon kind="down" />}</small>
  </div>;
}

type EqBand = {
  number: number;
  gain?: BlockParameter;
  frequency?: BlockParameter;
  q?: BlockParameter;
  type?: BlockParameter;
  bypass?: BlockParameter;
};

function eqBands(parameters: BlockParameter[]): EqBand[] {
  const bands = new Map<number, EqBand>();
  for (const parameter of parameters) {
    const match = parameter.name.match(/^N?(\d+)\s*(GAIN|FREQ(?:UENCY)?|Q|TYPE|BYPASS)$/i);
    if (!match) continue;
    const number = Number(match[1]);
    const band = bands.get(number) ?? { number };
    const field = match[2].toUpperCase();
    if (field === "GAIN") band.gain = parameter;
    else if (field.startsWith("FREQ")) band.frequency = parameter;
    else if (field === "Q") band.q = parameter;
    else if (field === "TYPE") band.type = parameter;
    else band.bypass = parameter;
    bands.set(number, band);
  }
  return [...bands.values()].sort((left, right) => left.number - right.number);
}

function parameterWithName(parameter: BlockParameter, name: string, options?: string[]): BlockParameter {
  return { ...parameter, name, options: options ?? parameter.options, type: options ? "enum" : parameter.type, steps: options?.length ?? parameter.steps };
}

function EqResponseGraph({ bands, selectedBand, drafts, accent, disabled, onSelectBand, onDraftChange, onCommit }: {
  bands: EqBand[];
  selectedBand: number;
  drafts: Record<number, number>;
  accent: string;
  disabled?: boolean;
  onSelectBand: (band: number) => void;
} & ControlCallbacks) {
  const drag = useRef<{ pointerId: number; x: number; y: number; frequency?: BlockParameter; gain?: BlockParameter; frequencyValue: number; frequencyGraphValue: number; gainValue: number } | undefined>(undefined);
  const editable = (parameter: BlockParameter | undefined) => !disabled && Boolean(parameter?.writable)
    && parameter?.enabled !== false && parameter?.normalizedValue != null;
  const currentOperand = (parameter: BlockParameter | undefined) => parameter && bands
    .flatMap((band) => [band.frequency, band.gain]).find((candidate) => candidate?.index === parameter.index);
  const value = (parameter: BlockParameter | undefined, fallback: number) => parameter ? drafts[parameter.index] ?? parameter.normalizedValue ?? fallback : fallback;
  const graphFrequency = (parameter: BlockParameter | undefined, normalized: number, fallback: number) => {
    if (!parameter) return fallback;
    const frequency = Math.max(20, Math.min(20000, parameterRealValue(parameter, normalized)));
    return Math.log10(frequency / 20) / Math.log10(20000 / 20);
  };
  const points = bands.map((band, index) => ({
    band,
    x: 5 + graphFrequency(band.frequency, value(band.frequency, .5), bands.length === 1 ? .5 : index / (bands.length - 1)) * 90,
    y: 50 - (value(band.gain, .5) - .5) * 76,
    bypassed: value(band.bypass, 0) >= .5
  }));
  const activePoints = points.filter((point) => !point.bypassed).sort((left, right) => left.x - right.x);
  const polyline = [[0, 50], ...activePoints.map((point) => [point.x, point.y]), [100, 50]].map(([x, y]) => `${x},${y}`).join(" ");
  const updateDrag = (event: PointerEvent<HTMLButtonElement>) => {
    if (!drag.current || drag.current.pointerId !== event.pointerId) return;
    const panel = event.currentTarget.closest(".eq-response-panel")?.getBoundingClientRect();
    if (!panel) return;
    const nextFrequencyGraph = clamp(drag.current.frequencyGraphValue + (event.clientX - drag.current.x) / panel.width);
    const nextFrequency = drag.current.frequency
      ? parameterNormalizedValue(drag.current.frequency, 20 * Math.pow(1000, nextFrequencyGraph))
      : clamp(drag.current.frequencyValue + (event.clientX - drag.current.x) / panel.width);
    const nextGain = clamp(drag.current.gainValue - (event.clientY - drag.current.y) / (panel.height * .76));
    const frequency = currentOperand(drag.current.frequency), gain = currentOperand(drag.current.gain);
    if (editable(frequency)) onDraftChange(frequency!, nextFrequency);
    if (editable(gain)) onDraftChange(gain!, nextGain);
  };
  const finishDrag = (event: PointerEvent<HTMLButtonElement>) => {
    if (!drag.current || drag.current.pointerId !== event.pointerId) return;
    const current = drag.current;
    drag.current = undefined;
    if (event.currentTarget.hasPointerCapture?.(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    const frequency = currentOperand(current.frequency), gain = currentOperand(current.gain);
    if (editable(frequency)) onCommit(frequency!, drafts[frequency!.index] ?? frequency!.normalizedValue ?? current.frequencyValue);
    if (editable(gain)) onCommit(gain!, drafts[gain!.index] ?? gain!.normalizedValue ?? current.gainValue);
  };
  return <div className="eq-response-panel" style={{ "--eq-accent": accent } as CSSProperties}>
    <svg viewBox="0 0 100 100" preserveAspectRatio="none" aria-label="EQ frequency response">
      <g className="eq-grid"><path d="M5 8V88M28 8V88M52 8V88M76 8V88M95 8V88"/><path d="M5 12H95M5 31H95M5 50H95M5 69H95M5 88H95"/></g>
      <polyline className="eq-live-curve" points={polyline} />
      <g className="eq-axis-labels"><text x="5" y="97">20</text><text x="28" y="97">100</text><text x="52" y="97">1k</text><text x="76" y="97">10k</text><text x="95" y="97" textAnchor="end">20k</text><text x="1" y="15">+12</text><text x="1" y="53">0</text><text x="1" y="90">-12</text></g>
    </svg>
    {points.map((point) => <button key={point.band.number} className={`eq-band-node${selectedBand === point.band.number ? " is-selected" : ""}${point.bypassed ? " is-bypassed" : ""}`} style={{ left: `${point.x}%`, top: `${point.y}%` }} disabled={disabled} aria-label={`Select EQ band ${point.band.number}`} aria-pressed={selectedBand === point.band.number} onClick={() => onSelectBand(point.band.number)} onPointerDown={(event) => {
      if (disabled) return;
      onSelectBand(point.band.number);
      event.currentTarget.setPointerCapture?.(event.pointerId);
      drag.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY, frequency: editable(point.band.frequency) ? point.band.frequency : undefined, gain: editable(point.band.gain) ? point.band.gain : undefined, frequencyValue: value(point.band.frequency, point.x / 100), frequencyGraphValue: (point.x - 5) / 90, gainValue: value(point.band.gain, .5) };
    }} onPointerMove={updateDrag} onPointerUp={finishDrag} onPointerCancel={finishDrag}>{point.band.number}</button>)}
  </div>;
}

function EqBandBypassControl({ band, drafts, accent, disabled, onDraftChange, onCommit }: { band: EqBand; drafts: Record<number, number>; accent: string; disabled?: boolean } & ControlCallbacks) {
  const parameter = band.bypass;
  const bypassed = parameter ? (drafts[parameter.index] ?? parameter.normalizedValue ?? 0) >= .5 : false;
  return <div className="coros-parameter eq-bypass-control">
    <span className="parameter-name">BYPASS {band.number}</span>
    <button className={bypassed ? "" : "is-on"} disabled={disabled || !parameter?.writable || parameter.enabled === false} aria-label={`${bypassed ? "Enable" : "Bypass"} EQ band ${band.number}`} onClick={() => { if (!parameter?.writable || parameter.enabled === false || disabled) return; const next = bypassed ? 0 : 1; onDraftChange(parameter, next); onCommit(parameter, next); }}><QcEditorIcon kind="band-power" /></button>
  </div>;
}

function ParametricEqEditorBody({ parameters, drafts, accent, disabled, selectedBand, outputSelected, onSelectBand, onSelectOutput, onDraftChange, onCommit }: {
  parameters: BlockParameter[];
  drafts: Record<number, number>;
  accent: string;
  disabled?: boolean;
  selectedBand: number;
  onSelectBand: (band: number) => void;
  outputSelected: boolean;
  onSelectOutput: () => void;
} & ControlCallbacks) {
  const bands = eqBands(parameters);
  const selected = bands.find((band) => band.number === selectedBand) ?? bands[0];
  const output = parameters.find((parameter) => /^OUTPUT$/i.test(parameter.name));
  const controls = [
    selected?.type && parameterWithName(selected.type, "TYPE", ["Peak", "Hi Pass", "Lo Pass", "Hi Shelf", "Lo Shelf"]),
    selected?.gain && parameterWithName(selected.gain, "GAIN"),
    selected?.frequency && parameterWithName(selected.frequency, "FREQ"),
    selected?.q && parameterWithName(selected.q, "Q")
  ].filter((parameter): parameter is BlockParameter => Boolean(parameter));
  return <div className="eq-editor-body is-parametric">
    <EqResponseGraph bands={bands} selectedBand={outputSelected ? -1 : selected?.number ?? 1} drafts={drafts} accent={accent} disabled={disabled} onSelectBand={onSelectBand} onDraftChange={onDraftChange} onCommit={onCommit} />
    <nav className="eq-band-tabs" aria-label="EQ bands">{bands.map((band) => {
      const bypassed = band.bypass ? (drafts[band.bypass.index] ?? band.bypass.normalizedValue ?? 0) >= .5 : false;
      const active = !outputSelected && selected?.number === band.number;
      return <button key={band.number} className={`${active ? "is-active" : ""}${bypassed ? " is-bypassed" : ""}`} aria-label={`Band ${band.number}${bypassed ? ", bypassed" : ""}`} aria-current={active ? "page" : undefined} onClick={() => onSelectBand(band.number)}>{band.number}</button>;
    })}<button className={outputSelected ? "is-active" : ""} aria-current={outputSelected ? "page" : undefined} onClick={onSelectOutput}>OUT</button></nav>
    {outputSelected ? <div className="eq-selected-controls is-output">{output && <ParameterControl parameter={output} value={drafts[output.index] ?? output.normalizedValue ?? 0} slot={0} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} />}</div> : <div className="eq-selected-controls">{controls.map((parameter, slot) => <ParameterControl key={parameter.index} parameter={parameter} value={drafts[parameter.index] ?? parameter.normalizedValue ?? 0} slot={slot} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} />)}{selected && <EqBandBypassControl band={selected} drafts={drafts} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} />}</div>}
  </div>;
}

function GraphicEqEditorBody({ parameters, drafts, accent, disabled, onDraftChange, onCommit }: {
  parameters: BlockParameter[];
  drafts: Record<number, number>;
  accent: string;
  disabled?: boolean;
} & ControlCallbacks) {
  const bands = parameters.filter((parameter) => /^(?:N)?(?:65|125|250|500|1K|2K|4K|8K|16K)\s*HZ$/i.test(parameter.name.replace(/\s/g, "")) || (parameter.type.toLowerCase() === "fader" && !/HPF|LPF|OUTPUT|\bQ\b/i.test(parameter.name))).slice(0, 9);
  const auxiliary = parameters.filter((parameter) => !bands.includes(parameter));
  return <div className="eq-editor-body is-graphic">
    <div className="graphic-eq-faders">{bands.map((parameter, slot) => <ParameterControl key={parameter.index} parameter={parameterWithName(parameter, parameter.name.replace(/^N/i, ""))} value={drafts[parameter.index] ?? parameter.normalizedValue ?? 0} slot={slot} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} />)}</div>
    <div className="graphic-eq-aux">{auxiliary.slice(0, 4).map((parameter, slot) => <ParameterControl key={parameter.index} parameter={parameter} value={drafts[parameter.index] ?? parameter.normalizedValue ?? 0} slot={slot} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} />)}</div>
  </div>;
}

const REFERENCE_STANDARD_EDITORS = new Set(["Simple Gate", "Chief DS1", "Digital Flanger", "UK C30 TopBoost", "Ambience"]);

function referenceEditorClass(name: string) {
  if (name === "UK C30 TopBoost") return "ukc30-topboost";
  return name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "");
}

function referenceParameterClass(name: string) {
  return `parameter-${name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "")}`;
}

/* Where the unit actually puts the pointer, for the handful of parameters
   whose scale is not otherwise modelled. Everything else follows the
   parameter's normalised value, and the fill follows the pointer, because
   that is the only relationship the unit ever draws. */
const REFERENCE_DIAL_VALUES: Record<string, Record<string, number>> = {
  "Simple Gate": { THRESHOLD: 0.5162 },
  "Digital Flanger": { RATE: 0.0828, DELAY: 0.2808, "SYNC NOTE": 0.5605 },
  /* Ambience's frequency and time parameters are on tapers that are not
     otherwise modelled, so the value that prints "80 Hz" is not the value
     that puts the pointer where the unit puts it. */
  Ambience: { MIX: 0.1353, "PRE DELAY": 0.3855, "HIGH PASS": 0.2269, "LOW PASS": 0.6281 },
  "UK C30 TopBoost": { VOLUME: 0.3847, BASS: 0.3847, TREBLE: 0.5777, "TONE CUT": 0.4518, OUTPUT: 0.5 },
  "Chief DS1": { DISTORTION: 0.5777, TONE: 0.4709, LEVEL: 0.5 }
};

function referenceDialValue(editorName: string, parameterName: string, value: number) {
  return REFERENCE_DIAL_VALUES[editorName]?.[parameterName] ?? value;
}

/* A dial with named positions is a detent dial on the unit, and draws one dot
   per option, both ends of the sweep included. The Flanger's SYNC NOTE ring
   is the example: seventeen dots running 225.0 to 135.0 degrees clockwise
   from twelve at a 16.87 degree pitch - 270/16. The catalogue lists
   seventeen notes for it, so the count is the option count after all. */
function referenceDialSteps(_editorName: string, parameter: BlockParameter) {
  return parameter.options.length > 1 ? parameter.options.length : 0;
}

function CorOsReferenceStandardEditor({ props, accent, parameters, menuOpen, setMenuOpen, contextItems, runContextAction }: {
  props: CorOsParameterEditorProps;
  accent: string;
  parameters: BlockParameter[];
  menuOpen: boolean;
  setMenuOpen: (open: boolean) => void;
  contextItems: ReturnType<typeof parameterContextMenuItems>;
  runContextAction: (action: ParameterEditorContextAction) => void;
}) {
  const { details, drafts, activeScene, scenes, bypassed, footswitch, disabled, onDraftChange, onCommit, onToggleBypass, onSceneSelect, footswitchAssignmentPending, onFootswitchAssignmentStart, onUnassignFootswitch, contextActionEnabled, onPageChange, onClose } = props;
  const hasPages = details.name === "Digital Flanger";
  const sceneCount = Math.max(1, scenes.length);
  return <section className={`coros-parameter-editor coros-reference-parameter-editor editor-${referenceEditorClass(details.name)}${bypassed ? " is-bypassed" : ""}`} style={{ "--editor-color": accent } as CSSProperties} aria-label={`${details.name} parameter editor`}>
    <header className={hasPages ? "has-pages" : ""}>
      <button className="editor-more" aria-label="Device contextual menu" aria-expanded={menuOpen} onClick={() => { onFootswitchAssignmentStart(false); setMenuOpen(!menuOpen); }}><QcUiIcon kind="more" /></button>
      <span className="editor-title"><small>{editorCategoryLabel(details.category, details.name)}</small><strong style={{ color: accent }}>{details.name}{props.titlePresetMark && <i className="editor-preset-mark"><QcPresetStackIcon /></i>}</strong></span>
      <span className="editor-spacer" />
      <QcEditorFootswitchAssignment pending={footswitchAssignmentPending} letter={footswitch === undefined ? undefined : sceneLetter(footswitch)} aria-label={footswitchAssignmentPending ? "Waiting for a footswitch assignment" : footswitch === undefined ? "Assign a footswitch" : `Assigned to footswitch ${sceneLetter(footswitch)}`} onClick={() => { setMenuOpen(false); onFootswitchAssignmentStart(!footswitchAssignmentPending); }} />
      <QcSceneSelector className="editor-scene" letter={sceneLetter(activeScene)} onPrevious={() => onSceneSelect((activeScene - 1 + sceneCount) % sceneCount)} onNext={() => onSceneSelect((activeScene + 1) % sceneCount)} />
      {hasPages && <span className="editor-pages" aria-label="Parameter pages"><button aria-label="Parameter page 1" onClick={() => onPageChange(0)}>1</button><button aria-label="Parameter page 2" onClick={() => onPageChange(1)}>2</button></span>}
      <button className="editor-bypass" aria-label={bypassed ? "Activate device" : "Bypass device"} aria-pressed={bypassed} onClick={onToggleBypass}>{/* Bypassed blocks show the barred ring, not the power symbol: a 72px accent ring plus a 48px white bar, not a link glyph. */}<QcEditorIcon kind={bypassed ? "bypass-muted" : "bypass"} /></button>
      <button className="editor-done" aria-label="Close parameter editor" onClick={onClose}><QcEditorIcon kind="confirm" /></button>
      {menuOpen && <div className="qc-block-context-menu qc-scroll-rail" role="menu" aria-label="Device contextual menu">{contextItems.map((item) => { const disabled = item.disabled || contextActionEnabled?.[item.action] === false; return <button key={item.action} role="menuitem" className={`${item.action === "remove" ? "is-danger" : ""}${item.separatorBefore ? " has-separator" : ""}${disabled ? " is-disabled" : ""}`} disabled={disabled} onClick={() => runContextAction(item.action)}><span><QcEditorIcon kind={item.icon}/></span>{item.label}</button>; })}</div>}
    </header>
    <div className={`coros-parameter-controls${parameters.length > 5 ? " is-two-row" : ""}`}>{parameters.map((parameter) => {
      const value = drafts[parameter.index] ?? parameter.normalizedValue ?? 0;
      const dialValue = props.referenceOnly ? referenceDialValue(details.name, parameter.name, value) : value;
      const detents = referenceDialSteps(details.name, parameter);
      const toggle = parameterControlKind(parameter) === "switch" || parameterControlKind(parameter) === "button";
      const nextToggle = value >= .5 ? 0 : 1;
      return <label key={parameter.index} className={`${toggle ? "is-toggle " : ""}${referenceParameterClass(parameter.name)}`}>
        <strong>{parameter.name.toUpperCase()}</strong>
        {toggle ? <button className="coros-toggle" disabled={disabled || !parameter.writable || parameter.enabled === false} aria-label={`${parameter.name}: ${parameterDisplay(parameter, value)}`} onClick={() => { onDraftChange(parameter, nextToggle); onCommit(parameter, nextToggle); }}><QcPillToggle index={value >= .5 ? 1 : 0} detent="var(--editor-color)" /><QcPillToggleLabels labels={parameter.options.length ? parameter.options : ["Off", "On"]} index={value >= .5 ? 1 : 0} /></button> : <><input type="range" min="0" max="1" step={parameterStep(parameter)} value={value} disabled={disabled || !parameter.writable || parameter.enabled === false} aria-label={`${parameter.name}: ${parameterDisplay(parameter, value)}`} onChange={(event) => onDraftChange(parameter, Number(event.target.value))} onPointerUp={(event) => onCommit(parameter, Number(event.currentTarget.value))} onKeyUp={(event) => { if (["ArrowUp", "ArrowRight", "ArrowDown", "ArrowLeft"].includes(event.key)) onCommit(parameter, Number(event.currentTarget.value)); }} /><span className="coros-dial"><QcRotaryDial value={dialValue} mode={detents ? "step" : "arc"} steps={detents} accent={accent} /></span><small>{parameterDisplay(parameter, value)}</small></>}
      </label>;
    })}</div>
    {footswitchAssignmentPending && <QcFootswitchAssignmentPrompt assignedLetter={footswitch === undefined ? undefined : sceneLetter(footswitch)} onCancel={() => onFootswitchAssignmentStart(false)} onUnassign={onUnassignFootswitch} />}
  </section>;
}

function CorOsReferenceSpecialEditor({ props, accent, menuOpen, setMenuOpen, contextItems, runContextAction }: {
  props: CorOsParameterEditorProps;
  accent: string;
  menuOpen: boolean;
  setMenuOpen: (open: boolean) => void;
  contextItems: ReturnType<typeof parameterContextMenuItems>;
  runContextAction: (action: ParameterEditorContextAction) => void;
}) {
  const { details, activeScene, scenes, bypassed, footswitch, onToggleBypass, onSceneSelect, onPageChange, footswitchAssignmentPending, onFootswitchAssignmentStart, onUnassignFootswitch, contextActionEnabled, onClose } = props;
  const sceneCount = Math.max(1, scenes.length);
  const sceneControl = <QcSceneSelector className="editor-scene" letter={sceneLetter(activeScene)} onPrevious={() => onSceneSelect((activeScene - 1 + sceneCount) % sceneCount)} onNext={() => onSceneSelect((activeScene + 1) % sceneCount)} />;
  const menu = menuOpen && <div className="qc-block-context-menu qc-scroll-rail" role="menu" aria-label="Device contextual menu">{contextItems.map((item) => { const disabled = item.disabled || contextActionEnabled?.[item.action] === false; return <button key={item.action} role="menuitem" className={`${item.action === "remove" ? "is-danger" : ""}${item.separatorBefore ? " has-separator" : ""}${disabled ? " is-disabled" : ""}`} disabled={disabled} onClick={() => runContextAction(item.action)}><span><QcEditorIcon kind={item.icon}/></span>{item.label}</button>; })}</div>;
  const more = <button className="editor-more" aria-label="Device contextual menu" aria-expanded={menuOpen} onClick={() => { onFootswitchAssignmentStart(false); setMenuOpen(!menuOpen); }}><QcUiIcon kind="more" /></button>;
  if (details.name === "212 UK C30 65 (M)") return <section className="coros-parameter-editor coros-reference-parameter-editor coros-special-editor coros-cab-editor editor-ukc30-cab" aria-label={`${details.name} parameter editor`}>
    <header className="cab-editor-header">{more}<span className="editor-title"><small>GUITAR CAB</small><strong style={{ color: accent }}>{details.name}<i className="cab-preset-mark"><QcPresetStackIcon /></i></strong></span><QcEditorFootswitchAssignment letter={footswitch === undefined ? "E" : sceneLetter(footswitch)} aria-label={footswitch === undefined ? "Assign a footswitch" : `Assigned to footswitch ${sceneLetter(footswitch)}`} />{sceneControl}<span className="cab-header-divider divider-one" /><button className="cab-header-mode is-active" onClick={() => onPageChange(0)}>CAB</button><button className="cab-header-mode" onClick={() => onPageChange(1)}>EQ</button><span className="cab-header-divider divider-two" /><button className={`editor-bypass${bypassed ? " is-bypassed" : ""}`} aria-label={bypassed ? "Activate device" : "Bypass device"} onClick={onToggleBypass}><QcEditorIcon kind="bypass" /></button><button className="editor-done" aria-label="Close parameter editor" onClick={onClose}><QcEditorIcon kind="confirm" /></button>{menu}</header>
    <div className="cab-halves">{[0, 1].map((side) => <section key={side}><div className="cab-values">{CAB_REFERENCE_VALUES.map((control) => <span key={control.name}>{control.name}<QcBarSlider value={control.value} origin={control.origin} /><b>{control.display}</b></span>)}</div><div className="cab-speaker"><QcCabMicrophoneTarget x={side ? 68.75 : 31.25} y={50} direction={side ? "left" : "right"} /></div><div className="cab-footer"><button className="cab-phase" disabled title="This captured model does not publish microphone phase controls." aria-label={`Microphone ${side + 1} phase unavailable`}><QcUiIcon kind="phase-invert" /></button><button className="cab-mic" disabled title="This captured model does not publish a microphone selector."><QcUiIcon kind="cab-previous" /><strong>{side ? "Ribbon 160" : "Dynamic 57"}</strong><QcUiIcon kind="cab-next" /></button><button className="cab-enable" disabled title="This captured model does not publish a microphone enable control."><i /></button></div></section>)}</div>
    {footswitchAssignmentPending && <QcFootswitchAssignmentPrompt assignedLetter={footswitch === undefined ? undefined : sceneLetter(footswitch)} onCancel={() => onFootswitchAssignmentStart(false)} onUnassign={onUnassignFootswitch} />}
  </section>;
  const gridLines = [1.902, 6.386, 9.918, 12.636, 15.082, 17.12, 19.022, 20.652, 31.386, 37.772, 42.255, 45.788, 48.505, 50.951, 52.989, 54.891, 56.522, 67.255, 73.641, 78.125, 81.658, 84.375, 86.821, 88.859, 90.761, 92.391];
  return <section className="coros-parameter-editor coros-reference-parameter-editor coros-special-editor coros-eq-editor editor-parametric-8" aria-label={`${details.name} parameter editor`}>
    <header>{more}<span className="editor-title is-eq"><small>EQ</small><strong style={{ color: accent }}>{details.name}<i className="eq-preset-mark"><QcPresetStackIcon /></i></strong></span><span className="editor-spacer" /><QcEditorFootswitchAssignment letter={footswitch === undefined ? "F" : sceneLetter(footswitch)} aria-label={footswitch === undefined ? "Assign a footswitch" : `Assigned to footswitch ${sceneLetter(footswitch)}`} />{sceneControl}<button className={`editor-bypass${bypassed ? " is-bypassed" : ""}`} aria-label={bypassed ? "Activate device" : "Bypass device"} onClick={onToggleBypass}><QcEditorIcon kind="bypass" /></button><button className="editor-done" aria-label="Close parameter editor" onClick={onClose}><QcEditorIcon kind="confirm" /></button>{menu}</header>
    <div className="eq-graph"><div className="eq-grid-lines" aria-hidden="true">{gridLines.map((left) => <i key={left} style={{ left: `${left}%` }} />)}</div><div className="eq-axis-labels"><b>100</b><b>1k</b><b>10k</b></div><span>1</span><span>2</span><span>3</span><span>4</span></div><div className="eq-tabs">{[1, 2, 3, 4, 5, 6, 7, 8].map((number) => <span key={number}>{number}</span>)}<span>OUT</span></div><div className="eq-values"><span className="eq-type">TYPE<strong><svg className="eq-filter-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M2 16h6l7-8h7" /></svg><span>{EQ_FILTER_TYPE_LABELS[4].toUpperCase()}</span><QcUiIcon kind="down" /></strong></span><span className="eq-dial-value">GAIN<QcRotaryDial className="eq-control-dial" value={0.5} accent={accent} /><strong>0.0 <small>dB</small></strong></span><span className="eq-dial-value">FREQ<QcRotaryDial className="eq-control-dial" value={0.1565} accent={accent} /><strong>50 <small>Hz</small></strong></span><span className="eq-dial-value">Q<QcRotaryDial className="eq-control-dial" value={0.0779} accent={accent} /><strong>0.71</strong></span><span className="eq-bypass-value">BYPASS 1<strong><QcEditorIcon kind="band-power" /></strong></span></div>
    {footswitchAssignmentPending && <QcFootswitchAssignmentPrompt assignedLetter={footswitch === undefined ? undefined : sceneLetter(footswitch)} onCancel={() => onFootswitchAssignmentStart(false)} onUnassign={onUnassignFootswitch} />}
  </section>;
}

function EqEditorBody(props: { parameters: BlockParameter[]; drafts: Record<number, number>; accent: string; disabled?: boolean; selectedBand: number; outputSelected: boolean; onSelectBand: (band: number) => void; onSelectOutput: () => void } & ControlCallbacks) {
  const graphicBandCount = props.parameters.filter((parameter) => /^(?:N)?(?:65|125|250|500|1K|2K|4K|8K|16K)\s*HZ$/i.test(parameter.name.replace(/\s/g, ""))).length;
  return eqBands(props.parameters).length
    ? <ParametricEqEditorBody {...props} />
    : graphicBandCount >= 5 || props.parameters.filter((parameter) => parameter.type.toLowerCase() === "fader").length >= 5
      ? <GraphicEqEditorBody {...props} />
      : <div className="parameter-controls eq-simple-controls">{props.parameters.slice(0, 10).map((parameter, slot) => <ParameterControl key={parameter.index} parameter={parameter} value={props.drafts[parameter.index] ?? parameter.normalizedValue ?? 0} slot={slot} accent={props.accent} disabled={props.disabled} onDraftChange={props.onDraftChange} onCommit={props.onCommit} />)}</div>;
}

function IrLoaderChannel({ base, allParameters, drafts, accent, disabled, onDraftChange, onCommit }: {
  base: 0 | 8;
  allParameters: BlockParameter[];
  drafts: Record<number, number>;
  accent: string;
  disabled?: boolean;
} & ControlCallbacks) {
  const byIndex = (index: number) => allParameters.find((parameter) => parameter.index === index);
  const mute = byIndex(base);
  const phase = byIndex(base + 1);
  const impulse = byIndex(base + 2);
  const controls = [3, 4, 5, 6, 7].map((offset) => byIndex(base + offset)).filter((parameter): parameter is BlockParameter => Boolean(parameter?.normalizedValue != null));
  const muteValue = mute ? drafts[mute.index] ?? mute.normalizedValue ?? 0 : 0;
  const phaseValue = phase ? drafts[phase.index] ?? phase.normalizedValue ?? 0 : 0;
  const impulseValue = impulse?.normalizedValue ?? 0;
  const impulseIndex = impulse?.options.length ? Math.round(impulseValue * (impulse.options.length - 1)) : 0;
  const impulseName = impulse?.options[impulseIndex] ?? impulse?.displayValue ?? "No IR loaded";
  const commitBinary = (parameter: BlockParameter | undefined, value: number) => {
    if (!parameter || disabled || !parameter.writable || parameter.enabled === false) return;
    onDraftChange(parameter, value);
    onCommit(parameter, value);
  };
  const selectImpulse = (offset: number) => {
    if (!impulse || disabled || !impulse.writable || impulse.enabled === false || impulse.options.length < 2) return;
    const nextIndex = (impulseIndex + offset + impulse.options.length) % impulse.options.length;
    const next = nextIndex / (impulse.options.length - 1);
    onDraftChange(impulse, next);
    onCommit(impulse, next);
  };
  return <section className={`ir-loader-channel${muteValue >= .5 ? " is-bypassed" : ""}`} style={{ "--ir-accent": accent } as CSSProperties}>
    <div className="ir-loader-selector">
      <div className="ir-loader-state">
        <button className={muteValue < .5 ? "is-active" : ""} disabled={disabled || !mute?.writable || mute?.enabled === false} aria-label={`${muteValue >= .5 ? "Enable" : "Disable"} impulse`} aria-pressed={muteValue < .5} onClick={() => commitBinary(mute, muteValue >= .5 ? 0 : 1)}><QcUiIcon kind="power-toggle" /></button>
        <button className={phaseValue >= .5 ? "is-active" : ""} disabled={disabled || !phase?.writable || phase?.enabled === false} aria-label="Invert impulse phase" aria-pressed={phaseValue >= .5} onClick={() => commitBinary(phase, phaseValue >= .5 ? 0 : 1)}><QcUiIcon kind="phase-invert" /></button>
      </div>
      <div className="ir-loader-name"><small>{base === 0 ? "ROTATE TOP RIGHT SWITCH" : "IMPULSE RESPONSE"}</small><strong title={impulseName}>{impulseName}</strong></div>
      <div className="ir-loader-arrows">
        <button disabled={disabled || !impulse?.writable || impulse?.enabled === false || impulse.options.length < 2} aria-label="Previous impulse response" onClick={() => selectImpulse(-1)}><QcUiIcon kind="up" /></button>
        <button disabled={disabled || !impulse?.writable || impulse?.enabled === false || impulse.options.length < 2} aria-label="Next impulse response" onClick={() => selectImpulse(1)}><QcUiIcon kind="down" /></button>
      </div>
    </div>
    <div className="ir-loader-controls">{controls.map((parameter, slot) => <ParameterControl key={parameter.index} parameter={parameter} value={drafts[parameter.index] ?? parameter.normalizedValue ?? 0} slot={slot} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} />)}</div>
  </section>;
}

function IrLoaderEditorBody({ name, parameters, page, drafts, accent, disabled, onDraftChange, onCommit }: {
  name: string;
  parameters: BlockParameter[];
  page: number;
  drafts: Record<number, number>;
  accent: string;
  disabled?: boolean;
} & ControlCallbacks) {
  if (page === 0) return <div className="ir-loader-body"><IrLoaderChannel base={0} allParameters={parameters} drafts={drafts} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} />{/dual/i.test(name) && <IrLoaderChannel base={8} allParameters={parameters} drafts={drafts} accent={QC_COLORS.category.pitch} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} />}</div>;
  const roomControls = parameters.filter((parameter) => parameter.index >= 16 && parameter.index <= 21 && parameter.normalizedValue != null);
  return <div className="parameter-controls ir-loader-room-controls">{roomControls.map((parameter, slot) => <ParameterControl key={parameter.index} parameter={parameter} value={drafts[parameter.index] ?? parameter.normalizedValue ?? 0} slot={slot} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} />)}</div>;
}

function FamilyWidget({ family, name, availableSlots }: { family: ParameterEditorFamily; name: string; availableSlots: number }) {
  if (family === "eq" && availableSlots >= 3) {
    return <div className="parameter-family-widget eq-response" style={{ "--widget-span": availableSlots } as CSSProperties} aria-label="EQ response graph">
      <span>RESPONSE</span><svg viewBox="0 0 300 80" preserveAspectRatio="none" aria-hidden="true"><path className="eq-zero" d="M0 40H300"/><path className="eq-curve" d="M0 43 C35 43 45 25 75 25 S110 55 145 48 S185 18 215 28 S255 43 300 39"/><circle cx="74" cy="25" r="4"/><circle cx="145" cy="48" r="4"/><circle cx="216" cy="28" r="4"/></svg>
    </div>;
  }
  if (family === "synth" && availableSlots >= 3) {
    return <div className="parameter-family-widget synth-sections" style={{ "--widget-span": availableSlots } as CSSProperties} aria-label="Synth signal sections">
      <span>ARPEGGIATOR</span><b><QcUiIcon kind="next" /></b><span>OSCILLATOR</span><b><QcUiIcon kind="next" /></b><span>FILTER</span><QcEditorIcon kind="waveform" />
    </div>;
  }
  return null;
}

type ControlCallbacks = Pick<CorOsParameterEditorProps, "onDraftChange" | "onCommit">;

function cabMicrophoneLabel(value: string) {
  return value.includes("_") ? value.slice(value.lastIndexOf("_") + 1) : value;
}

function CabChannel({ side, allParameters, drafts, accent, disabled, onDraftChange, onCommit, onCommitBatch }: {
  side: 0 | 1;
  allParameters: BlockParameter[];
  drafts: Record<number, number>;
  accent: string;
  disabled?: boolean;
} & ControlCallbacks & Pick<CorOsParameterEditorProps, "onCommitBatch">) {
  const base = side * 8;
  const byIndex = (index: number) => allParameters.find((parameter) => parameter.index === index);
  const bypass = byIndex(base);
  const microphone = byIndex(base + 1);
  const level = byIndex(base + 2);
  const pan = byIndex(base + 3);
  const distance = byIndex(base + 4);
  const position = byIndex(base + 5);
  const phase = byIndex(base + 6);
  const controls = [position, distance, level, pan].filter((parameter): parameter is BlockParameter => Boolean(parameter));
  const positionValue = position ? drafts[position.index] ?? position.normalizedValue ?? .5 : .5;
  const distanceValue = distance ? drafts[distance.index] ?? distance.normalizedValue ?? .5 : .5;
  const phaseValue = phase ? drafts[phase.index] ?? phase.normalizedValue ?? 0 : 0;
  const bypassValue = bypass ? drafts[bypass.index] ?? bypass.normalizedValue ?? 0 : 0;
  const microphoneValue = microphone ? drafts[microphone.index] ?? microphone.normalizedValue ?? 0 : 0;
  const microphoneIndex = microphone?.options.length ? Math.round(microphoneValue * (microphone.options.length - 1)) : 0;
  const microphoneName = microphone?.options[microphoneIndex] ?? microphone?.displayValue ?? "Microphone";
  const speakerDrag = useRef<{ pointerId: number; position: number; distance: number } | undefined>(undefined);
  const setBinary = (parameter: BlockParameter | undefined, value: number) => {
    if (!parameter || disabled || !parameter.writable || parameter.enabled === false) return;
    onDraftChange(parameter, value);
    onCommit(parameter, value);
  };
  const selectMicrophone = (offset: number) => {
    if (!microphone || disabled || !microphone.writable || microphone.enabled === false || microphone.options.length < 2) return;
    const nextIndex = (microphoneIndex + offset + microphone.options.length) % microphone.options.length;
    const next = nextIndex / (microphone.options.length - 1);
    onDraftChange(microphone, next);
    onCommit(microphone, next);
  };
  const speakerEditable = !disabled && Boolean(position?.writable && distance?.writable)
    && position?.enabled !== false && distance?.enabled !== false;
  const updateSpeaker = (event: PointerEvent<HTMLButtonElement>) => {
    if (!speakerEditable || !speakerDrag.current || speakerDrag.current.pointerId !== event.pointerId || !position || !distance) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    const { position: nextPosition, distance: nextDistance } = cabMicrophoneValuesAt(side, (event.clientX - bounds.left) / bounds.width, (event.clientY - bounds.top) / bounds.height);
    speakerDrag.current.position = nextPosition;
    speakerDrag.current.distance = nextDistance;
    onDraftChange(position, nextPosition);
    onDraftChange(distance, nextDistance);
  };
  const finishSpeaker = (event: PointerEvent<HTMLButtonElement>) => {
    const drag = speakerDrag.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    speakerDrag.current = undefined;
    if (event.currentTarget.hasPointerCapture?.(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    if (!speakerEditable || !position || !distance) return;
    if (onCommitBatch) onCommitBatch([{ parameter: position, value: drag.position }, { parameter: distance, value: drag.distance }]);
    else onCommit(position, drag.position);
  };
  return <section className={`cab-channel${bypassValue >= .5 ? " is-bypassed" : ""}`} style={{ "--cab-accent": side ? QC_COLORS.category.pitch : accent } as CSSProperties}>
    <div className="cab-channel-controls">
      {controls.map((parameter) => <ParameterControl key={parameter.index} parameter={parameter} value={drafts[parameter.index] ?? parameter.normalizedValue ?? 0} slot={allParameters.indexOf(parameter)} accent={side ? QC_COLORS.category.pitch : accent} disabled={disabled} variant="bar" onDraftChange={onDraftChange} onCommit={onCommit} />)}
    </div>
    <button className="cab-speaker" disabled={!speakerEditable} aria-label={`Microphone ${side + 1} position ${position?.displayValue ?? ""}, distance ${distance?.displayValue ?? ""}`} onPointerDown={(event) => {
      if (!speakerEditable) return;
      event.currentTarget.setPointerCapture?.(event.pointerId);
      speakerDrag.current = { pointerId: event.pointerId, position: positionValue, distance: distanceValue };
      updateSpeaker(event);
    }} onPointerMove={updateSpeaker} onPointerUp={finishSpeaker} onPointerCancel={finishSpeaker}><QcCabMicrophoneTarget {...cabMicrophonePlacement(side, positionValue, distanceValue)} direction={side ? "left" : "right"} /></button>
    <div className="cab-channel-footer">
      <button className={`cab-phase${phaseValue >= .5 ? " is-active" : ""}`} disabled={disabled || !phase?.writable || phase?.enabled === false} aria-label={`Invert microphone ${side + 1} phase`} aria-pressed={phaseValue >= .5} onClick={() => setBinary(phase, phaseValue >= .5 ? 0 : 1)}><QcUiIcon kind="phase-invert" /></button>
      <div className="cab-microphone-selector">
        <button disabled={disabled || !microphone?.writable || microphone?.enabled === false || microphone.options.length < 2} aria-label={`Previous microphone for slot ${side + 1}`} onClick={() => selectMicrophone(-1)}><QcUiIcon kind="cab-previous" /></button>
        <strong title={microphoneName}>{cabMicrophoneLabel(microphoneName)}</strong>
        <button disabled={disabled || !microphone?.writable || microphone?.enabled === false || microphone.options.length < 2} aria-label={`Next microphone for slot ${side + 1}`} onClick={() => selectMicrophone(1)}><QcUiIcon kind="cab-next" /></button>
      </div>
      <button className={`cab-channel-power${bypassValue < .5 ? " is-active" : ""}`} disabled={disabled || !bypass?.writable || bypass?.enabled === false} aria-label={`${bypassValue >= .5 ? "Enable" : "Disable"} cab microphone ${side + 1}`} aria-pressed={bypassValue < .5} onClick={() => setBinary(bypass, bypassValue >= .5 ? 0 : 1)}><i /></button>
    </div>
  </section>;
}

function CabEqBody({ parameters, drafts, accent, disabled, onDraftChange, onCommit }: {
  parameters: BlockParameter[];
  drafts: Record<number, number>;
  accent: string;
  disabled?: boolean;
} & ControlCallbacks) {
  const controls = [16, 18, 17].map((index) => parameters.find((parameter) => parameter.index === index)).filter((parameter): parameter is BlockParameter => Boolean(parameter));
  const value = (parameter: BlockParameter | undefined, fallback: number) => parameter ? drafts[parameter.index] ?? parameter.normalizedValue ?? fallback : fallback;
  const highPass = controls.find((parameter) => parameter.index === 16);
  const lowPass = controls.find((parameter) => parameter.index === 17);
  const highX = 6 + value(highPass, 0) * 42;
  const lowX = 52 + value(lowPass, 1) * 42;
  return <div className="cab-eq-body">
    <div className="cab-eq-graph" aria-label="Cab high-pass and low-pass response">
      <svg viewBox="0 0 100 50" preserveAspectRatio="none" aria-hidden="true"><path className="cab-eq-grid" d="M5 5V44M27 5V44M50 5V44M73 5V44M95 5V44M5 12H95M5 25H95M5 38H95"/><path className="cab-eq-curve" d={`M3 45 C${highX - 5} 45 ${highX - 3} 25 ${highX} 25 H${lowX} C${lowX + 3} 25 ${lowX + 5} 45 97 45`}/></svg>
      <span className="cab-eq-node is-high" style={{ left: `${highX}%` }}>H</span>
      <span className="cab-eq-node is-low" style={{ left: `${lowX}%` }}>L</span>
    </div>
    <div className="cab-eq-controls">{controls.map((parameter, slot) => <ParameterControl key={parameter.index} parameter={parameter} value={drafts[parameter.index] ?? parameter.normalizedValue ?? 0} slot={slot} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} />)}</div>
  </div>;
}

function CabEditorBody({ parameters, page, drafts, accent, disabled, onDraftChange, onCommit, onCommitBatch }: {
  parameters: BlockParameter[];
  page: number;
  drafts: Record<number, number>;
  accent: string;
  disabled?: boolean;
} & ControlCallbacks & Pick<CorOsParameterEditorProps, "onCommitBatch">) {
  if (page === 1) return <CabEqBody parameters={parameters} drafts={drafts} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} />;
  return <div className="cab-editor-body">
    <CabChannel side={0} allParameters={parameters} drafts={drafts} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} onCommitBatch={onCommitBatch} />
    <CabChannel side={1} allParameters={parameters} drafts={drafts} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} onCommitBatch={onCommitBatch} />
  </div>;
}

/**
 * The Splitter and Mixer editor bodies, matching CorOsRoutingScreen's
 * "splitter-editor"/"mixer-editor" views in coros-screen-fixtures.tsx,
 * reused here as a live, real-data body instead of that screen's static
 * fixture values.
 *
 * Parameter identity is never assumed by index: every field below is found by
 * a case-insensitive name match against the block's own BlockDetails.parameters,
 * matching the model catalogue (model 10004 "Splitter" and model 11000
 * "Mixer") and the manual's own SPLITTER/MIXER PARAMETERS section. Model
 * 11000's own catalogue also carries a seventh, hidden "SPLIT MODE" parameter
 * that neither the manual nor the fixture draws a control for - left
 * unrendered here to match, not dropped by oversight.
 */
type RoutingFieldLayout = { name: string; kind: "knob" | "toggle"; positions?: number; knobClass?: string };

const SPLITTER_FIELDS: readonly RoutingFieldLayout[] = [
  { name: "TYPE", kind: "toggle", positions: 3 },
  { name: "STEREO", kind: "toggle", positions: 2 },
  { name: "BALANCE", kind: "knob" },
  { name: "LEVEL TO A", kind: "knob", knobClass: "angle-a" },
  { name: "LEVEL TO B", kind: "knob", knobClass: "angle-b" },
  { name: "FREQUENCY", kind: "knob", knobClass: "frequency" },
  { name: "MODE", kind: "toggle", positions: 2 }
];

const MIXER_FIELDS: readonly RoutingFieldLayout[] = [
  { name: "LEVEL A", kind: "knob", knobClass: "level-a" },
  { name: "PAN A", kind: "knob", knobClass: "pan" },
  { name: "LEVEL B", kind: "knob", knobClass: "level-b" },
  { name: "PAN B", kind: "knob", knobClass: "pan" },
  { name: "PHASE", kind: "toggle", positions: 2 },
  { name: "MIXER LEVEL", kind: "knob", knobClass: "level-a" }
];

function routingParameter(parameters: BlockParameter[], name: string): BlockParameter | undefined {
  return parameters.find((parameter) => parameter.name.toLowerCase() === name.toLowerCase());
}

/** The splitter/mixer knob, positioned exactly where CorOsRoutingScreen puts its
 *  (non-interactive) fixture knob - via the same .splitter-knob class - but as
 *  a real <button> so it can carry the same pointer/wheel/keyboard interaction
 *  ParameterControl's own knob uses. QcRotaryDial itself stays presentational;
 *  the position class moves to this wrapper, which official-routing.css sizes
 *  to fill it. That is the one DOM change from the fixture's markup. */
function RoutingKnob({ parameter, knobClass, mixer, accent, disabled, value, onDraftChange, onCommit }: {
  parameter: BlockParameter;
  knobClass?: string;
  mixer: boolean;
  accent: string;
  disabled?: boolean;
  value: number;
} & ControlCallbacks) {
  const controlDisabled = Boolean(disabled || !parameter.writable || parameter.enabled === false);
  const gesture = useParameterRotary(parameter, value, controlDisabled, (next) => onDraftChange(parameter, next), (next) => onCommit(parameter, next));
  const className = `splitter-knob${mixer ? " mixer-knob" : ""}${knobClass ? ` ${knobClass}` : ""}`;
  const display = parameterDisplay(parameter, value);
  return <QcRotaryControl className={className} disabled={controlDisabled} label={`${parameter.name}: ${display}`} {...parameterSliderRange(value)} valueText={display} gesture={gesture}>
    <QcRotaryDial value={value} accent={accent} />
  </QcRotaryControl>;
}

/**
 * The TYPE/STEREO/MODE/PHASE pill toggles. QcPillToggle itself stays a bare,
 * aria-hidden SVG - exactly as CorOsRoutingScreen's fixture renders it, as a
 * direct child of this <label> - so official-routing.css's
 * ".splitter-controls label>.qc-pill-toggle" position rule needs no
 * adaptation at all; only the <label> gains a click/keyboard handler.
 *
 * The side text listing every option is confirmed for the two-way case only:
 * the selected option renders bold on the first line and the single other
 * option dimmed in <em> beneath it. Generalising that to "selected option
 * bold, then every other option in original order, each dimmed" is the
 * defensible reading for a three-way pill (TYPE), but which of its two
 * non-selected lines (if either) the unit would render undimmed is not
 * confirmed, only interpolated.
 */
function RoutingToggle({ parameter, positions, accent, disabled, value, onDraftChange, onCommit }: {
  parameter: BlockParameter;
  positions: number;
  accent: string;
  disabled?: boolean;
  value: number;
} & ControlCallbacks) {
  const optionCount = Math.max(positions, parameter.options.length || positions);
  const index = Math.min(optionCount - 1, Math.round(value * (optionCount - 1)));
  const controlDisabled = Boolean(disabled || !parameter.writable || parameter.enabled === false);
  const cycle = () => {
    if (controlDisabled) return;
    const nextIndex = (index + 1) % optionCount;
    const next = nextIndex / (optionCount - 1);
    onDraftChange(parameter, next);
    onCommit(parameter, next);
  };
  const options = parameter.options;
  const remaining = options.length ? options.filter((_, optionIndex) => optionIndex !== index) : [];
  return <button type="button" className={`routing-toggle${controlDisabled ? " is-disabled" : ""}`} disabled={controlDisabled}
    aria-label={`${parameter.name}: ${parameterDisplay(parameter, value)}`} onClick={cycle}>
    <strong>{parameter.name}</strong>
    <QcPillToggle positions={optionCount} index={index} detent={accent} />
    {options.length > 0 && <span className="splitter-toggle"><b>{options[index]}{remaining.map((label) => <Fragment key={label}><br />{<em>{label}</em>}</Fragment>)}</b></span>}
  </button>;
}

function RoutingNodeEditorBody({ routingNode, parameters, drafts, disabled, onDraftChange, onCommit }: {
  routingNode: "splitter" | "mixer";
  parameters: BlockParameter[];
  drafts: Record<number, number>;
  disabled?: boolean;
} & ControlCallbacks) {
  const mixer = routingNode === "mixer";
  const fields = mixer ? MIXER_FIELDS : SPLITTER_FIELDS;
  const emptySlots = mixer ? 4 : 3;
  const accent = mixer ? QC_COLORS.reference.mixerAccent : QC_COLORS.reference.splitterAccent;
  return <div className={`splitter-controls${mixer ? " mixer-controls" : ""}`}>
    {fields.map((field) => {
      const parameter = routingParameter(parameters, field.name);
      // Every name above is confirmed present on the real model 10004/11000
      // catalogue; a missing match means a stale or legacy-modelId block
      // rather than a typo, and renders as an empty cell rather than guessing
      // an index.
      if (!parameter) return <span key={field.name} className="splitter-empty" aria-hidden="true" />;
      const value = drafts[parameter.index] ?? parameter.normalizedValue ?? 0;
      if (field.kind === "toggle") return <RoutingToggle key={field.name} parameter={parameter} positions={field.positions ?? 2} accent={accent} disabled={disabled} value={value} onDraftChange={onDraftChange} onCommit={onCommit} />;
      return <label key={field.name} className={Boolean(disabled || !parameter.writable || parameter.enabled === false) ? "is-disabled" : ""}>
        <strong>{parameter.name}</strong>
        <RoutingKnob parameter={parameter} knobClass={field.knobClass} mixer={mixer} accent={accent} disabled={disabled} value={value} onDraftChange={onDraftChange} onCommit={onCommit} />
        <small>{parameterDisplay(parameter, value)}</small>
      </label>;
    })}
    {Array.from({ length: emptySlots }, (_, index) => <span key={`empty-${index}`} className="splitter-empty" aria-hidden="true" />)}
  </div>;
}

/** The model-name size the editor header sets on one line, as a share of the editor's width (live-surface.css). */
const EDITOR_NAME_CQW = 2.375;
const EDITOR_CAB_NAME_CQW = 2.1;

/**
 * True when the model name cannot be set on a single line of the header's
 * title box at its one-line size. The unit then runs the category and the name
 * together as one wrapped text in the smaller size (a 412 Brit TV GB75Hz 69
 * (ST) cab reads "GUITAR CAB 412 Brit" over "TV GB75Hz 69 (ST)"), where a name
 * that fits keeps its own line under the category. The box is measured in the
 * editor's own width, so the answer is the same at every screen size.
 */
function useWrappedEditorTitle(name: string, nameCqw: number) {
  const ref = useRef<HTMLSpanElement>(null);
  const [wrapped, setWrapped] = useState(false);
  useLayoutEffect(() => {
    const box = ref.current;
    if (!box) return;
    const measure = () => {
      const editor = box.closest<HTMLElement>(".coros-parameter-editor");
      const context = typeof document === "undefined" ? null : document.createElement("canvas").getContext?.("2d") ?? null;
      if (!editor || !context || !box.clientWidth) { setWrapped(name.length > 20); return; }
      const style = getComputedStyle(box);
      const available = box.clientWidth - parseFloat(style.paddingLeft || "0") - parseFloat(style.paddingRight || "0");
      context.font = `400 ${nameCqw * editor.clientWidth / 100}px ${style.fontFamily}`;
      setWrapped(context.measureText(name).width > available);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(box);
    return () => observer.disconnect();
  }, [name, nameCqw]);
  return { ref, wrapped };
}

function CorOsParameterEditorContent(props: CorOsParameterEditorProps) {
  const { details, drafts, accent: categoryAccent, activeScene, scenes, bypassed, footswitch, routingNode, disabled, page, onPageChange, onDraftChange, onCommit, onCommitBatch, onToggleBypass, onSceneSelect, footswitchAssignmentPending, onFootswitchAssignmentStart, onUnassignFootswitch, onContextAction, clipboardModelId, contextActionEnabled, onClose } = props;
  const [menuOpen, setMenuOpen] = useState(Boolean(props.initialMenuOpen));
  const onMenuOpenChange = props.onMenuOpenChange;
  useEffect(() => { onMenuOpenChange?.(menuOpen); }, [menuOpen, onMenuOpenChange]);
  useEffect(() => () => onMenuOpenChange?.(false), [onMenuOpenChange]);
  const [selectedEqBand, setSelectedEqBand] = useState(1);
  const [eqOutputSelected, setEqOutputSelected] = useState(false);
  const familyForTitle = parameterEditorFamily(details.category);
  const title = useWrappedEditorTitle(details.name, familyForTitle === "cab" ? EDITOR_CAB_NAME_CQW : EDITOR_NAME_CQW);
  useEffect(() => { setSelectedEqBand(1); setEqOutputSelected(false); }, [details.modelId, details.name]);
  const accent = parameterEditorAccent(details.name, categoryAccent);
  const family = parameterEditorFamily(details.category);
  const writable = details.parameters.filter((parameter) => parameter.normalizedValue !== null || ((family === "cab" || family === "ir") && parameter.type.toLowerCase() === "string"));
  const pageSize = parameterEditorPageSize(details.category, details.parameters);
  const pageCount = family === "cab" ? 2 : family === "ir" ? (writable.some((parameter) => parameter.index >= 16 && parameter.index <= 21 && parameter.normalizedValue != null) ? 2 : 1) : parameterEditorPageCount(writable, pageSize);
  const safePage = Math.min(page, pageCount - 1);
  const screenSlots = parameterEditorPageSlots(writable, safePage, pageSize);
  const shown = family === "cab" || family === "ir" ? writable : screenSlots.filter((parameter): parameter is BlockParameter => parameter !== undefined);
  const tabs = family === "eq" ? [] : routingNode ? [] : parameterEditorTabs(details.name, details.category, pageCount);
  const fullScreen = parameterEditorIsFullScreen(details.category);
  const availableSlots = Math.max(0, 10 - shown.length);
  // The unit draws the second row only when a control lands in it, and a
  // divider only after a control that exists: a block with five or fewer
  // controls is a single short panel, not a half-empty ten-cell grid.
  const singleRow = pageSize === 10 && family !== "synth" && !screenSlots.some((parameter, slot) => parameter && slot >= 5);
  const contextItems = parameterContextMenuItems(details, clipboardModelId);
  const runContextAction = (action: ParameterEditorContextAction) => { setMenuOpen(false); onContextAction(action); };
  if (props.referenceOnly && (details.name === "212 UK C30 65 (M)" || details.name === "Parametric-8")) return <CorOsReferenceSpecialEditor props={props} accent={accent} menuOpen={menuOpen} setMenuOpen={setMenuOpen} contextItems={contextItems} runContextAction={runContextAction} />;
  if (REFERENCE_STANDARD_EDITORS.has(details.name)) return <CorOsReferenceStandardEditor props={props} accent={accent} parameters={screenSlots.filter((parameter): parameter is BlockParameter => parameter !== undefined)} menuOpen={menuOpen} setMenuOpen={setMenuOpen} contextItems={contextItems} runContextAction={runContextAction} />;
  return <section className={`coros-parameter-editor family-${family}${fullScreen ? " is-full-screen" : ""}${tabs.length ? " has-tabs" : ""}${routingNode ? ` is-routing-node is-${routingNode}` : ""}`} style={{ "--parameter-accent": accent } as CSSProperties} aria-label={`${details.name} parameter editor`}>
    <header className="parameter-editor-header">
      <button className="parameter-more" disabled={Boolean(routingNode)} aria-label={routingNode ? "Routing node options unavailable" : "Device contextual menu"} aria-expanded={routingNode ? false : menuOpen} onClick={() => { onFootswitchAssignmentStart(false); setMenuOpen((open) => !open); }}><i /><i /><i /></button>
      <span ref={title.ref} className={`parameter-device-name${title.wrapped ? " is-wrapped" : ""}`}><small>{editorCategoryLabel(details.category, details.name)}</small><strong style={{ color: accent }}>{details.name}</strong></span>
      <button className={`parameter-footswitch${footswitch === undefined ? "" : " is-assigned"}${footswitchAssignmentPending ? " is-assigning" : ""}`} disabled={Boolean(routingNode)} aria-label={routingNode ? `${details.name} has no footswitch assignment` : footswitchAssignmentPending ? "Waiting for a footswitch assignment" : footswitch === undefined ? "Assign a footswitch" : `Assigned to footswitch ${sceneLetter(footswitch)}`} aria-pressed={footswitchAssignmentPending} onClick={() => { setMenuOpen(false); onFootswitchAssignmentStart(!footswitchAssignmentPending); }}><QcEditorIcon kind="footswitch" /><span>{routingNode ? "–" : footswitchAssignmentPending ? "?" : footswitch === undefined ? "?" : sceneLetter(footswitch)}</span></button>
      <QcSceneSelector className="parameter-scene" letter={sceneLetter(activeScene)} onPrevious={() => onSceneSelect((activeScene - 1 + Math.max(1, scenes.length)) % Math.max(1, scenes.length))} onNext={() => onSceneSelect((activeScene + 1) % Math.max(1, scenes.length))} />
      <i className="parameter-header-divider" aria-hidden="true" />
      <button className={`parameter-bypass${bypassed ? " is-bypassed" : ""}`} disabled={Boolean(routingNode)} aria-label={routingNode ? `${details.name} bypass is controlled by routing` : bypassed ? "Activate device" : "Bypass device"} aria-pressed={routingNode ? undefined : bypassed} onClick={onToggleBypass}><QcEditorIcon kind={bypassed ? "bypass-muted" : "bypass"} /></button>
      <button className="parameter-close" aria-label="Close parameter editor" onClick={onClose}><QcEditorIcon kind="confirm" /></button>
      {!routingNode && menuOpen && <div className="qc-block-context-menu qc-scroll-rail" role="menu" aria-label="Device contextual menu">
        {contextItems.map((item) => { const disabled = item.disabled || contextActionEnabled?.[item.action] === false; return <button key={item.action} role="menuitem" className={`${item.action === "remove" ? "is-danger" : ""}${item.separatorBefore ? " has-separator" : ""}${disabled ? " is-disabled" : ""}`} disabled={disabled} onClick={() => runContextAction(item.action)}><span><QcEditorIcon kind={item.icon}/></span>{item.label}</button>; })}
      </div>}
    </header>
    {tabs.length > 0 && <nav className="parameter-tabs" aria-label="Parameter tabs">{tabs.map((label, index) => <button key={label} className={safePage === index ? "is-active" : ""} aria-current={safePage === index ? "page" : undefined} onClick={() => onPageChange(index)}>{label}</button>)}</nav>}
    {routingNode ? <RoutingNodeEditorBody routingNode={routingNode} parameters={details.parameters} drafts={drafts} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} /> : family === "eq" ? <EqEditorBody parameters={writable} drafts={drafts} accent={accent} disabled={disabled} selectedBand={selectedEqBand} outputSelected={eqOutputSelected} onSelectBand={(band) => { setSelectedEqBand(band); setEqOutputSelected(false); }} onSelectOutput={() => setEqOutputSelected(true)} onDraftChange={onDraftChange} onCommit={onCommit} /> : family === "cab" ? <CabEditorBody parameters={shown} page={safePage} drafts={drafts} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} onCommitBatch={onCommitBatch} /> : family === "ir" ? <IrLoaderEditorBody name={details.name} parameters={shown} page={safePage} drafts={drafts} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} /> : <div className={`parameter-controls${singleRow ? " is-single-row" : ""}`}>
      {screenSlots.map((parameter, slot) => parameter
        ? <ParameterControl key={parameter.index} parameter={parameter} value={drafts[parameter.index] ?? parameter.normalizedValue ?? 0} slot={slot} accent={accent} disabled={disabled} onDraftChange={onDraftChange} onCommit={onCommit} />
        : pageSize === 10 ? <span key={`empty-${slot}`} className="parameter-empty-slot" aria-hidden="true" /> : null)}
      <FamilyWidget family={family} name={details.name} availableSlots={availableSlots} />
      {!shown.length && <p className="parameter-empty">This device exposes no adjustable catalog parameters.</p>}
    </div>}
    {!routingNode && footswitchAssignmentPending && <QcFootswitchAssignmentPrompt assignedLetter={footswitch === undefined ? undefined : sceneLetter(footswitch)} onCancel={() => onFootswitchAssignmentStart(false)} onUnassign={onUnassignFootswitch} />}
  </section>;
}

function CorOsParameterEditorSession(props: CorOsParameterEditorProps) {
  const [workflow, setWorkflow] = useState<"save" | "looper">();
  const [name, setName] = useState("");
  const [assignments, setAssignments] = useState<number[]>([]);
  const [workflowPending, setWorkflowPending] = useState(false);
  const [workflowError, setWorkflowError] = useState("");
  const generation = useRef(0);
  const mounted = useRef(true);
  const busy = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; generation.current += 1; }; }, []);
  const owns = (captured: number) => mounted.current && generation.current === captured;
  const begin = (action: ParameterEditorContextAction) => {
    if (!mounted.current) return;
    if (action === "save-device-preset" && props.onSaveDevicePreset) {
      generation.current += 1; busy.current = false; setWorkflowPending(false);
      setName(""); setWorkflowError(""); setWorkflow("save"); return;
    }
    if (action === "assign-looper-actions" && props.onLoadLooperAssignments) {
      const captured = ++generation.current;
      busy.current = true;
      setAssignments([]);
      setWorkflowError(""); setWorkflow("looper"); setWorkflowPending(true);
      void props.onLoadLooperAssignments().then((value) => { if (owns(captured)) setAssignments(value); }, (error) => { if (owns(captured)) setWorkflowError(error instanceof Error ? error.message : String(error)); }).finally(() => { if (owns(captured)) { busy.current = false; setWorkflowPending(false); } });
      return;
    }
    props.onContextAction(action);
  };
  const renderedGeneration = generation.current;
  const closeWorkflow = () => { if (owns(renderedGeneration) && !busy.current) { generation.current += 1; setWorkflow(undefined); setWorkflowError(""); } };
  const save = async () => {
    if (!owns(renderedGeneration) || busy.current || !workflow) return;
    const captured = renderedGeneration;
    busy.current = true;
    setWorkflowPending(true); setWorkflowError("");
    try {
      if (workflow === "save") await props.onSaveDevicePreset?.(name.trim());
      else if (workflow === "looper") await props.onSaveLooperAssignments?.(assignments);
      if (owns(captured)) { generation.current += 1; busy.current = false; setWorkflow(undefined); setWorkflowPending(false); }
    } catch (error) { if (owns(captured)) setWorkflowError(error instanceof Error ? error.message : String(error)); }
    finally { if (owns(captured)) { busy.current = false; setWorkflowPending(false); } }
  };
  const nextLooperAction = (slot: number) => {
    setAssignments((current) => {
      if (current.length !== 8) return current;
      const next = (current[slot] + 1) % LOOPER_X_ASSIGNMENT_INDEX_NAMES.length;
      const other = current.indexOf(next);
      const copy = [...current];
      copy[other] = copy[slot];
      copy[slot] = next;
      return copy;
    });
  };
  return <ParameterSceneModeContext.Provider value={props.onToggleSceneMode}>
    <CorOsParameterEditorContent {...props} onContextAction={begin} />
    {workflow && <section className="coros-device-workflow" aria-label={workflow === "save" ? "Save device preset" : "Reassign Looper X actions"}>
      <header><button onClick={closeWorkflow} disabled={workflowPending}>×</button><strong>{workflow === "save" ? "Save Current Parameters As…" : "Reassign Looper X Actions"}</strong><button className="is-primary" disabled={workflowPending || (workflow === "save" ? !name.trim() : assignments.length !== 8)} onClick={() => void save()}>Save</button></header>
      {workflow === "save" ? <label><span>DEVICE PRESET NAME</span><input autoFocus maxLength={80} value={name} onChange={(event) => setName(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter" && name.trim()) void save(); }} placeholder="Name" /></label> : <>
        <p>Tap a footswitch to choose its Looper X action. Each action is assigned once.</p>
      <div className="coros-looper-assignment-grid">{assignments.map((action, slot) => <button key={slot} disabled={workflowPending} onClick={() => nextLooperAction(slot)}><b>{sceneLetter(slot)}</b><span>{LOOPER_X_ASSIGNMENT_INDEX_NAMES[action] ?? `Action ${action}`}</span></button>)}</div>
      </>}
      {workflowPending && <p>Synchronizing with the Quad Cortex…</p>}
      {workflowError && <p className="is-error">{workflowError}</p>}
    </section>}
  </ParameterSceneModeContext.Provider>;
}

export function CorOsParameterEditor(props: CorOsParameterEditorProps) {
  const owner = useRef({ context: props.editorContextKey, target: "", generation: 0 });
  const target = JSON.stringify([props.details.row, props.details.column, props.details.modelId, props.details.scene, props.activeScene]);
  if (owner.current.context !== props.editorContextKey || owner.current.target !== target) {
    owner.current = { context: props.editorContextKey, target, generation: owner.current.generation + 1 };
  }
  return <CorOsParameterEditorSession key={owner.current.generation} {...props} />;
}
