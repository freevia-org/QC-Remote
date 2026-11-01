import { COROS_HEADER_FONT, COROS_HEADER_GLYPHS, COROS_HEADER_KERNS } from "./coros-header-glyphs.ts";

/** The live Grid uses a discrete title-size ladder from 22px through 70px. */
export const COROS_HEADER_OUTLINE_SIZE_RANGE = { minimum: 22, maximum: 70 } as const;

export type CorOsHeaderFallback = "disabled" | "unsupported-text" | "unsupported-size" | "ligature" | "whitespace" | "dirty" | "transformed" | "squeezed" | "spaced" | "unsupported-style" | "invalid-geometry" | "outline-overflow";

export interface CorOsHeaderOutlineAdmission {
  enabled?: boolean;
  dirty?: boolean;
  transformed?: boolean;
  squeezed?: boolean;
  spaced?: boolean;
  unsupportedStyle?: boolean;
}

export interface CorOsHeaderGlyphPlacement {
  /** Index in the original string, also used to preserve run-specific colors. */
  index: number;
  path: string;
  pen: number;
}

export interface CorOsHeaderGlyphLayout {
  glyphs: readonly CorOsHeaderGlyphPlacement[];
  advance: number;
  scale: number;
  advanceMode: "fractional";
  /** Exact design-outline bounds after scale and rounded placement; baseline is zero. */
  inkBounds: { left: number; top: number; right: number; bottom: number } | undefined;
  fitsMaxWidth: boolean;
}

export function corOsHeaderOutlineFallback(text: string, fontSize: number, options: CorOsHeaderOutlineAdmission = {}): CorOsHeaderFallback | undefined {
  if (!options.enabled) return "disabled";
  if (!Number.isFinite(fontSize) || fontSize <= 0) return "invalid-geometry";
  if (fontSize < COROS_HEADER_OUTLINE_SIZE_RANGE.minimum || fontSize > COROS_HEADER_OUTLINE_SIZE_RANGE.maximum) return "unsupported-size";
  if (options.dirty) return "dirty";
  if (options.transformed) return "transformed";
  if (options.squeezed) return "squeezed";
  if (options.spaced) return "spaced";
  if (options.unsupportedStyle) return "unsupported-style";
  if (!/^[\x20-\x7e]*$/.test(text)) return "unsupported-text";
  // SVG text collapses edge/repeated spaces under the existing whitespace policy.
  // Preserve that complete shaping behavior rather than painting literal gaps.
  if (text.startsWith(" ") || text.endsWith(" ") || text.includes("  ")) return "whitespace";
  // Pair positioning is modeled; glyph substitution is deliberately delegated to SVG text.
  if (COROS_HEADER_FONT.fallbackLigatureSequences.some(sequence => text.includes(sequence))) return "ligature";
  return undefined;
}

function roundAwayFromZero(value: number): number {
  return Math.sign(value) * Math.floor(Math.abs(value) + 0.5);
}

/**
 * Current-font design metrics only: no captured strings, per-string positions,
 * browser character origins, native masks or native-library dependency.
 *
 * The live Grid chooses from a discrete 22px-to-70px title-size ladder. The
 * outline layout also admits intermediate sizes for safe fitted rendering.
 * Preserve fractional advances and kerning,
 * then round each painted pen once so error cannot accumulate over a long title.
 * Shaping, dirty/sheared text and fitted runs retain their existing SVG behavior.
 */
export function layoutCorOsHeaderGlyphs(text: string, fontSize: number, x = 0, maxWidth?: number, letterSpacing = 0): CorOsHeaderGlyphLayout | undefined {
  if (!Number.isFinite(x) || !Number.isFinite(letterSpacing) || (maxWidth !== undefined && (!Number.isFinite(maxWidth) || maxWidth < 0))
    || corOsHeaderOutlineFallback(text, fontSize, { enabled: true })) return undefined;
  const scale = fontSize / COROS_HEADER_FONT.unitsPerEm;
  const trackingUnits = letterSpacing / scale;
  // Accumulate integral design units before scaling. Adding each scaled double
  // separately can turn an exact .5px boundary into .4999999999998.
  let designUnits = 0;
  const glyphs: CorOsHeaderGlyphPlacement[] = [];
  let inkBounds: CorOsHeaderGlyphLayout["inkBounds"];
  for (let index = 0; index < text.length; index += 1) {
    const codepoint = text.charCodeAt(index);
    const { advanceUnits, path, bounds } = COROS_HEADER_GLYPHS[codepoint - COROS_HEADER_FONT.firstCodepoint];
    const pen = (x * COROS_HEADER_FONT.unitsPerEm + designUnits * fontSize) / COROS_HEADER_FONT.unitsPerEm;
    const paintPen = roundAwayFromZero(pen);
    glyphs.push({ index, path, pen: paintPen });
    if (bounds) {
      const em = COROS_HEADER_FONT.unitsPerEm;
      // A common-unit numerator also prevents distributive floating-point
      // rounding from falsely rejecting an ink edge exactly on maxWidth.
      const glyphBounds = { left: (paintPen * em + bounds[0] * fontSize) / em, top: -bounds[3] * fontSize / em,
        right: (paintPen * em + bounds[2] * fontSize) / em, bottom: -bounds[1] * fontSize / em };
      inkBounds = inkBounds ? { left: Math.min(inkBounds.left, glyphBounds.left), top: Math.min(inkBounds.top, glyphBounds.top),
        right: Math.max(inkBounds.right, glyphBounds.right), bottom: Math.max(inkBounds.bottom, glyphBounds.bottom) } : glyphBounds;
    }
    designUnits += advanceUnits;
    if (index + 1 < text.length) {
      const units = COROS_HEADER_KERNS[`${codepoint},${text.charCodeAt(index + 1)}`] ?? 0;
      designUnits += units;
    }
    designUnits += trackingUnits;
  }
  const advance = designUnits * fontSize / COROS_HEADER_FONT.unitsPerEm;
  return { glyphs, advance, scale, advanceMode: "fractional", inkBounds,
    fitsMaxWidth: maxWidth === undefined || inkBounds === undefined || inkBounds.right <= x + maxWidth };
}
