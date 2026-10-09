import type { CSSProperties } from "react";
import "./qc-bar-slider.css";

/**
 * The CorOS horizontal bar slider - the control a cab editor draws where every
 * other editor draws a rotary.
 *
 * It appears eight times per cab: POSITION, DISTANCE, LEVEL and PAN in each of
 * the two CabsimPanels. All four are declared `float` by the model catalogue,
 * so `float` alone does not pick this construction; the cab editor does.
 *
 * The pane ground is the screen's control surface and the track is the
 * screen's background ink, both neutral, while the two panel accents - the
 * browser's cab and pitch category colours - are not, so the fill separates
 * from both with one channel difference.
 *
 *   track          81.0 x 5.0, pitch 97.0, four to a panel
 *   track ink      black; the pane around it is the screen's control surface
 *   fill height    the track's own height
 *   zero mark      1.50 x 4.00, one pixel shorter than a fill
 *
 * THE FILL RUNS FROM AN ORIGIN, NOT ALWAYS FROM THE LEFT. POSITION, DISTANCE
 * and LEVEL fill rightward from the track's left edge. PAN does not: its mark
 * sits at the track's centre and its fill runs either way from there. The
 * only thing that distinguishes PAN in the model catalogue is its
 * `midpointLabel: 'C'` beside `minimumLabel: 'L'` and `maximumLabel: 'R'`;
 * LEVEL declares a minimumLabel and no midpoint, and the other two declare
 * none. So `origin` is a parameter here and the call site decides it.
 *
 * A ZERO-LENGTH FILL STILL PAINTS. DISTANCE reads 0.0 and PAN reads C, and
 * both draw a 1.50 x 4.00 mark rather than nothing. The mark is a pixel
 * shorter than a fill, which is not a rounding artefact.
 *
 * The fill's mapping to a value is proportional, and nothing draws a fill
 * past about half the track, so the upper half of the sweep is an
 * extrapolation from the values that are drawn.
 *
 * Separately, and not this component's business: the unit's cab LEVEL curve
 * does not match the linear or power-scaled mapping the model catalogue
 * declares for it, which is a question for parameterNormalizedValue rather
 * than for the drawing.
 *
 * Nothing here sets shapeRendering. The unit antialiases these edges - the
 * track's ends carry a partial column and the fill's a partial pixel either
 * side - and crispEdges would snap both onto whole pixels.
 *
 * Colour is a role. The track is the screen's background ink and the fill is
 * the panel's accent, which the two CabsimPanels take from two different block
 * categories rather than from one screen accent, so it is passed in.
 */

/** Track length in device pixels on the unit's 800px screen. */
export const QC_BAR_TRACK_WIDTH = 81;
/** Track height, and a fill's height. */
export const QC_BAR_TRACK_HEIGHT = 5;
/** The mark a zero-length fill still paints: narrower and a pixel shorter. */
const MARK_WIDTH = 1.5;
const MARK_HEIGHT = 4;
/**
 * The unit's track edges lie on half pixels, and a browser snaps an element
 * box to the pixel grid before it paints, so an element placed at 19.5 rounds
 * to 20 and loses every edge ramp the device draws. The viewBox therefore
 * carries half a unit of bleed on each side and the geometry sits at 0.5: the
 * element is placed at a whole pixel and the antialiasing happens inside the
 * SVG, where nothing snaps it.
 */
const BLEED = 0.5;
/**
 * One pixel of each corner is cut: on the track's two visible ends, the row
 * above and below the fill carry a partial column that the middle rows do
 * not, which is a corner and not an edge. Only those two ends are visible;
 * the other six are under a fill or a mark.
 */
const TRACK_RADIUS = 1;

const clamp = (value: number) => Math.max(0, Math.min(1, value));

export type QcBarSliderProps = {
  className?: string;
  style?: CSSProperties;
  /** Position along the track, 0..1. */
  value: number;
  /**
   * Where the fill starts, 0..1. 0 on POSITION, DISTANCE and LEVEL; 0.5 on PAN,
   * which is the one control here the ModelRepo gives a midpointLabel.
   */
  origin?: number;
  /** Accessible name, where the bar is the control rather than decoration. */
  label?: string;
};

export function QcBarSlider({ className, style, value, origin = 0, label }: QcBarSliderProps) {
  const to = clamp(value) * QC_BAR_TRACK_WIDTH;
  const from = clamp(origin) * QC_BAR_TRACK_WIDTH;
  const length = Math.abs(to - from);
  const forward = to >= from;
  const classes = `qc-bar-slider${className ? ` ${className}` : ""}`;
  return (
    <svg
      className={classes}
      style={style}
      viewBox={`0 0 ${QC_BAR_TRACK_WIDTH + 2 * BLEED} ${QC_BAR_TRACK_HEIGHT + 2 * BLEED}`}
      preserveAspectRatio="none"
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <rect
        className="qc-bar-track"
        x={BLEED}
        y={BLEED}
        width={QC_BAR_TRACK_WIDTH}
        height={QC_BAR_TRACK_HEIGHT}
        rx={TRACK_RADIUS}
      />
      {length >= MARK_WIDTH ? (
        <rect
          className="qc-bar-fill"
          x={BLEED + Math.min(from, to)}
          y={BLEED}
          width={length}
          height={QC_BAR_TRACK_HEIGHT}
        />
      ) : (
        <rect
          className="qc-bar-fill"
          x={BLEED + (forward ? from : from - MARK_WIDTH)}
          y={BLEED + (QC_BAR_TRACK_HEIGHT - MARK_HEIGHT) / 2}
          width={MARK_WIDTH}
          height={MARK_HEIGHT}
        />
      )}
    </svg>
  );
}
