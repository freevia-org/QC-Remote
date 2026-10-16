import { formatFixed } from "./format-number.ts";
import { closeUserOpenedTarget, escapeUserOpenedTarget } from "./user-open-focus";
import { useEffect, useRef, useState } from "react";
import type { PresetSnapshot, TempoSettings } from "@qc-remote/client";
import { sceneLetter } from "@qc-remote/core";
import { QcPillToggle } from "./qc-pill-toggle";
import { QcRotaryControl } from "./qc-rotary-control";
import { QcRotaryDial } from "./qc-rotary-dial";
import { QcSceneSelector } from "./qc-shared-controls";
import { QcUiIcon } from "./theme-icons";
import { tempoVolumePosition } from "./tempo-volume-taper";
import { useRotaryValue } from "./use-rotary-gesture";
import { TEMPO_ROUTINGS, TEMPO_SOUNDS, tempoSubdivisionOptions, TEMPO_TIME_SIGNATURES, type TempoWorkflow } from "./use-tempo-workflow";
import "./official-tempo.css";

export interface TempoScreenProps {
  workflow: TempoWorkflow;
  screenGestureContextKey?: string;
  captureSessionCurrent?(): () => boolean;
  snapshot: PresetSnapshot;
  /** The existing preset-scoped BPM path (use-continuous-control-workflow.ts's
   * queueTempo) - reused as-is for the PRESET-mode dial, never duplicated. */
  queueTempo(bpm: number, source?: "Encoder" | "Tap"): void;
  onSelectScene(index: number): void;
  onClose(): void;
}

/** Drag distance per step on the Tempo screen's dials. Coarser than the block
 * editor's 7 px as shipped; not measured against the unit. */
const TEMPO_DIAL_PIXELS_PER_STEP = 4;
/** Wheel-settle debounce on the Tempo screen's dials. Longer than the block
 * editor's 55 ms as shipped; not measured against the unit. */
const TEMPO_DIAL_WHEEL_SETTLE_MS = 200;

/**
 * A rotary control over a plain `min`/`max`/`step` range, since TEMPO, VOLUME
 * and PAN are raw numbers rather than device-scaled parameters. The gesture is
 * the shared `useRotaryValue` (use-rotary-gesture.ts); only the two constants
 * above differ from the block editor's knobs.
 */
function TempoDial({ className, label, valueText, value, min, max, step, origin, position, disabled, accent, onDraftChange, onCommit }: {
  className?: string;
  /** The control's name: TEMPO, VOLUME or PAN. */
  label: string;
  /** The value as the screen prints it beside the dial. */
  valueText: string;
  value: number | undefined;
  min: number;
  max: number;
  step: number;
  /** Where the fill starts, in the dial's own units. Omit for the unit's usual fill from the minimum. */
  origin?: number;
  /** Where the dial is drawn, 0..1 along its sweep, when that is not the value's place in min..max (VOLUME's taper). */
  position?: (value: number) => number;
  disabled?: boolean;
  accent?: string;
  onDraftChange(value: number): void;
  onCommit(value: number): void;
}) {
  const clamp = (next: number) => Math.max(min, Math.min(max, next));
  const gesture = useRotaryValue({
    value: value ?? min, step, disabled: disabled || value === undefined, onDraftChange, onCommit,
    quantize: (next) => clamp(Math.round(next / step) * step),
    pixelsPerStep: TEMPO_DIAL_PIXELS_PER_STEP,
    wheelSettleMs: TEMPO_DIAL_WHEEL_SETTLE_MS
  });
  if (value === undefined) return <span className={`tempo-dial${className ? ` ${className}` : ""}`} role="status" aria-label={`${label}: Unavailable`}>—</span>;
  return <QcRotaryControl className={`tempo-dial${className ? ` ${className}` : ""}`} disabled={disabled || value === undefined} label={label} value={value ?? min} min={min} max={max} valueText={valueText} gesture={gesture}>
    {value !== undefined && <QcRotaryDial value={position ? position(clamp(value)) : (clamp(value) - min) / (max - min)} origin={origin === undefined ? undefined : (clamp(origin) - min) / (max - min)} accent={accent} />}
  </QcRotaryControl>;
}

/** BPM's real range, packages/typescript/qc-client/src/generated-domain.ts. */
const BPM_MIN = 40;
const BPM_MAX = 240;
/** The wire contract's own ranges, not a UI convenience: the Rust gateway
 * (packages/rust/quadcortex responses.rs / device/request.rs) decodes pan as
 * -1 + 2 * v and rejects a write outside "-60 through +9 dB" / "-1 through
 * +1". `volumeDb` on the wire is already the dB the unit's Tempo screen shows:
 * the gateway maps the raw parameter through the unit's measured taper
 * (commands.rs TEMPO_VOLUME_KNOTS, floor -60 at raw 0, +9 at raw 1), so the
 * label below prints the gateway's number with no conversion here and the dial
 * spans exactly the displayed range. Pan 0 is the centre ("C"), which is what
 * the unit shows for it. The L/R magnitude is shown as a percentage of full
 * travel; that suffix is not measured from the device (centre is). */
const PAN_MIN = -1;
const PAN_MAX = 1;
const PAN_CENTER = 0;
const VOLUME_MIN = -60;
const VOLUME_MAX = 9;

function panLabel(pan: number): string {
  const percent = Math.round((pan - PAN_CENTER) * 100);
  if (percent === 0) return "C";
  return percent < 0 ? `L${-percent}` : `R${percent}`;
}

/**
 * The device's own Tempo & Metronome screen - CorOsTempo in coros-screen-
 * fixtures.tsx, reached from App.tsx's "Tempo…" menu item (replacing a raw
 * `window.prompt`) and made real: every visible control now has a real
 * backend write behind it - see use-tempo-workflow.ts's own module comment
 * for the PRESET/GLOBAL BPM split and every field's exact gateway call.
 */
export function CorOsTempoScreen(props: TempoScreenProps) {
  const ownerKey = JSON.stringify([props.screenGestureContextKey, props.snapshot.presetIdentity, props.snapshot.setlistKey, props.snapshot.presetPosition, props.snapshot.activeScene, props.workflow.settings?.mode ?? "PRESET"]);
  return <CorOsTempoSession key={ownerKey} {...props} />;
}

function CorOsTempoSession({ workflow, snapshot, queueTempo, onSelectScene, onClose, captureSessionCurrent }: TempoScreenProps) {
  // A physical replacement can precede its host render. The keyed session also
  // retires drafts and rotary settle timers on preset, scene or tempo-mode changes.
  const [ownerCurrent] = useState(() => captureSessionCurrent?.() ?? (() => true));
  const { settings, tempoClock, pending, loading } = workflow;
  const mode = settings?.mode;
  const globalMode = mode === "GLOBAL";
  const ledEnabled = settings?.ledEnabled;
  const running = settings?.running;
  const volumeDb = settings?.volumeDb;
  const pan = settings?.pan;
  const timeSignature = settings?.timeSignature;
  const subdivision = settings?.subdivision;
  const sound = settings?.sound;
  const routing = settings?.routing;
  const committedBpm = globalMode ? settings?.globalBpm : mode === "PRESET" && !snapshot.unobserved?.includes("tempo") ? snapshot.tempo : undefined;
  // PRESET BPM belongs to this preset; switching GLOBAL/PRESET changes the target.
  const bpmContext = `${snapshot.stateVersion?.session ?? ""}:${globalMode ? "GLOBAL" : mode === "PRESET" ? `PRESET:${snapshot.presetIdentity ?? ""}:${snapshot.setlistKey}:${snapshot.presetPosition}` : "UNKNOWN"}`;
  const latestBpmContext = useRef(bpmContext);
  const bpmEpoch = useRef(0);
  if (latestBpmContext.current !== bpmContext) bpmEpoch.current += 1;
  latestBpmContext.current = bpmContext;
  const admittedBpmEpoch = bpmEpoch.current;
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);

  // Transient drag state for the three continuous dials: overrides the
  // committed/live value only while the user is actively dragging/nudging,
  // the same pattern the block editor's callers use their own drafts for.
  const [draftBpm, setDraftBpm] = useState<{ context: string; epoch: number; value: number }>();
  const [draftVolume, setDraftVolume] = useState<number>();
  const [draftPan, setDraftPan] = useState<number>();
  const displayBpm = committedBpm === undefined ? undefined : draftBpm?.context === bpmContext && draftBpm.epoch === admittedBpmEpoch ? draftBpm.value : committedBpm;
  const displayVolume = volumeDb === undefined ? undefined : draftVolume ?? volumeDb;
  const displayPan = pan === undefined ? undefined : draftPan ?? pan;

  // currentTempoClock()'s currentBeat indexing (0- vs 1-based) is not
  // confirmed anywhere in this codebase; this is a best-effort, non-live
  // (fetched once on open, not polled) reflection of the fixture's own
  // static single-dot default, not a claim about the real indexing.
  const activeBeat = tempoClock?.available && typeof tempoClock.currentBeat === "number"
    ? ((Math.trunc(tempoClock.currentBeat) % 4) + 4) % 4
    : undefined;

  const disabled = pending || loading;

  const commitBpm = (bpm: number) => {
    if (!ownerCurrent() || !mounted.current || committedBpm === undefined || latestBpmContext.current !== bpmContext || bpmEpoch.current !== admittedBpmEpoch) return;
    setDraftBpm(undefined);
    if (globalMode) void workflow.setGlobalBpm(bpm);
    else queueTempo(bpm, "Encoder");
  };
  const commitVolume = (value: number) => { if (!ownerCurrent() || volumeDb === undefined) return; setDraftVolume(undefined); void workflow.setVolumeDb(value); };
  const commitPan = (value: number) => { if (!ownerCurrent() || pan === undefined) return; setDraftPan(undefined); void workflow.setPan(value); };

  // A device screen, not a window-level dialog: the host passes it as the
  // surface's screenContent so it fills the bezel's own 800x480 canvas.
  return <>
      <section className="coros-tempo" aria-label="Tempo and Metronome" data-qc-open-target="tempo" tabIndex={-1} onKeyDown={(event) => escapeUserOpenedTarget(event, onClose, workflow.returnFocusTarget)}>
        <header>
          <span>Tempo</span>
          <QcSceneSelector className="tempo-scene" letter={sceneLetter(snapshot.activeScene)} onPrevious={() => onSelectScene((snapshot.activeScene - 1 + snapshot.scenes.length) % snapshot.scenes.length)} onNext={() => onSelectScene((snapshot.activeScene + 1) % snapshot.scenes.length)} />
          <button type="button" aria-label="Close Tempo and Metronome" onClick={(event) => closeUserOpenedTarget(event.currentTarget, onClose, workflow.returnFocusTarget)}><QcUiIcon kind="check" /></button>
        </header>
        <div className="tempo-display">
          <strong>{displayBpm === undefined ? "—" : Math.round(displayBpm)}</strong>
          <small>BPM = QUARTERS</small>
          <div>
            {[0, 1, 2, 3].map((beat) => <i key={beat} className={beat === activeBeat ? "is-active" : ""} />)}
          </div>
        </div>
        <div className="tempo-controls">
          <section className="tempo-control is-tempo">
            <span>TEMPO</span>
            <TempoDial key={bpmContext} label="Tempo" valueText={`${displayBpm === undefined ? "—" : Math.round(displayBpm)} BPM`} value={displayBpm} min={BPM_MIN} max={BPM_MAX} step={1} disabled={disabled} onDraftChange={(value) => { if (ownerCurrent() && mounted.current && latestBpmContext.current === bpmContext && bpmEpoch.current === admittedBpmEpoch) setDraftBpm({ context: bpmContext, epoch: admittedBpmEpoch, value }); }} onCommit={commitBpm} />
            <strong>{displayBpm === undefined ? "—" : Math.round(displayBpm)}<span className="tempo-unit"> BPM</span></strong>
          </section>
          <button type="button" className={`tempo-control is-radio${disabled ? " is-pending" : ""}`} disabled={disabled || mode === undefined} aria-pressed={mode === undefined ? undefined : globalMode} aria-label={`Tempo Mode: ${mode === undefined ? "Unavailable" : globalMode ? "Global" : "Preset"}`} onClick={() => { if (!disabled && mode !== undefined) void workflow.setMode(globalMode ? "PRESET" : "GLOBAL"); }}>
            <span>MODE <b className="tempo-info" title="GLOBAL MODE shares Tempo and Metronome settings across every preset. PRESET MODE stores them in each preset.">i</b></span>
            {mode !== undefined && <QcPillToggle index={globalMode ? 0 : 1} />}
            <span className={globalMode ? "is-active" : undefined}>Global</span>
            <span className={mode === "PRESET" ? "is-active" : undefined}>Preset</span>
          </button>
          <button type="button" className={`tempo-control is-radio${disabled ? " is-pending" : ""}`} disabled={disabled || ledEnabled === undefined} aria-pressed={ledEnabled} aria-label={`Tempo LED: ${ledEnabled === undefined ? "Unavailable" : ledEnabled ? "On" : "Off"}`} onClick={() => { if (!disabled && ledEnabled !== undefined) void workflow.setLedEnabled(!ledEnabled); }}>
            <span>TEMPO LED</span>
            {ledEnabled !== undefined && <QcPillToggle index={ledEnabled ? 0 : 1} />}
            <span className={ledEnabled ? "is-active" : undefined}>On</span>
            <span className={ledEnabled === false ? "is-active" : undefined}>Off</span>
          </button>
          <section className="tempo-control is-volume">
            <span>VOLUME</span>
            <TempoDial label="Volume" valueText={`${displayVolume === undefined ? "—" : formatFixed(displayVolume)} dB`} value={displayVolume} min={VOLUME_MIN} max={VOLUME_MAX} step={.5} position={tempoVolumePosition} disabled={disabled} onDraftChange={(value) => { if (ownerCurrent()) setDraftVolume(value); }} onCommit={commitVolume} />
            <strong>{displayVolume === undefined ? "—" : formatFixed(displayVolume)}<span className="tempo-unit"> dB</span></strong>
          </section>
          <section className="tempo-control is-mute">
            <button type="button" disabled={disabled || running === undefined} onClick={() => { if (!disabled && running !== undefined) void workflow.setRunning(!running); }}>{running === undefined ? "Unavailable" : running ? "MUTE" : "UNMUTE"}</button>
          </section>
          <section className="tempo-control is-pan">
            <span>PAN</span>
            <TempoDial label="Pan" valueText={displayPan === undefined ? "—" : panLabel(displayPan)} value={displayPan} min={PAN_MIN} max={PAN_MAX} step={.02} origin={PAN_CENTER} disabled={disabled} onDraftChange={(value) => { if (ownerCurrent()) setDraftPan(value); }} onCommit={commitPan} />
            <strong>{displayPan === undefined ? "—" : panLabel(displayPan)}</strong>
          </section>
          <section className="tempo-control is-select">
            <span>T/SIGNATURE</span>
            <div className="tempo-select"><select value={timeSignature ?? ""} disabled={disabled} onChange={(event) => void workflow.setTimeSignature(event.target.value as NonNullable<TempoSettings["timeSignature"]>)}>
              {timeSignature === undefined && <option value="" disabled>Unavailable</option>}
              {TEMPO_TIME_SIGNATURES.map((value) => <option key={value} value={value}>{value}</option>)}
            </select></div>
          </section>
          <section className="tempo-control is-select">
            <span>SUBDIVISIONS</span>
            <div className="tempo-select"><select value={subdivision ?? ""} disabled={disabled || !settings?.timeSignature} onChange={(event) => void workflow.setSubdivision(event.target.value as NonNullable<TempoSettings["subdivision"]>)}>
              {subdivision === undefined && <option value="" disabled>Unavailable</option>}
              {tempoSubdivisionOptions(settings?.timeSignature).map((value) => <option key={value} value={value}>{value}</option>)}
            </select></div>
          </section>
          <section className="tempo-control is-select">
            <span>SOUND</span>
            <div className="tempo-select"><select value={sound ?? ""} disabled={disabled} onChange={(event) => void workflow.setSound(event.target.value as NonNullable<TempoSettings["sound"]>)}>
              {sound === undefined && <option value="" disabled>Unavailable</option>}
              {TEMPO_SOUNDS.map((value) => <option key={value} value={value}>{value}</option>)}
            </select></div>
          </section>
          <section className="tempo-control is-select">
            <span>ROUTING</span>
            <div className="tempo-select"><select value={routing ?? ""} disabled={disabled} onChange={(event) => void workflow.setRouting(event.target.value as NonNullable<TempoSettings["routing"]>)}>
              {routing === undefined && <option value="" disabled>Unavailable</option>}
              {TEMPO_ROUTINGS.map((value) => <option key={value} value={value}>{value}</option>)}
            </select></div>
          </section>
        </div>
      </section>
  </>;
}
