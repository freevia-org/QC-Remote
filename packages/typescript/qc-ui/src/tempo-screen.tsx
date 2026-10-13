import { closeUserOpenedTarget, escapeUserOpenedTarget } from "./user-open-focus";
import { useState } from "react";
import type { PresetSnapshot, TempoSettings } from "@qc-remote/client";
import { sceneLetter } from "@qc-remote/core";
import { QcPillToggle } from "./qc-pill-toggle";
import { QcRotaryControl } from "./qc-rotary-control";
import { QcRotaryDial } from "./qc-rotary-dial";
import { QcSceneSelector } from "./qc-shared-controls";
import { QcUiIcon } from "./theme-icons";
import { useRotaryValue } from "./use-rotary-gesture";
import { TEMPO_ROUTINGS, TEMPO_SOUNDS, TEMPO_SUBDIVISIONS, TEMPO_TIME_SIGNATURES, type TempoWorkflow } from "./use-tempo-workflow";
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
function TempoDial({ className, label, valueText, value, min, max, step, origin, disabled, accent, onDraftChange, onCommit }: {
  className?: string;
  /** The control's name: TEMPO, VOLUME or PAN. */
  label: string;
  /** The value as the screen prints it beside the dial. */
  valueText: string;
  value: number;
  min: number;
  max: number;
  step: number;
  /** Where the fill starts, in the dial's own units. Omit for the unit's usual fill from the minimum. */
  origin?: number;
  disabled?: boolean;
  accent?: string;
  onDraftChange(value: number): void;
  onCommit(value: number): void;
}) {
  const clamp = (next: number) => Math.max(min, Math.min(max, next));
  const gesture = useRotaryValue({
    value, step, disabled, onDraftChange, onCommit,
    quantize: (next) => clamp(Math.round(next / step) * step),
    pixelsPerStep: TEMPO_DIAL_PIXELS_PER_STEP,
    wheelSettleMs: TEMPO_DIAL_WHEEL_SETTLE_MS
  });
  return <QcRotaryControl className={`tempo-dial${className ? ` ${className}` : ""}`} disabled={disabled} label={label} value={value} min={min} max={max} valueText={valueText} gesture={gesture}>
    <QcRotaryDial value={(clamp(value) - min) / (max - min)} origin={origin === undefined ? undefined : (clamp(origin) - min) / (max - min)} accent={accent} />
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
  const mode: NonNullable<TempoSettings["mode"]> = settings?.mode ?? "PRESET";
  const globalMode = mode === "GLOBAL";
  const ledEnabled = settings?.ledEnabled ?? true;
  const running = settings?.running ?? false;
  const volumeDb = settings?.volumeDb ?? 0;
  const pan = settings?.pan ?? PAN_CENTER;
  const timeSignature = settings?.timeSignature ?? "4/4";
  const subdivision = settings?.subdivision ?? "1/4";
  const sound = settings?.sound ?? "BLIP";
  const routing = settings?.routing ?? "MULTI";
  const committedBpm = globalMode ? settings?.globalBpm ?? snapshot.tempo : snapshot.tempo;

  // Transient drag state for the three continuous dials: overrides the
  // committed/live value only while the user is actively dragging/nudging,
  // the same pattern the block editor's callers use their own drafts for.
  const [draftBpm, setDraftBpm] = useState<number>();
  const [draftVolume, setDraftVolume] = useState<number>();
  const [draftPan, setDraftPan] = useState<number>();
  const displayBpm = draftBpm ?? committedBpm;
  const displayVolume = draftVolume ?? volumeDb;
  const displayPan = draftPan ?? pan;

  // currentTempoClock()'s currentBeat indexing (0- vs 1-based) is not
  // confirmed anywhere in this codebase; this is a best-effort, non-live
  // (fetched once on open, not polled) reflection of the fixture's own
  // static single-dot default, not a claim about the real indexing.
  const activeBeat = tempoClock?.available && typeof tempoClock.currentBeat === "number"
    ? ((Math.trunc(tempoClock.currentBeat) % 4) + 4) % 4
    : 0;

  const disabled = pending || loading;

  const commitBpm = (bpm: number) => {
    if (!ownerCurrent()) return;
    setDraftBpm(undefined);
    if (globalMode) void workflow.setGlobalBpm(bpm);
    else queueTempo(bpm, "Encoder");
  };
  const commitVolume = (value: number) => { if (!ownerCurrent()) return; setDraftVolume(undefined); void workflow.setVolumeDb(value); };
  const commitPan = (value: number) => { if (!ownerCurrent()) return; setDraftPan(undefined); void workflow.setPan(value); };

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
          <strong>{Math.round(displayBpm)}</strong>
          <small>BPM = QUARTERS</small>
          <div>
            {[0, 1, 2, 3].map((beat) => <i key={beat} className={beat === activeBeat ? "is-active" : ""} />)}
          </div>
        </div>
        <div className="tempo-controls">
          <section className="tempo-control is-tempo">
            <span>TEMPO</span>
            <TempoDial label="Tempo" valueText={`${Math.round(displayBpm)} BPM`} value={displayBpm} min={BPM_MIN} max={BPM_MAX} step={1} disabled={disabled} onDraftChange={(value) => { if (ownerCurrent()) setDraftBpm(value); }} onCommit={commitBpm} />
            <strong>{Math.round(displayBpm)}<span className="tempo-unit"> BPM</span></strong>
          </section>
          <button type="button" className={`tempo-control is-radio${disabled ? " is-pending" : ""}`} disabled={disabled} aria-pressed={globalMode} aria-label={`Tempo Mode: ${globalMode ? "Global" : "Preset"}`} onClick={() => void workflow.setMode(globalMode ? "PRESET" : "GLOBAL")}>
            <span>MODE <b className="tempo-info" title="GLOBAL MODE shares Tempo and Metronome settings across every preset. PRESET MODE stores them in each preset.">i</b></span>
            <QcPillToggle index={globalMode ? 0 : 1} />
            <span className={globalMode ? "is-active" : undefined}>Global</span>
            <span className={globalMode ? undefined : "is-active"}>Preset</span>
          </button>
          <button type="button" className={`tempo-control is-radio${disabled ? " is-pending" : ""}`} disabled={disabled} aria-pressed={ledEnabled} aria-label={`Tempo LED: ${ledEnabled ? "On" : "Off"}`} onClick={() => void workflow.setLedEnabled(!ledEnabled)}>
            <span>TEMPO LED</span>
            <QcPillToggle index={ledEnabled ? 0 : 1} />
            <span className={ledEnabled ? "is-active" : undefined}>On</span>
            <span className={ledEnabled ? undefined : "is-active"}>Off</span>
          </button>
          <section className="tempo-control is-volume">
            <span>VOLUME</span>
            <TempoDial label="Volume" valueText={`${displayVolume.toFixed(1)} dB`} value={displayVolume} min={VOLUME_MIN} max={VOLUME_MAX} step={.5} disabled={disabled} onDraftChange={(value) => { if (ownerCurrent()) setDraftVolume(value); }} onCommit={commitVolume} />
            <strong>{displayVolume.toFixed(1)}<span className="tempo-unit"> dB</span></strong>
          </section>
          <section className="tempo-control is-mute">
            <button type="button" disabled={disabled} onClick={() => void workflow.setRunning(!running)}>{running ? "MUTE" : "UNMUTE"}</button>
          </section>
          <section className="tempo-control is-pan">
            <span>PAN</span>
            <TempoDial label="Pan" valueText={panLabel(displayPan)} value={displayPan} min={PAN_MIN} max={PAN_MAX} step={.02} origin={PAN_CENTER} disabled={disabled} onDraftChange={(value) => { if (ownerCurrent()) setDraftPan(value); }} onCommit={commitPan} />
            <strong>{panLabel(displayPan)}</strong>
          </section>
          <section className="tempo-control is-select">
            <span>T/SIGNATURE</span>
            <div className="tempo-select"><select value={timeSignature} disabled={disabled} onChange={(event) => void workflow.setTimeSignature(event.target.value as NonNullable<TempoSettings["timeSignature"]>)}>
              {TEMPO_TIME_SIGNATURES.map((value) => <option key={value} value={value}>{value}</option>)}
            </select></div>
          </section>
          <section className="tempo-control is-select">
            <span>SUBDIVISIONS</span>
            <div className="tempo-select"><select value={subdivision} disabled={disabled} onChange={(event) => void workflow.setSubdivision(event.target.value as NonNullable<TempoSettings["subdivision"]>)}>
              {TEMPO_SUBDIVISIONS.map((value) => <option key={value} value={value}>{value}</option>)}
            </select></div>
          </section>
          <section className="tempo-control is-select">
            <span>SOUND</span>
            <div className="tempo-select"><select value={sound} disabled={disabled} onChange={(event) => void workflow.setSound(event.target.value as NonNullable<TempoSettings["sound"]>)}>
              {TEMPO_SOUNDS.map((value) => <option key={value} value={value}>{value}</option>)}
            </select></div>
          </section>
          <section className="tempo-control is-select">
            <span>ROUTING</span>
            <div className="tempo-select"><select value={routing} disabled={disabled} onChange={(event) => void workflow.setRouting(event.target.value as NonNullable<TempoSettings["routing"]>)}>
              {TEMPO_ROUTINGS.map((value) => <option key={value} value={value}>{value}</option>)}
            </select></div>
          </section>
        </div>
      </section>
  </>;
}
