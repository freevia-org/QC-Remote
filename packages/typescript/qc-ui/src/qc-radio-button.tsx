import type { CSSProperties } from "react";
import "./qc-radio-button.css";

/**
 * The CorOS selection mark - a circle and a square checkbox, one construction.
 *
 * WHAT THE UNIT ACTUALLY DRAWS. The two shapes are the same mark with one
 * number changed. Both are a 22-wide outline of stroke 2 inside a 24 box; the
 * circle is that outline at corner radius 11 and the checkbox at corner
 * radius 6. Nothing else differs.
 *
 * The green ring the mark appears to carry when it is selected is not drawn.
 * It is the accent plate showing through the 2px gap between the dot at
 * radius 7 and the ring's inner edge at radius 9 - so the three concentric
 * parts are two pieces of ink and a hole.
 *
 * THE PLATE. Where the mark sits on a rounded plate, the plate's own height
 * and corner radius are fixed (44 tall, corner 8); the width is the call
 * site's - 66 for a single-line label, 98 for the directory's Multi Select -
 * so width is a prop and the height and corner are not.
 *
 * The checked-checkbox state places the same dot the selected radio uses,
 * in the same place, for a filled centre on this mark.
 *
 * Nothing here sets shapeRendering. The unit antialiases its curves - the ring
 * carries a full coverage ramp from 0 at radius 8.4 to 1 at radius 9.6 - and
 * crispEdges would stair-step every one of them.
 *
 * Colour is a role, not a literal. The mark is the screen's primary text ink
 * when unselected, and selection inverts it: the plate becomes the screen
 * accent and the mark becomes the screen background. All four are exact
 * palette roles, not alpha blends of one another. The plate is the screen's
 * raised surface when unselected, not its disabled surface.
 */

/** Outer extent of the mark, in device pixels. One viewBox unit equals one device pixel. */
export const QC_RADIO_MARK_EXTENT = 22;
/** Outline stroke width: 2. */
export const QC_RADIO_MARK_STROKE = 2;
/** The sprite box the unit ships the mark in - `icons/24/`, 1px of margin. */
export const QC_RADIO_BOX = 24;
/** Selected centre dot diameter: 14. */
export const QC_RADIO_DOT = 14;
/** Plate height: 44. */
export const QC_RADIO_PLATE_HEIGHT = 44;
/** Plate corner radius: 8. */
export const QC_RADIO_PLATE_CORNER = 8;
/** Plate width where a call site does not say - both 66-wide instances. */
export const QC_RADIO_PLATE_WIDTH = 66;

/**
 * Outer corner radius per shape. `radio` at 11 is a circle; `check` at 6 is the
 * rounded square. These are the only two the unit draws.
 */
export const QC_RADIO_SHAPE_CORNER = { radio: 11, check: 6 } as const;

export type QcRadioShape = keyof typeof QC_RADIO_SHAPE_CORNER;

export type QcRadioButtonProps = {
  className?: string;
  /**
   * Which of the unit's two marks. `radio` is the circle; `check` is the
   * square checkbox. They differ only in corner radius.
   */
  shape?: QcRadioShape;
  /**
   * Whether the mark carries its centre dot. This is the whole point of the
   * component: a selection can move, so the state is a parameter rather than a
   * second stylesheet rule.
   */
  selected?: boolean;
  /**
   * Draw the rounded plate behind the mark. Off by default: the directory's
   * per-row boxes are bare marks on the row itself.
   */
  plate?: boolean;
  /** Plate width in device pixels. The height and corner are fixed - this is the only dimension a call site sets. */
  plateWidth?: number;
  /** Mark colour. Defaults to the screen's primary ink through the stylesheet. */
  ink?: string;
  /** Plate colour. Defaults to the screen's raised surface through the stylesheet. */
  surface?: string;
  /** Accessible name, where the mark is the control rather than decoration. */
  label?: string;
};

export function QcRadioButton({
  className,
  shape = "radio",
  selected = false,
  plate = false,
  plateWidth = QC_RADIO_PLATE_WIDTH,
  ink,
  surface,
  label
}: QcRadioButtonProps) {
  const width = plate ? plateWidth : QC_RADIO_BOX;
  const height = plate ? QC_RADIO_PLATE_HEIGHT : QC_RADIO_BOX;
  const cx = width / 2;
  const cy = height / 2;
  // The stroke straddles the path, so the centreline sits half a stroke inside
  // the mark's outer extent and its corner shrinks by the same half.
  const half = (QC_RADIO_MARK_EXTENT - QC_RADIO_MARK_STROKE) / 2;
  const corner = QC_RADIO_SHAPE_CORNER[shape] - QC_RADIO_MARK_STROKE / 2;
  const style = {
    ...(plate ? { "--qc-radio-plate-width": plateWidth } : {}),
    ...(ink === undefined ? {} : { "--qc-radio-ink": ink }),
    ...(surface === undefined ? {} : { "--qc-radio-surface": surface })
  } as CSSProperties;

  return <svg
    className={`qc-radio-button${className ? ` ${className}` : ""}`}
    viewBox={`0 0 ${width} ${height}`}
    data-shape={shape}
    data-selected={selected ? "true" : "false"}
    data-plate={plate ? "true" : "false"}
    style={style}
    role={label ? "img" : undefined}
    aria-label={label}
    aria-hidden={label ? undefined : true}
  >
    {plate && <rect
      className="qc-radio-plate"
      x="0"
      y="0"
      width={width}
      height={height}
      rx={QC_RADIO_PLATE_CORNER}
    />}
    <rect
      className="qc-radio-mark"
      x={cx - half}
      y={cy - half}
      width={half * 2}
      height={half * 2}
      rx={corner}
    />
    {selected && <circle className="qc-radio-dot" cx={cx} cy={cy} r={QC_RADIO_DOT / 2} />}
  </svg>;
}
