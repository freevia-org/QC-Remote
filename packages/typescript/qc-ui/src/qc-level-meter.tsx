import "./qc-level-meter.css";

/**
 * The CorOS level meter - one control, drawn thirty-one times on the I/O pages.
 *
 * Every meter on every I/O page is the same 137 x 16 track, and it is not one
 * rectangle: it is 104 px of track, one column of the panel's own ink, then
 * 32 more. That one column is the zero tick.
 *
 * All thirty-one instances default to unlit, at the unit's own -40.0 dB idle
 * reading, so the track ink is the only ink any of them shows by default.
 *
 * THE TICK IS A GAP, NOT A MARK. The column at x 104 reads the editor panel's
 * own surface, so the device's raster cannot tell a one-pixel hole in the track
 * from a one-pixel tick painted in the panel colour over it. This draws two
 * track rectangles, which is pixel-identical to either and needs no third ink.
 *
 * `value` defaults to 0, the unit's idle reading; the lit ink is a role a
 * screen can restate. The tick's position is a length - 104 of 137 - not a
 * claim about which dB it marks.
 *
 * Nothing sets shapeRendering: every edge lands on a whole pixel at the unit's
 * own size, so neither setting has anything to do, and crispEdges would only
 * take the fallback away at other sizes.
 */

/** Track length in device pixels, tick included. */
export const QC_LEVEL_METER_LENGTH = 137;
/** Track height in device pixels. Sixteen on all thirty-one instances. */
export const QC_LEVEL_METER_HEIGHT = 16;
/** Where the track breaks, counted from its left edge. */
export const QC_LEVEL_METER_ZERO = 104;
/** How wide the break is. One column, on all thirty-one instances. */
export const QC_LEVEL_METER_ZERO_WIDTH = 1;

export type QcLevelMeterProps = {
  className?: string;
  /** How much of the track is lit, 0..1. Zero is the unit's own idle reading. */
  value?: number;
  /** Accessible name, where the meter is the control rather than decoration. */
  label?: string;
};

export function QcLevelMeter({ className, value = 0, label }: QcLevelMeterProps) {
  const lit = Math.max(0, Math.min(1, value)) * QC_LEVEL_METER_LENGTH;
  const tail = QC_LEVEL_METER_ZERO + QC_LEVEL_METER_ZERO_WIDTH;
  return (
    <svg
      className={`qc-level-meter${className ? ` ${className}` : ""}`}
      viewBox={`0 0 ${QC_LEVEL_METER_LENGTH} ${QC_LEVEL_METER_HEIGHT}`}
      preserveAspectRatio="none"
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <rect className="qc-level-meter-track" x="0" y="0" width={QC_LEVEL_METER_ZERO} height={QC_LEVEL_METER_HEIGHT} />
      <rect className="qc-level-meter-track" x={tail} y="0" width={QC_LEVEL_METER_LENGTH - tail} height={QC_LEVEL_METER_HEIGHT} />
      {lit > 0 && <rect className="qc-level-meter-lit" x="0" y="0" width={Math.min(lit, QC_LEVEL_METER_ZERO)} height={QC_LEVEL_METER_HEIGHT} />}
      {lit > tail && <rect className="qc-level-meter-lit" x={tail} y="0" width={lit - tail} height={QC_LEVEL_METER_HEIGHT} />}
    </svg>
  );
}
