import type { CSSProperties } from "react";
import { QC_COLORS } from "@qc-remote/theme";
import "./qc-rotary-dial.css";

/**
 * The CorOS rotary control - one construction for every screen that draws one.
 *
 * The unit draws the control at a single size on every screen: the knob body
 * is a 49px circle, so one unit of this 72-unit viewBox is one device pixel
 * and 72px is the natural render size.
 *
 *   face fill      r <= 21.2
 *   body rim       r 21.2 .. 24.5      near-black, 3.3 wide, centre r 22.9
 *   value scale    r 29.0 .. 32.0      3.0 wide, centre r 30.5
 *   value fill     r 30.6 .. 34.5      3.9 wide, centre r 32.55
 *   pointer        r 8.5 .. 26.5       3.0 wide, and exactly radial
 *
 * The fill sits outboard of the scale rather than on it, overlapping only the
 * scale's outer half and reaching 2.5px further out. Reading the two as one
 * ring makes the scale look smaller than it is.
 *
 * The sweep is 270 degrees, from 135 to 405 measured clockwise from +x,
 * leaving a 90 degree gap centred on the bottom.
 *
 * Caps are butt, not round. The fill's low end tapers 5, 3, 1 pixels wide,
 * which is a square cut across a 3.9-wide stroke lying at 135 degrees; a round
 * cap would have carried the colour ~4 degrees past the start instead.
 *
 * A discrete parameter drops both arcs and marks every step with a dot of
 * radius 3.0 riding on the fill's own radius - unlit, except the one the
 * pointer is on. The dots are spread evenly across the same 270 degree sweep,
 * and the unlit ones are darker than the panel, not a dimmed accent.
 *
 * Nothing here sets shapeRendering. The unit antialiases its curves - the fill
 * has a full blend ramp either side of its edges - and crispEdges would
 * stair-step every one of them.
 */

/** Degrees of rotation between the unit's minimum and maximum. */
export const QC_ROTARY_SWEEP = 270;
/** Screen angle of the minimum, clockwise from +x. */
export const QC_ROTARY_START = 135;

const CENTRE = 36;
const FACE_RADIUS = 21.2;
const RIM_RADIUS = 24.5;
const SCALE_RADIUS = 30.5;
const FILL_RADIUS = 32.55;
const POINTER_INNER = 8.5;
const POINTER_OUTER = 26.5;
/* Step dots ride on the fill's own radius. */
const STEP_RADIUS = FILL_RADIUS;
const STEP_DOT = 3;
/** The swept fraction of a whole turn, for stroke-dasharray against pathLength 100. */
const SWEEP_PERCENT = (QC_ROTARY_SWEEP / 360) * 100;

const clamp = (value: number) => Math.max(0, Math.min(1, value));

/** Screen angle, clockwise from +x, for a 0..1 position along the unit's sweep. */
export function qcRotaryAngle(value: number) {
  return QC_ROTARY_START + clamp(value) * QC_ROTARY_SWEEP;
}

/** The inverse: where a pointer angle sits on the unit's sweep. */
export function qcRotaryValue(angle: number) {
  const turned = (((angle - QC_ROTARY_START) % 360) + 360) % 360;
  return clamp(turned / QC_ROTARY_SWEEP);
}

export type QcRotaryDialProps = {
  className?: string;
  /** Position along the unit's 270 degree sweep, 0..1. Drives pointer and fill together. */
  value?: number;
  /** The same state spelled as a pointer angle in degrees clockwise from +x. */
  angle?: number;
  /** "arc" fills the scale to the value; "step" is the unit's discrete detent dots. */
  mode?: "arc" | "step";
  /**
   * Where the fill starts, 0..1. The unit fills from the minimum on nearly
   * every control, but not on two: the tuner's reference frequency is drawn
   * from the 270 degree centre back to the reading rather than up from 135,
   * and so is the Tempo screen's PAN, which at centre (C) shows no arc at all
   * (corpus tempo-metronome.png, and the unit walked on 2026-10-04).
   */
  origin?: number;
  /** How many detents "step" mode marks. Two or more, evenly across the sweep. */
  steps?: number;
  /** Rendered edge length in px. Omit where a stylesheet already sizes the dial. */
  size?: number;
  accent?: string;
  track?: string;
  face?: string;
  rim?: string;
  step?: string;
  pointer?: string;
};

export function QcRotaryDial({
  className,
  value,
  angle,
  mode = "arc",
  origin = 0,
  steps = 2,
  size,
  accent = QC_COLORS.reference.rotaryAccent,
  track = QC_COLORS.reference.rotaryTrack,
  face = QC_COLORS.reference.rotaryFace,
  rim = QC_COLORS.reference.rotarySeparator,
  step = QC_COLORS.reference.rotaryStep,
  pointer = QC_COLORS.reference.iconPrimary
}: QcRotaryDialProps) {
  const position = value !== undefined ? clamp(value) : angle !== undefined ? qcRotaryValue(angle) : 0;
  const from = clamp(origin);
  const style = {
    "--qc-rotary-progress": Math.abs(position - from) * SWEEP_PERCENT,
    "--qc-rotary-fill-start": -Math.min(position, from) * SWEEP_PERCENT,
    "--qc-rotary-angle": `${qcRotaryAngle(position)}deg`,
    "--qc-rotary-accent": accent,
    "--qc-rotary-track": track,
    "--qc-rotary-face": face,
    "--qc-rotary-rim": rim,
    "--qc-rotary-step": step,
    "--qc-rotary-pointer": pointer,
    ...(size === undefined ? {} : { width: `${size}px`, height: `${size}px` })
  } as CSSProperties;
  const detents = Math.max(2, Math.round(steps));
  const current = Math.round(position * (detents - 1));

  return <svg className={`qc-rotary${className ? ` ${className}` : ""}`} viewBox="0 0 72 72" style={style} aria-hidden="true">
    <circle className="qc-rotary-rim" cx={CENTRE} cy={CENTRE} r={RIM_RADIUS} />
    <circle className="qc-rotary-face" cx={CENTRE} cy={CENTRE} r={FACE_RADIUS} />
    {mode === "arc" && <circle className="qc-rotary-track" cx={CENTRE} cy={CENTRE} r={SCALE_RADIUS} pathLength="100" />}
    {mode === "arc" && <circle className="qc-rotary-progress" cx={CENTRE} cy={CENTRE} r={FILL_RADIUS} pathLength="100" />}
    {mode === "step" && Array.from({ length: detents }, (_, index) => {
      const radians = (qcRotaryAngle(index / (detents - 1)) * Math.PI) / 180;
      return <circle
        key={index}
        className={`qc-rotary-step${index === current ? " is-current" : ""}`}
        cx={CENTRE + STEP_RADIUS * Math.cos(radians)}
        cy={CENTRE + STEP_RADIUS * Math.sin(radians)}
        r={STEP_DOT}
      />;
    })}
    <line className="qc-rotary-pointer" x1={CENTRE + POINTER_INNER} y1={CENTRE} x2={CENTRE + POINTER_OUTER} y2={CENTRE} />
  </svg>;
}
