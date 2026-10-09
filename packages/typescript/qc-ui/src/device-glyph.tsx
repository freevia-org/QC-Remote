import type { GridBlock } from "@qc-remote/client";
import { QC_COLORS } from "@qc-remote/theme";
import { blockUsesActiveFill, officialBlockVisual, OFFICIAL_BLOCK_CATEGORY_BY_KEY } from "./block-visuals";
import { QcDeviceCategoryGlyph } from "./device-category-glyph";
import { QC_BYPASSED_TILE_RUNG_SCALE, qcDimToRungs } from "./rgb565-dim";
import { QcPluginBadgeGlyph } from "./plugin-badge-glyph";
import { pluginBadge } from "./plugin-badges";
import { QcScreenVectorGlyph } from "./theme-icons";

/**
 * The margin the unit keeps around a block tile, as a fraction of the tile.
 *
 * A tile is not just its bordered square: the unit reserves three further
 * pixels of screen colour on every side and paints nothing else there, so the
 * whole item is 70 wide for a 64 tile. It shows wherever the tile sits on
 * something other than the screen colour - the chain cable stops three pixels
 * short of each tile on grid-base (x 55..65, then black to the border at 69),
 * and on gig-view and device-browser-plugin-models the tile sits in a black
 * 70-square cut out of the lit row: ring 418..487, border 421..484.
 */
const TILE_MARGIN = 3 / 64;

/** The single block-tile renderer used by live and reference surfaces. */
export function QcDeviceGlyph({ block, x, y, size = 64, selected = false, cpuLoad, cpuCore }: { block: GridBlock; x: number; y: number; size?: number; selected?: boolean; cpuLoad?: number; cpuCore?: number }) {
  // The unit draws a bypassed tile with ring and mark both stepped down its own
  // ladders (rgb565-dim.ts), never faded, and lays the bypass line across it at
  // full brightness: Rage Booster's ring and mark read 49,20,0 and 49,48,49
  // under a 198,195,198 line.
  const bypassed = block.bypassed === true;
  const rungScale = bypassed ? QC_BYPASSED_TILE_RUNG_SCALE : undefined;
  const bypassLine = bypassed ? <path className="official-block-bypass-line" d={`M${x - size / 2} ${y}H${x + size / 2}`} fill="none" stroke={QC_COLORS.device.bypassPath} strokeWidth="2" pointerEvents="none" aria-hidden="true" /> : null;
  // Same rounded-rect proportions as the tile, so the margin reads as one
  // shape grown by three pixels rather than a square behind a rounded tile.
  const margin = size * TILE_MARGIN;
  const surround = size + margin * 2;
  const tileSurround = <rect
    className="official-block-surround"
    x={x - surround / 2}
    y={y - surround / 2}
    width={surround}
    height={surround}
    rx={surround * .21875}
    fill={QC_COLORS.reference.screen}
    pointerEvents="none"
    aria-hidden="true"
  />;
  const visual = officialBlockVisual(block);
  const strokeWidth = selected ? 6 : 2;
  const inset = strokeWidth / 2;
  const normalizedCpuLoad = cpuLoad === undefined ? undefined : Math.max(0, Math.min(1, cpuLoad));
  const cpuFill = normalizedCpuLoad === undefined ? null : <rect
    className={`official-block-cpu-fill${cpuCore === 2 ? " is-core-2" : " is-core-1"}`}
    x={x - size / 2 + inset}
    y={y + size / 2 - inset - (size - strokeWidth) * normalizedCpuLoad}
    width={size - strokeWidth}
    height={(size - strokeWidth) * normalizedCpuLoad}
    rx={Math.min(size * .21875 - inset, (size - strokeWidth) * normalizedCpuLoad / 2)}
    fill={bypassed ? qcDimToRungs(visual.color) : visual.color}
    fillOpacity=".42"
    pointerEvents="none"
    aria-hidden="true"
  />;
  if (block.glyph === "capture-wave") {
    return <g className={`official-block-tile official-block-tile-capture${bypassed ? " is-bypassed" : ""}`}>
      {tileSurround}
      <QcScreenVectorGlyph icon="block.capture-wave" x={x - size / 2} y={y - size / 2} width={size} height={size} rungScale={rungScale} />
      {cpuFill}
      {bypassLine}
    </g>;
  }
  const badge = pluginBadge(block);
  const fill = blockUsesActiveFill(block) ? <rect
    className="official-block-active-fill"
    x={x - size / 2}
    y={y - size / 2}
    width={size}
    height={size}
    rx={size * .2}
    fill={visual.color}
    fillOpacity=".3"
    style={{ mixBlendMode: "screen" }}
    pointerEvents="none"
    aria-hidden="true"
  /> : null;
  const pluginLabel = badge ? <QcPluginBadgeGlyph abbreviation={badge} color={bypassed ? qcDimToRungs(visual.color) : visual.color} x={x} y={y} tileSize={size} /> : null;
  const iconSize = size * .625;
  // The unit paints a chain tile 64px across including its border: on the
  // Flanger editor and on grid-base the first tile spans x 69..132 and
  // y 115..178 inclusive. An SVG stroke straddles the path, so a rect of
  // `size` with a stroke on top measures size + strokeWidth and sits half a
  // stroke high and left of where the unit puts it. Inset the path by half the
  // stroke so the painted extent is `size`, and take the same half off the
  // corner radius so the outer corner keeps the unit's curvature.
  //
  // The border is two whole pixels, never 2.4. The unit's straight edges are
  // `0 0 0 148 148 0 0 0` - two columns of the border colour with a hard step
  // to black on both sides and no intermediate value anywhere, which is all
  // its RGB565 framebuffer could show in any case. A 2.4 stroke covers 2
  // pixels and 40% of a third, so Chrome blends that third one: at grid-base
  // row 147 that paints 69,70 solid and then a 40%-lit grey at 71 where the
  // unit is black, and the same row inside each of the other three edges.
  // That ring of part-lit pixels just inside the border is the unevenness on
  // an otherwise vector-clean shape. A whole-number stroke also makes the
  // inset whole, so the path itself lands on the pixel grid instead of on
  // x.2. The unit never shows a selected tile, so its width is ours to
  // choose; it is even for the same reason.
  return <g className={`official-block-tile official-block-tile-${visual.key}${bypassed ? " is-bypassed" : ""}`}>
    {tileSurround}
    <rect x={x - size / 2 + inset} y={y - size / 2 + inset} width={size - strokeWidth} height={size - strokeWidth} rx={size * .21875 - inset} fill={QC_COLORS.reference.screen} stroke={bypassed ? qcDimToRungs(visual.color) : visual.color} strokeWidth={strokeWidth} />
    {cpuFill}
    {fill}
    <QcDeviceCategoryGlyph label={OFFICIAL_BLOCK_CATEGORY_BY_KEY[visual.key].label} x={x - iconSize / 2} y={y - iconSize / 2} width={iconSize} height={iconSize} rungScale={rungScale} />
    {pluginLabel}
    {bypassLine}
  </g>;
}
