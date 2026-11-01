import type { SVGProps } from "react";
import { QC_TYPOGRAPHY } from "@qc-remote/theme";
import { corOsHeaderOutlineFallback, layoutCorOsHeaderGlyphs } from "./coros-header-layout.ts";

export interface CorOsHeaderTextRun {
  text: string;
  fill?: string;
}

export interface CorOsHeaderOutlineTextProps extends Omit<SVGProps<SVGTextElement>, "children" | "x" | "y" | "fontSize" | "ref"> {
  runs: readonly CorOsHeaderTextRun[];
  x: number;
  y: number;
  fontSize: number;
  /** Explicitly opt in while the candidate remains under investigation. */
  enabled?: boolean;
  dirty?: boolean;
  /** Preserve the existing available title edge even when rounded advances differ. */
  maxWidth?: number;
  "data-grid-header"?: string;
}

function hasSpacing(value: string | number | undefined): boolean {
  return value !== undefined && value !== "normal" && Number(value) !== 0;
}

// Admit only SVG attributes whose semantics survive replacing text with paths.
// Unknown props fall back to the original text so events and rendering hints are
// never silently dropped by the prototype.
const OUTLINE_ATTRIBUTES = new Set([
  "id", "className", "style", "role", "tabIndex", "focusable",
  "fill", "fillOpacity", "color", "opacity", "visibility", "display",
  "fontStyle", "fontFamily", "fontWeight", "fontVariant", "fontStretch",
  "textAnchor", "dominantBaseline", "alignmentBaseline", "textDecoration",
  "direction", "unicodeBidi", "letterSpacing", "wordSpacing",
  "transform", "textLength", "lengthAdjust", "dx", "dy", "rotate"
]);
const OUTLINE_STYLES = new Set(["fill", "fillOpacity", "color", "opacity", "visibility", "display", "letterSpacing"]);

function hasUnsupportedTextStyle(attributes: Omit<CorOsHeaderOutlineTextProps, "runs" | "x" | "y" | "fontSize" | "enabled" | "dirty" | "maxWidth">): boolean {
  const style = attributes.style;
  if (Object.entries(attributes).some(([name, value]) => value !== undefined
    && !OUTLINE_ATTRIBUTES.has(name) && !name.startsWith("aria-") && !name.startsWith("data-"))) return true;
  if (style && Object.entries(style).some(([name, value]) => value !== undefined && !OUTLINE_STYLES.has(name))) return true;
  return Boolean((attributes.fontStyle && attributes.fontStyle !== "normal")
    || (attributes.fontFamily && attributes.fontFamily !== QC_TYPOGRAPHY.devicePlain)
    || (attributes.textAnchor && attributes.textAnchor !== "start")
    || (attributes.dominantBaseline && !["auto", "alphabetic"].includes(attributes.dominantBaseline))
    || (attributes.alignmentBaseline && !["auto", "baseline"].includes(attributes.alignmentBaseline))
    || (attributes.fontVariant && attributes.fontVariant !== "normal")
    || (attributes.fontStretch && attributes.fontStretch !== "normal")
    || (attributes.textDecoration && attributes.textDecoration !== "none")
    || (attributes.direction && attributes.direction !== "ltr")
    || (attributes.unicodeBidi && attributes.unicodeBidi !== "normal")
    || attributes.dx !== undefined || attributes.dy !== undefined || attributes.rotate !== undefined
    || (attributes.fontWeight && !["700", "800", "bold"].includes(String(attributes.fontWeight))));
}

/** One parameterized paint implementation for the bank/slot and preset-title runs. */
export function CorOsHeaderOutlineText({ runs, x, y, fontSize, enabled = false, dirty = false, maxWidth, ...attributes }: CorOsHeaderOutlineTextProps) {
  const text = runs.map(run => run.text).join("");
  let fallback = corOsHeaderOutlineFallback(text, fontSize, {
    enabled,
    dirty,
    transformed: Boolean(attributes.transform),
    squeezed: attributes.textLength !== undefined || attributes.lengthAdjust !== undefined,
    spaced: hasSpacing(attributes.letterSpacing) || hasSpacing(attributes.wordSpacing)
      || hasSpacing(attributes.style?.letterSpacing) || hasSpacing(attributes.style?.wordSpacing),
    unsupportedStyle: hasUnsupportedTextStyle(attributes)
  });
  if (!Number.isFinite(x) || !Number.isFinite(y) || (maxWidth !== undefined && (!Number.isFinite(maxWidth) || maxWidth < 0))) fallback = "invalid-geometry";
  const layout = !fallback ? layoutCorOsHeaderGlyphs(text, fontSize, x, maxWidth) : undefined;
  if (layout && !layout.fitsMaxWidth) fallback = "outline-overflow";
  if (!layout || fallback) {
    return <text {...attributes} x={x} y={y} fontSize={fontSize} data-outline-mode="text" data-outline-fallback={fallback ?? "invalid-geometry"}>
      {runs.map((run, index) => <tspan key={index} fill={run.fill}>{run.text}</tspan>)}
    </text>;
  }
  let end = 0;
  const colors = runs.map(run => ({ end: end += run.text.length, fill: run.fill }));
  const hidden = attributes["aria-hidden"] === true || attributes["aria-hidden"] === "true";
  // SVG presentation/events use the same props on this group; a text-element ref
  // is intentionally not part of the component API. Callers inside the labeled
  // device SVG pass aria-hidden=true; standalone use has one named image semantic.
  const groupAttributes = attributes as unknown as SVGProps<SVGGElement>;
  return <g {...groupAttributes} fontSize={fontSize}
    role={hidden ? undefined : attributes.role ?? "img"}
    aria-label={hidden ? undefined : attributes["aria-label"] ?? text}
    data-qc-outline-text="true" data-outline-mode="paths" data-outline-fallback="none"
    data-outline-advance={layout.advance} data-outline-advance-mode={layout.advanceMode}>
    {layout.glyphs.map(glyph => glyph.path ? <path key={glyph.index} d={glyph.path}
      transform={`translate(${glyph.pen} ${y}) scale(${layout.scale} ${-layout.scale})`}
      fill={colors.find(color => glyph.index < color.end)?.fill}
      aria-hidden="true" /> : null)}
  </g>;
}
