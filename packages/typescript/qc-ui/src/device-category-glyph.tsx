import { QC_COLORS, QC_SCREEN_ICON_VECTORS, QC_TYPOGRAPHY } from "@qc-remote/theme";
import { QcScreenVectorGlyph } from "./theme-icons";

/**
 * Neutral functional category symbols shared by the live grid
 * and every CorOS browser/fixture. Visible geometry follows the device UI.
 *
 * There is no text `fallback`: the unit never substitutes a character for a
 * missing symbol, and the prop that once offered one was declared but never
 * destructured, so every `fallback="≋"` at a call site drew nothing. A label
 * with no branch returns null, and the call site must name a real glyph.
 */
export interface QcDeviceCategoryGlyphProps {
  label: string;
  className?: string;
  x?: number;
  y?: number;
  width?: number;
  height?: number;
  /** Substitute every ink's rung by this scale (see rgb565-dim.ts), for a dimmed tile. */
  rungScale?: number;
}

// Every mark the device browser paints down its category rail. The geometry
// is in the shared registry with the rest of the icon library; this map is
// the only thing the label needs.
const DEVICE_CATEGORY_ICONS = {
  "Plugins": "device-category.plugins",
  "Amp": "device-category.amp",
  "Neural Capture": "device-category.capture",
  "Cab": "device-category.cab",
  "Overdrive": "device-category.overdrive",
  "Delay": "device-category.delay",
  "Reverb": "device-category.reverb",
  "Compressor": "device-category.compressor",
  "Pitch": "device-category.pitch",
  "Modulation": "device-category.modulation",
  "Morph": "device-category.morph",
  "Synth": "device-category.synth",
  "Filter": "device-category.filter",
  "Equalizer": "device-category.equalizer",
  "IR Loader": "device-category.ir-loader",
  "Wah": "device-category.wah",
  "FX Loop": "device-category.fx-loop",
  "Looper": "device-category.looper",
  "Utility": "device-category.utility",
} as const;

export function QcDeviceCategoryGlyph({ label, className, x, y, width, height, rungScale }: QcDeviceCategoryGlyphProps) {
  const icon = DEVICE_CATEGORY_ICONS[label as keyof typeof DEVICE_CATEGORY_ICONS];
  if (!icon) return null;
  return <QcScreenVectorGlyph icon={icon} className={`qc-device-category-glyph${className ? ` ${className}` : ""}`} x={x} y={y} width={width} height={height} rungScale={rungScale} />;
}

/**
 * Where a category mark's ink sits inside its 40-unit box.
 *
 * A tile centres what the mark draws, not the box it was traced into, and the
 * two differ: `device-category.cab`, `.amp` and `.capture` all centre their ink
 * on y 19 while `.utility` centres on 21. The unit does the same on its own
 * tiles: `capture-save` puts the Cab ring at 177..206 and the Amp box at
 * 183..200 on a tile at y 160..223, one unit lower than a centred box would,
 * and `input-gate-control` puts the Utility mark at 133..160 on a tile at
 * 115..178, one unit higher. Both are the ink centred to the pixel.
 *
 * The registry's marks are polylines, so the extent is exact rather than
 * sampled; a mark with a command this does not know returns undefined and the
 * caller falls back to centring the box.
 */
const INK_CENTRES = new Map<string, { x: number; y: number } | undefined>();

export function qcDeviceCategoryInkCentre(label: string): { x: number; y: number } | undefined {
  const icon = DEVICE_CATEGORY_ICONS[label as keyof typeof DEVICE_CATEGORY_ICONS];
  if (!icon) return undefined;
  if (!INK_CENTRES.has(icon)) INK_CENTRES.set(icon, measureInkCentre(icon));
  return INK_CENTRES.get(icon);
}

function measureInkCentre(icon: keyof typeof QC_SCREEN_ICON_VECTORS): { x: number; y: number } | undefined {
  let left = Infinity, right = -Infinity, top = Infinity, bottom = -Infinity;
  for (const layer of QC_SCREEN_ICON_VECTORS[icon].layers) {
    const tokens = layer.path.match(/[A-Za-z]|-?\d*\.?\d+/g) ?? [];
    let x = 0, y = 0, command = "";
    for (let index = 0; index < tokens.length; index += 1) {
      const token = tokens[index];
      if (/[A-Za-z]/.test(token)) {
        if (!"MLHVZmlhvz".includes(token)) return undefined;
        command = token;
        continue;
      }
      if (command === "M" || command === "L") { x = Number(token); y = Number(tokens[index += 1]); }
      else if (command === "m" || command === "l") { x += Number(token); y += Number(tokens[index += 1]); }
      else if (command === "H") x = Number(token);
      else if (command === "h") x += Number(token);
      else if (command === "V") y = Number(token);
      else if (command === "v") y += Number(token);
      else continue;
      left = Math.min(left, x); right = Math.max(right, x);
      top = Math.min(top, y); bottom = Math.max(bottom, y);
    }
  }
  if (right < left || bottom < top) return undefined;
  return { x: (left + right) / 2, y: (top + bottom) / 2 };
}

/**
 * The stomp tile Gig View draws around a category glyph. It lives beside the
 * category registry rather than in the icon registry because it composes
 * QcDeviceCategoryGlyph, and the icon registry may not depend back on this file.
 */
export function QcGigStompGlyph({ index }: { index: number }) {
  const categories = ["Wah", "Overdrive", "Overdrive", "Overdrive", "Looper", "Pitch", "Utility", "Reverb"] as const;
  const labels = ["WAH", "DRV", "DRV", "DRV", "LOP", "PIT", "MULTI", "RVB"];
  const colors = [QC_COLORS.category.wah, QC_COLORS.category.overdrive, QC_COLORS.category.overdrive, QC_COLORS.category.overdrive, QC_COLORS.category.looper, QC_COLORS.category.pitch, QC_COLORS.category.utility, QC_COLORS.category.reverb];
  // The category art is a 40-unit vector, several layers of it crispEdges pixel
  // art. The outer frame is 70 units drawn at 70px, so one outer unit is one CSS
  // pixel: giving the nested art its own 40 units on integer x/y keeps every art
  // unit on the pixel grid. Drawing it into 30 units put it on a 0.75 scale at
  // quarter-pixel origins, which is what broke the rows up.
  return <svg className="qc-gig-stomp-glyph" viewBox="0 0 70 70" data-qc-gig-stomp={index} aria-hidden="true"><rect x="4" y="4" width="62" height="62" rx="14" fill={QC_COLORS.reference.screen} stroke={colors[index]} strokeWidth="2.4" /><QcDeviceCategoryGlyph label={categories[index]} x={15} y={7} width={40} height={40} /><text x="35" y="54" textAnchor="middle" fill={QC_COLORS.reference.primaryText} fontFamily={QC_TYPOGRAPHY.devicePlain} fontSize="8" fontWeight="800">{labels[index]}</text></svg>;
}
