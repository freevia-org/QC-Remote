import type { ReactNode } from "react";
import { QC_COLORS } from "@qc-remote/theme";
import { QcDeviceCategoryGlyph, qcDeviceCategoryInkCentre } from "./device-category-glyph";

/**
 * The block/device tile, drawn once for every screen that shows one.
 *
 * The unit draws a single tile construction for every screen that shows a
 * device, rather than a bespoke drawing per screen:
 *
 *   tile           64x64 including its ring, ring 2 units thick, corner
 *                  radius 14
 *   selected ring  4 units, not 2
 *   surround       3 further units of the tile's own ground on every side, so
 *                  the whole item is 70. Visible wherever the tile sits on
 *                  something else.
 *   art            40 units centred, the same box `QcDeviceGlyph` gives the
 *                  Grid's tiles
 *   badge          a 24-unit disc centred at (7, 57), straddling the ring at
 *                  the lower left. The first tile carries no badge: nothing
 *                  but ground lies in that corner.
 *
 * Inks are the caller's, because the unit dims a tile by substituting rungs on
 * its 5/6/5 ladders rather than by fading one value, and the rung differs per
 * screen. What does not differ is the geometry, which is why it lives here.
 */

/** The tile, its ring included. Every other number below is in these units. */
const TILE = 64;
/** 64 * .21875, the corner the unit draws on every tile. */
const RADIUS = 14;
/** The ground the unit reserves around a tile and paints nothing else in. */
const SURROUND = 3;
/** 64 * .625 - the box `QcDeviceGlyph` already gives the Grid's category art. */
const ART = 40;
/**
 * The badge is the same 40-unit capture mark at 30, not a 24-unit asset: its
 * disc is 32 of those 40 units, so a 24-unit disc needs a 30-unit box, and only
 * that scale reproduces the unit's 4.5-pixel stripe pitch (209, 214, 218, 223
 * down the middle of the Cab badge on `capture-save`).
 */
const BADGE_BOX = 30;
const BADGE_DISC = 24;
const BADGE_CENTRE_X = 7;
const BADGE_CENTRE_Y = 57;
const BADGE_CATEGORY = "Neural Capture";
/** Room in the viewBox for the surround and for the badge's overhang. */
const BLEED = 6;
const ITEM = TILE + BLEED * 2;

/**
 * The tile's own rendered edge (CSS pixels, ring and bleed included) at a
 * given `size` prop. A caller that needs to centre the tile at a point in
 * its own coordinate system - `quad-cortex-surface.tsx`'s SVG canvas nests
 * `QcDeviceTile` as an inner `<svg>`, which positions from its top-left, not
 * its centre - halves this to get the offset, rather than duplicating the
 * TILE/BLEED relationship at the call site.
 */
export function qcDeviceTileRenderedSize(size: number = TILE): number {
  return (size * ITEM) / TILE;
}

export interface QcDeviceTileProps {
  /** A `QcDeviceCategoryGlyph` label, for a tile whose mark is library art. */
  category?: string;
  /** Art for a tile the library has no mark for; drawn in the same 40-unit box. */
  art?: ReactNode;
  /** The tile's edge in CSS pixels, ring included. */
  size?: number;
  /** The unit widens the ring to 4 units on the tile a screen has chosen. */
  selected?: boolean;
  /** Paint the 3-unit ground ring, for a tile that sits on another colour. */
  surround?: boolean;
  /** The capture disc the unit overlays at the lower left of a capture's tile. */
  badge?: boolean;
  className?: string;
}

export function QcDeviceTile({ category, art, size = TILE, selected = false, surround = false, badge = false, className }: QcDeviceTileProps) {
  // An SVG stroke straddles its path, so a 64 rect with a ring on top measures
  // 64 + ring. Inset by half the ring and take the same half off the radius, and
  // the painted extent is the 64 the unit draws, with the outer corner keeping
  // its curvature - the same correction `QcDeviceGlyph` makes for the Grid.
  const ring = selected ? 4 : 2;
  const inset = ring / 2;
  const ground = `var(--qc-device-tile-ground, ${QC_COLORS.reference.screen})`;
  // The unit centres the mark's ink on the tile, not the box it was traced
  // into; the two are a unit apart on every mark these screens draw.
  const artCentre = (category && qcDeviceCategoryInkCentre(category)) || { x: ART / 2, y: ART / 2 };
  const badgeCentre = qcDeviceCategoryInkCentre(BADGE_CATEGORY) ?? { x: ART / 2, y: ART / 2 };
  const badgeScale = BADGE_BOX / ART;
  return (
    <svg
      className={`qc-device-tile${selected ? " is-selected" : ""}${className ? ` ${className}` : ""}`}
      width={(size * ITEM) / TILE}
      height={(size * ITEM) / TILE}
      viewBox={`${-BLEED} ${-BLEED} ${ITEM} ${ITEM}`}
      aria-hidden="true"
    >
      {surround && <rect className="qc-device-tile-surround" x={-SURROUND} y={-SURROUND} width={TILE + SURROUND * 2} height={TILE + SURROUND * 2} rx={(TILE + SURROUND * 2) * (RADIUS / TILE)} fill={ground} />}
      <rect
        className="qc-device-tile-box"
        x={inset}
        y={inset}
        width={TILE - ring}
        height={TILE - ring}
        rx={RADIUS - inset}
        fill={ground}
        stroke={`var(--qc-device-tile-ink, ${QC_COLORS.reference.utilityMark})`}
        strokeWidth={ring}
      />
      {/* `fill`/`stroke` here are inherited, so the registry art's own
          per-layer presentation attributes still win and only the outline art
          a caller passes in picks them up. */}
      <g className="qc-device-tile-art" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
        {category
          ? <QcDeviceCategoryGlyph label={category} x={TILE / 2 - artCentre.x} y={TILE / 2 - artCentre.y} width={ART} height={ART} />
          : <svg x={(TILE - ART) / 2} y={(TILE - ART) / 2} width={ART} height={ART} viewBox={`0 0 ${ART} ${ART}`} overflow="visible">{art}</svg>}
      </g>
      {badge && (
        <g className="qc-device-tile-badge">
          <circle cx={BADGE_CENTRE_X} cy={BADGE_CENTRE_Y} r={BADGE_DISC / 2} fill={`var(--qc-device-tile-badge-ground, ${QC_COLORS.reference.screen})`} />
          <QcDeviceCategoryGlyph label={BADGE_CATEGORY} x={BADGE_CENTRE_X - badgeCentre.x * badgeScale} y={BADGE_CENTRE_Y - badgeCentre.y * badgeScale} width={BADGE_BOX} height={BADGE_BOX} />
        </g>
      )}
    </svg>
  );
}
