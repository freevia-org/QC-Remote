import type { ReactNode } from "react";
import type { CpuColumnLoad, PresetSnapshot } from "@qc-remote/client";
import { QC_TYPOGRAPHY } from "@qc-remote/theme";
import { MODE_LABEL, PRESET_TITLE_RIGHT_EDGE, presetTitlePresentation, RAIL_LABEL_FONT_SIZE, RAIL_LABEL_LINE_STEP, railLabelBaseline, SCENE_BADGE } from "./coros-ui";
import { QcDeviceGlyph } from "./device-glyph";
import { useFontFaceReady } from "./use-font-face-ready";
import { QcModeGlyph, QcScreenHeaderGlyph, QcScreenVectorGlyph } from "./theme-icons";

export function ModeGlyph({ mode }: { mode: PresetSnapshot["mode"] }) {
  return <QcModeGlyph mode={mode} />;
}


const BROWSER_TITLE_SIZE = 69;
// The one font spec the title is measured with and the one the hook waits for.
const BROWSER_TITLE_FONT = `800 ${BROWSER_TITLE_SIZE}px ${QC_TYPOGRAPHY.devicePlain}`;

function measureBrowserTitle(text: string) {
  if (typeof document === "undefined") return text.length * BROWSER_TITLE_SIZE * 0.575;
  const context = document.createElement("canvas").getContext("2d");
  if (!context) return text.length * BROWSER_TITLE_SIZE * 0.575;
  context.font = BROWSER_TITLE_FONT;
  return context.measureText(text).width;
}

// Geometry of the inline title drawn below (the <text> and its bank/letter/name tspans). A dirty name is its own
// element, so it takes its origin from these same values rather than repeating them.
const INLINE_TITLE = { x: 14, y: 76, size: 64, tracking: -2, gutter: 16 } as const;
// remaining-fixtures-fixes.css re-lays the title out for this class only: x 12, y 77, bank/letter at
// BROWSER_TITLE_SIZE without tracking, name 19 after them (the 12 and 19 are in browserTitleFit's start).
const BROWSER_GRID_TITLE_CLASS = "coros-device-browser-grid";
const BROWSER_GRID_TITLE_Y = 77;

// Advance of a bank/letter tspan at the inline size with the inline tracking (applied after every character).
function inlineTitleAdvance(text: string) {
  return (measureBrowserTitle(text) * INLINE_TITLE.size) / BROWSER_TITLE_SIZE + INLINE_TITLE.tracking * text.length;
}

function browserTitleFit(presetLocation: string, presetName: string) {
  const start = 12 + measureBrowserTitle(presetLocation) + 19;
  const maxWidth = Math.max(180, PRESET_TITLE_RIGHT_EDGE - start);
  const natural = Math.max(1, measureBrowserTitle(presetName));
  const fontSize = Math.max(22, Math.min(BROWSER_TITLE_SIZE, (BROWSER_TITLE_SIZE * maxWidth) / natural));
  return { start, fontSize, baselineRise: (BROWSER_TITLE_SIZE - fontSize) * 0.375 };
}

export function CorOsOfficialGrid({ snapshot, children, browserChrome = false, naturalTitleWidth = false, forceRouteLines = false, hideConnectionNodes = false, screenClassName, letterTone, cpuColumns }: { snapshot: PresetSnapshot; children?: ReactNode; browserChrome?: boolean; naturalTitleWidth?: boolean; forceRouteLines?: boolean; hideConnectionNodes?: boolean; screenClassName?: string; letterTone?: string; cpuColumns?: readonly CpuColumnLoad[] }) {
  // The fit below measures on a canvas during render, so it must run again once the face is usable.
  // The browser's title is the main Grid's: a dirty working buffer prints "name*" slanted. The displayed text,
  // asterisk included, is what is fitted (as presetTitleLayout does) and what the font-ready hook waits on.
  const title = browserChrome ? presetTitlePresentation(snapshot.presetName, snapshot.dirty) : undefined;
  const titleText = title?.text ?? snapshot.presetName;
  useFontFaceReady(BROWSER_TITLE_FONT, `${snapshot.presetLocation}${titleText}`, browserChrome);
  const titleFit = browserTitleFit(snapshot.presetLocation, titleText);
  const dirtyTitle = Boolean(title?.italic);
  // Same pivot and angle as the main Grid's title (quad-cortex-surface.tsx); an SVG transform cannot sit on a tspan,
  // so a dirty name is its own <text>, started where the non-dirty name tspan would start for this call site.
  const browserGridTitle = screenClassName?.split(/\s+/).includes(BROWSER_GRID_TITLE_CLASS) ?? false;
  const dirtyTitleX = browserGridTitle
    ? titleFit.start
    : INLINE_TITLE.x + inlineTitleAdvance(snapshot.presetLocation.slice(0, -1)) + inlineTitleAdvance(snapshot.presetLocation.slice(-1)) + INLINE_TITLE.gutter;
  const dirtyTitleY = (browserGridTitle ? BROWSER_GRID_TITLE_Y : INLINE_TITLE.y) - titleFit.baselineRise;
  const columns = [101, 187, 272, 357, 443, 529, 615, 701];
  const rowY = [147, 241, 335, 429];
  const screenBlocks = snapshot.blocks.filter((block) => block.row >= 0 && block.row < 4 && block.column >= 0 && block.column < 8);
  const routes = rowY.map((_, row) => snapshot.routes.find((route) => route.row === row));
  const routeLines = (label: string | undefined) => {
    const value = label || "+";
    const words = value.split(" ");
    return words.length > 1 ? [words[0], words.slice(1).join(" ")] : [value];
  };
  const railLabel = (label: string | undefined, x: number, y: number) => {
    const lines = routeLines(label);
    // `interface.add` owns this mark; see the same note in quad-cortex-surface.
    // The stroked pair this replaces was 1.7 units thick, so every one of its
    // pixels was 85% covered and none of them could reach the unit's flat ink.
    if ((label || "+") === "+") return <QcScreenVectorGlyph icon="interface.add" x={x - 12} y={y - 12} />;
    return (
      <text x={x} y={railLabelBaseline(y, lines.length)} fill="var(--qc-palette-e6e6e6)" stroke="none" fontFamily={QC_TYPOGRAPHY.deviceRoute} fontWeight="400" fontSize={RAIL_LABEL_FONT_SIZE}>
        {lines.map((line, index) => (
          <tspan key={`${line}-${index}`} x={x} dy={index ? RAIL_LABEL_LINE_STEP : 0}>
            {line}
          </tspan>
        ))}
      </text>
    );
  };
  return (
    <div className={`qc-screen coros-vector-screen${screenClassName ? ` ${screenClassName}` : ""}`} aria-label="CorOS Grid">
      <svg className="coros-vector-canvas" viewBox="0 0 800 480" role="img" aria-label={`${snapshot.presetLocation} ${snapshot.presetName}, ${snapshot.mode} mode`}>
        {/* The Grid this underlay draws is only ever seen through the browser
            and DSP washes, so this fill is a composite input rather than a
            surface role. The unit's washed ground is 66.69.66 on every browser
            frame, which the two-layer browser map reaches from this token and
            misses by four counts from var(--qc-screen-surface-base). */}
        <rect className="grid-screen-fill" width="800" height="480" fill="var(--qc-palette-020202)" />
        <text className="grid-preset-title" x={INLINE_TITLE.x} y={INLINE_TITLE.y} fill="var(--qc-grid-white-ink)" fontFamily={QC_TYPOGRAPHY.devicePlain} fontWeight="800" fontSize={INLINE_TITLE.size}>
          <tspan className="grid-preset-bank" letterSpacing={INLINE_TITLE.tracking}>
            {snapshot.presetLocation.slice(0, -1)}
          </tspan>
          <tspan className="grid-preset-letter" fill={letterTone ?? (browserChrome ? "var(--qc-palette-d63b3e)" : "var(--qc-palette-2df36a)")} letterSpacing={INLINE_TITLE.tracking}>
            {snapshot.presetLocation.slice(-1)}
          </tspan>
          {!dirtyTitle && <tspan className="grid-preset-name" dx={INLINE_TITLE.gutter} dy={browserChrome ? -titleFit.baselineRise : 0} fill="var(--qc-grid-white-ink)" fontSize={browserChrome ? titleFit.fontSize : 64} letterSpacing={browserChrome ? 0 : -2} textLength={browserChrome || naturalTitleWidth ? undefined : 313} lengthAdjust={browserChrome || naturalTitleWidth ? undefined : "spacingAndGlyphs"}>
            {titleText}
          </tspan>}
        </text>
        {dirtyTitle && (
          <text className="grid-preset-name-dirty" x={dirtyTitleX} y={dirtyTitleY} fill="var(--qc-grid-white-ink)" fontFamily={QC_TYPOGRAPHY.devicePlain} fontWeight="800" fontSize={titleFit.fontSize} letterSpacing={0} transform={`translate(${dirtyTitleX} ${dirtyTitleY}) skewX(-12) translate(${-dirtyTitleX} ${-dirtyTitleY})`}>
            {titleText}
          </text>
        )}
        <QcScreenHeaderGlyph kind="nav-undo" />
        {/* The browser frames show the plain floppy, as the main Grid does (device-browser-dsp-limited reference). */}
        <QcScreenHeaderGlyph kind={browserChrome ? "nav-save" : "export"} />
        <g className="grid-scene-badge">
          <rect x={SCENE_BADGE.x} y={SCENE_BADGE.y} width={SCENE_BADGE.size} height={SCENE_BADGE.size} rx={SCENE_BADGE.radius} fill="var(--qc-palette-f2cf32)" />
          <text x={SCENE_BADGE.letterX} y={SCENE_BADGE.letterBaseline} textAnchor="middle" fill="var(--qc-screen-surface-base)" fontFamily={QC_TYPOGRAPHY.devicePlain} fontWeight="800" fontSize={SCENE_BADGE.letterSize}>
            A
          </text>
        </g>
        <QcScreenHeaderGlyph kind="menu" />
        <g transform="translate(655 60)">
          <ModeGlyph mode={snapshot.mode} />
        </g>
        <text x={MODE_LABEL.x} y={MODE_LABEL.baseline} fill="var(--qc-palette-f0f0f0)" fontFamily={QC_TYPOGRAPHY.devicePlain} fontWeight="800" fontSize={MODE_LABEL.size}>
          {snapshot.mode}
        </text>
        <g className="grid-route-rails" fill="var(--qc-screen-surface-base)" stroke="var(--qc-screen-background)" strokeWidth="1.5" fontFamily={QC_TYPOGRAPHY.deviceRoute} textAnchor="middle">
          {/* The output rail is not the input's mirror: it fills x 750..793
              on every one of the four rows, where the input fills 8..51, and
              both run y 109..184 - 44 by 76, with no border. The unit steps
              straight from screen to pill on every edge, so a screen-coloured
              hairline only ever ate the pill's outermost pixel. The corner
              stays 15 rather than the marginally closer alternative, which
              costs more elsewhere than it wins here. */}
          {rowY.flatMap((y, row) => [<rect className="grid-route-input" key={`in-${row}`} x="8" y={y - 38} width="44" height="76" rx="15" stroke="none" />, <rect className="grid-route-output" key={`out-${row}`} x="750" y={y - 38} width="44" height="76" rx="15" stroke="none" />])}
          {rowY.map((y, row) => (
            <g key={`rails-${row}`}>
              {railLabel(routes[row]?.input, 30, y)}
              {/* 772, not 770: the output pill fills 750..793, so its centre
                  is 771.5. */}
              {railLabel(routes[row]?.output, 772, y)}
            </g>
          ))}
        </g>
        <g className="grid-route-cables" fill="none" stroke="var(--qc-device-split-path)" strokeWidth="1.4">
          {/* A port item reserves the same three pixels of screen colour a block
              tile does (see TILE_MARGIN in device-glyph.tsx). On grid-base the
              input rail fills to x 51, then black at 52..54, and only at 55 does
              the cable begin; on the far side the cable stops at 746 and the
              output rail starts at 750. Drawing from 52 to 748 painted over both
              margins. The tiles in between need no such care here - each one
              carries its own official-block-surround. */}
          {rowY.map((y, row) => (routes[row] && (forceRouteLines || screenBlocks.some((block) => block.row === row)) ? <path key={row} d={`M55 ${y}H747`} /> : null))}
        </g>
        {!browserChrome && !hideConnectionNodes && (
          <g aria-hidden="true">
            {[
              [53, 147, false],
              [587, 147, false],
              [159, 335, true],
            ].map(([x, y, dimmed]) => (
              <g key={`${x}-${y}`} transform={`translate(${x} ${y})`} opacity={dimmed ? 0.42 : 1}>
                <rect x="-8" y="-7" width="16" height="14" rx="5" fill="var(--qc-screen-text-primary)" />
                <circle cx="-3" cy="0" r="2" fill="var(--qc-palette-171917)" />
                <path d="M0-3 5 0 0 3Z" fill="var(--qc-palette-171917)" />
              </g>
            ))}
          </g>
        )}
        <g>
          {screenBlocks.map((block) => {
            const cx = columns[block.column];
            const cy = rowY[block.row];
            return (
              <g key={block.id}>
                <QcDeviceGlyph block={block} x={cx} y={cy} cpuLoad={cpuColumns?.[block.row * 8 + block.column]?.load} cpuCore={cpuColumns?.[block.row * 8 + block.column]?.core} />
              </g>
            );
          })}
        </g>
      </svg>
      {children}
    </div>
  );
}
