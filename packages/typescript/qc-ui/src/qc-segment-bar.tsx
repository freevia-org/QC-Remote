import "./qc-segment-bar.css";

/**
 * The CorOS segmented value bar.
 *
 * It is a row of equal lit and unlit segments, not a slider with a thumb.
 *
 * ONE CONTAINER EXPLAINS ALL THREE WIDTHS. The painted widths differ across
 * segment counts, but they are not three layouts: every one of them is
 * floor((W - 2(n-1)) / n) per segment out of the same 486-wide box, with the
 * remainder left unpainted at the right end - 486 is the value at which a
 * four-segment bar paints flush to the box.
 *
 * THE UNLIT SEGMENT AND THE GAP ARE THE SAME INK. Both read the screen's base
 * surface, so a 32-segment bar with 16 lit is pixel-identical to a 16-segment
 * lit run on a plain dark track. This draws the track once and the lit
 * segments over it, which is the same result with 31 fewer nodes.
 *
 * The bar height is per-pane: 24 on most panes, 22 on the power pane. This
 * takes it as a parameter rather than deriving one value from the other.
 *
 * Nothing here sets shapeRendering: every edge in this control lands on a whole
 * pixel at the unit's own size, so there is nothing for either setting to do,
 * and crispEdges would only take the fallback away at other sizes.
 *
 * Colour is a role. Lit is the screen accent and the track is the screen's
 * base surface; neither is a literal here.
 */

/** Gap between segments, in device pixels: 2. */
export const QC_SEGMENT_GAP = 2;
/**
 * The box the segments are laid out in: 486, the width at which a
 * four-segment bar paints flush to the box.
 */
export const QC_SEGMENT_BOX = 486;
/** Bar height. 24 on most panes; the power pane draws 22. */
export const QC_SEGMENT_HEIGHT = 24;

/** Segment width the unit paints for a given count, out of the shared box. */
export function qcSegmentWidth(segments: number, box = QC_SEGMENT_BOX) {
  return Math.floor((box - QC_SEGMENT_GAP * (segments - 1)) / Math.max(1, segments));
}

export type QcSegmentBarProps = {
  className?: string;
  /** How many segments the bar is divided into. The unit draws 4, 6 and 32. */
  segments: number;
  /** How many of them are lit, counted from the left. */
  filled: number;
  /** The layout box; only change it to match a different container width. */
  box?: number;
  /** Bar height in device pixels: 24 everywhere but the power pane's 22. */
  height?: number;
  /** Accessible name, where the bar is the control rather than decoration. */
  label?: string;
};

export function QcSegmentBar({ className, segments, filled, box = QC_SEGMENT_BOX, height = QC_SEGMENT_HEIGHT, label }: QcSegmentBarProps) {
  const count = Math.max(1, Math.round(segments));
  const width = qcSegmentWidth(count, box);
  const pitch = width + QC_SEGMENT_GAP;
  const painted = count * width + QC_SEGMENT_GAP * (count - 1);
  const lit = Math.max(0, Math.min(count, Math.round(filled)));
  return (
    <svg
      className={`qc-segment-bar${className ? ` ${className}` : ""}`}
      data-height={height}
      viewBox={`0 0 ${box} ${height}`}
      preserveAspectRatio="none"
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <rect className="qc-segment-track" x="0" y="0" width={painted} height={height} />
      {Array.from({ length: lit }, (_, index) => (
        <rect key={index} className="qc-segment-lit" x={index * pitch} y="0" width={width} height={height} />
      ))}
    </svg>
  );
}
