import type { CSSProperties } from "react";
import "./qc-pill-toggle.css";

/**
 * The CorOS pill toggle - one construction for every screen that draws one.
 *
 * The unit draws it at exactly two lengths. The body is not a stadium: a
 * stadium on a 22-wide pill needs corner radius 11, but this pill's corners
 * are shallower, at 9.4, leaving about 3.1px of straight edge across each
 * end.
 *
 * Only the current detent is drawn. There are no unlit marks - a two-way
 * pill carries exactly one ring and a three-way pill carries exactly one
 * ring.
 *
 * The three-way toggle's middle detent is placed arithmetically, evenly
 * spaced between the two end positions (11.0 inset from each end of a
 * 58-long body, an 18.0 pitch) rather than pinned to its own independent
 * value.
 *
 * The two lengths are independent: a two-way pill spaces its detents 24
 * apart and a three-way pill 18, so neither pitch can be derived from the
 * other. Any other detent count falls back to the three-way pitch.
 *
 * Nothing here sets shapeRendering. The unit antialiases its curves - the
 * corner and the ring both carry full coverage ramps - and crispEdges would
 * stair-step every one of them.
 *
 * Colour is a role, not a literal. The body is the screen's base surface and
 * the detent is the screen accent, except where a screen names its own
 * accent (a block editor's category colour, the splitter's blue) or the
 * unit has substituted a dimmed surface for a disabled control. Those are
 * passed in; they are not alpha blends of these.
 */

/** Body width, in device pixels. One viewBox unit equals one device pixel. */
export const QC_PILL_WIDTH = 22;
/** Corner radius: 9.4, not 11 - the body is not a stadium. */
export const QC_PILL_CORNER = 9.4;
/** Detent centre, from the near end: 11. */
export const QC_PILL_DETENT_INSET = 11;
/** Detent ring centreline radius: 7.49. */
export const QC_PILL_DETENT_RADIUS = 7.49;
/** Detent ring stroke width: 2.83. */
export const QC_PILL_DETENT_STROKE = 2.83;

/**
 * Body length per detent count. The unit draws only these two, and their
 * detent pitches differ - 24 for the two-way, 18 for the three-way - so
 * neither can be derived from the other.
 */
export const QC_PILL_HEIGHTS: Record<number, number> = { 2: 46, 3: 58 };
/** The three-way pitch, used as the fallback for any other detent count. */
const FALLBACK_PITCH = 18;

/** Body length for a detent count. Anything but two or three falls back to the three-way pitch. */
export function qcPillHeight(positions: number) {
  const detents = Math.max(2, Math.round(positions));
  return QC_PILL_HEIGHTS[detents] ?? 2 * QC_PILL_DETENT_INSET + (detents - 1) * FALLBACK_PITCH;
}

/** Where detent `index` sits along the body, in device pixels from the top. */
export function qcPillDetentCentre(positions: number, index: number) {
  const detents = Math.max(2, Math.round(positions));
  const span = qcPillHeight(detents) - 2 * QC_PILL_DETENT_INSET;
  const step = Math.min(Math.max(Math.round(index), 0), detents - 1);
  return QC_PILL_DETENT_INSET + (span * step) / (detents - 1);
}

export type QcPillToggleProps = {
  className?: string;
  /**
   * How many detents the control has. The unit draws two and three; any
   * other count falls back to the three-way pitch - see qcPillHeight.
   */
  positions?: number;
  /**
   * Which detent is lit, 0 at the top. This is the whole point of the
   * component: the position is a parameter, not a hardcoded margin, so a
   * screen that puts its detent somewhere else says so here.
   */
  index?: number;
  /** The same state as a 0..1 position, for a call site that already has one. */
  value?: number;
  /** Body colour. Defaults to the screen's base surface through the stylesheet. */
  body?: string;
  /** Detent colour. Defaults to the screen accent through the stylesheet. */
  detent?: string;
  /** Rendered width in px. Omit to size from the screen, as the unit does. */
  width?: number | string;
  /** Rendered height in px. Omit to size from the screen. */
  height?: number | string;
};

export function QcPillToggle({
  className,
  positions = 2,
  index,
  value,
  body,
  detent,
  width,
  height
}: QcPillToggleProps) {
  const detents = Math.max(2, Math.round(positions));
  const length = qcPillHeight(detents);
  const step = index !== undefined
    ? index
    : value !== undefined
      ? Math.round(Math.min(Math.max(value, 0), 1) * (detents - 1))
      : 0;
  const style = {
    ...(body === undefined ? {} : { "--qc-pill-body": body }),
    ...(detent === undefined ? {} : { "--qc-pill-detent": detent }),
    ...(width === undefined ? {} : { width: typeof width === "number" ? `${width}px` : width }),
    ...(height === undefined ? {} : { height: typeof height === "number" ? `${height}px` : height })
  } as CSSProperties;

  return <svg
    className={`qc-pill-toggle${className ? ` ${className}` : ""}`}
    viewBox={`0 0 ${QC_PILL_WIDTH} ${length}`}
    data-positions={detents}
    style={style}
    aria-hidden="true"
  >
    <rect
      className="qc-pill-body"
      x="0"
      y="0"
      width={QC_PILL_WIDTH}
      height={length}
      rx={QC_PILL_CORNER}
    />
    <circle
      className="qc-pill-detent"
      cx={QC_PILL_WIDTH / 2}
      cy={qcPillDetentCentre(detents, step)}
      r={QC_PILL_DETENT_RADIUS}
    />
  </svg>;
}
