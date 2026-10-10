import { useId } from "react";
import type { PresetSnapshot } from "@qc-remote/client";
import { QC_COLORS, QC_SCREEN_ICON_VECTORS, QC_SCENE_SHUTTLE_REVERSE, QC_SELECTOR_TRIANGLE_TRANSFORMS, QC_TYPOGRAPHY, type QcScreenIconLayerRole, type QcScreenIconRendering, type QcSelectorTriangleOrientation } from "@qc-remote/theme";
import { qcDimToRungs } from "./rgb565-dim";
import { QC_ROTARY_START, qcRotaryAngle, qcRotaryValue } from "./qc-rotary-dial";
import { JAN_WAVING_HAND, QC_ANTIGRAVITY_GRADIENT_STOPS, QC_BIONIC_BARS, QC_BIONIC_BAR_SIZE, QC_LOCAL_APP_LOGO_MARKS } from "./device-reference/local-app-logos";
import {
  QC_CAPTURE_TYPE_GLYPH_GEOMETRY, QC_CONTEXT_CAPTURE_PATHS, QC_CPU_GLYPH, QC_DEVICE_EMPTY_SLOT_PLATE, QC_DEVICE_WAVEFORM_GLYPH,
  QC_DIRECTORY_BROOM_PATHS, QC_EXPRESSION_ASSIGNMENT_LINK_PATHS, QC_LOOPER_ACTION_GLYPH_GEOMETRY, QC_PRESET_STACK_PATHS,
  QC_ROUTE_GLYPH_GEOMETRY, QC_SCREEN_GLYPH_GEOMETRY, QC_SCREEN_HEADER_SAVE_GLYPH, QC_SETTINGS_GLYPH_PATHS, QC_SETTINGS_KNOB_GEOMETRY,
  QC_SETTINGS_STORAGE_GEOMETRY, QC_SYNTH_FOOTER_PATHS, QC_WIFI_SIGNAL_GEOMETRY
} from "./device-reference/theme-icon-geometry";

export type QcDirectoryIconName = "grid" | "download" | "cloud" | "cloud-upload" | "folder" | "folder-outline" | "new-folder" | "sort" | "filter" | "arrange" | "upload" | "search" | "trash" | "done";
export type QcEditorIconName = "save" | "change" | "copy" | "paste" | "reset" | "expression" | "looper" | "mute" | "model-update" | "model-downgrade" | "remove" | "assignment-expression" | "band-power" | "footswitch" | "momentary" | "scene-previous" | "scene-next" | "bypass" | "bypass-muted" | "confirm" | "waveform";
export type QcCaptureFilterIconName = "Default" | "Amp" | "Combo Amp" | "Amp + Cab" | "Cab" | "Overdrive" | "Fuzz" | "Compressor";
export type QcEqIconName = "peak" | "high-pass" | "lo-pass" | "hi-shelf" | "lo-shelf";
export type QcHardwareIconName = "power-led" | "brand-pulse";
// `headphone-active` is gone: it traced the headphone jack of io-headphones,
// which is io.jack with its metal layer inked accent and nothing else. QcIoJack
// takes the state as a parameter, so the lit socket needs no second name.
export type QcIoIconName = "usb" | "jack" | "midi" | "combo" | "input" | "headphones-symbol";
export type QcLibraryIconName = "capture-library" | "capture-header" | "impulse-response" | "heart" | "clock" | "binoculars" | "broken-heart";
export type QcScreenHeaderGlyphName = "nav-undo" | "nav-save" | "export" | "menu";
export type QcLocalAppLogoName = "claude" | "chatgpt" | "antigravity" | "lm-studio" | "jan" | "other";
export type QcSettingsIconName = "connection" | "updates" | "brightness" | "power" | "volume" | "storage" | "factory-reset";
export type QcUiIconName = "add" | "create-new" | "plugin-presets" | "plugin-lock" | "subtract" | "previous" | "next" | "disclosure-right" | "arrow-right" | "cab-previous" | "cab-next" | "up" | "down" | "collapse" | "close-caret" | "more" | "check" | "close" | "refresh" | "backspace" | "file" | "document" | "capture-note" | "save-as" | "edit" | "menu-midi" | "favorite" | "delete" | "capture" | "modes" | "tempo" | "cpu" | "settings" | "power-toggle" | "phase-invert" | "pin";

export type QcScreenGlyphName =
  | "about" | "backup" | "system-brightness" | "account-cloud" | "contact" | "scene-copy"
  | "device" | "diagnostics" | "factory" | "gig-view" | "global-bypass"
  | "headphones" | "hold" | "info" | "latency" | "licenses" | "lock" | "device-midi" | "power-functions" | "report" | "scene-bypass"
  | "status" | "stomp-bypass" | "swap" | "system" | "system-updates" | "user"
  | "wifi" | "progress" | "shift"
  | "route-input" | "route-output" | "route-row" | "route-usb"
  | "capture-back" | "capture-next" | "capture-skip" | "capture-step-check" | "capture-step-next";
export type QcLooperActionGlyphName = "duplicate" | "one-shot" | "half-speed" | "punch-in" | "record" | "play" | "reverse" | "undo";
export type QcSynthWaveformName = "sine" | "triangle" | "saw" | "square" | "pulse" | "white-noise" | "pink-noise";

type ScreenVectorName = keyof typeof QC_SCREEN_ICON_VECTORS;

export type QcScreenVectorName = ScreenVectorName;

/**
 * `shapeRendering` for every glyph this registry draws, decided once.
 *
 * `pixelTrace` is for geometry read row by row off the unit's 800x480
 * framebuffer. Every edge in such a path already lies on a pixel boundary, so
 * `crispEdges` reproduces the unit exactly and antialiasing would soften edges
 * the unit paints hard. It is only correct at 1:1: scale the bezel away from
 * 800px wide and the same attribute staircases the same paths, which is why
 * this is one decision rather than an attribute repeated per glyph.
 *
 * `curveArt` is for geometry the renderer must antialias at any scale - arcs,
 * beziers, and any other smooth-edged contour. It is SVG's initial value, so
 * naming it changes nothing except making the choice legible beside
 * `pixelTrace`.
 *
 * A registry entry that needs `curveArt` says so in its own `rendering` field,
 * beside its geometry; nothing here should ever grow a per-icon exception.
 */
export const QC_ICON_SHAPE_RENDERING = { pixelTrace: "crispEdges", curveArt: "auto" } as const;
/**
 * Third-party marks for the local desktop AI apps QC Remote can pair with.
 * They identify the linked product only; they are not QC Remote or CorOS
 * iconography and never reach a device screen. Their use is covered by the
 * release trademark disclaimers: each mark belongs to its owner, and its
 * presence indicates compatibility, not affiliation or endorsement.
 *
 * Every mark here is drawn from the icon the installed app itself ships.
 * Two of them were not marks at all: `lm-studio`
 * and `jan` rendered a **text monogram** - `LM` and `J` - where a logo belongs.
 * `LM` was the worse of the two, because LM Studio is called Bionic now (see
 * LM_STUDIO_SUPPORTED_VERSIONS, where the client id deliberately does *not*
 * follow that rename because it is an identity): the tile showed the initials
 * of a name the row beside it no longer used. Bionic's own icon is four
 * staggered colour bars and Jan's is a waving hand, so both are drawn as what
 * they are.
 *
 * `chatgpt` keeps OpenAI's own geometry, restored to the precision it is
 * published at. It had been rounded to one decimal on a 24-unit box, and the
 * gaps that make that mark read as interlocking loops are about 0.3 units
 * wide - so a rounding error of up to 0.05 closed them. It rendered as merged
 * blobs with one orphaned speck: that is what a 1-dp copy of an interlocking
 * path looks like, not anything about the mark itself.
 *
 * Colour comes from the tile rules in the app sheet (`.logo-*`) except where a
 * mark is polychrome and one `currentColor` cannot carry it. Bionic's bars and
 * Antigravity's arch each own a gradient here, beside the geometry it paints,
 * with stops naming the same palette tokens the sheet uses. `useId` namespaces
 * those gradient ids so a second copy of a row on one document cannot capture
 * the first one's paint.
 *
 * The marks themselves - geometry, gradient stops, Bionic's bar layout
 * (QC_BIONIC_BARS) and Jan's character (JAN_WAVING_HAND, U+1F44B, the mark
 * Jan ships as its own app icon) - are data in
 * `device-reference/local-app-logos.ts`; this component only draws them.
 */
export function QcLocalAppLogo({ provider }: { provider: QcLocalAppLogoName }) {
  const gradientPrefix = useId();
  if (provider === "claude") return <span className="local-app-logo logo-claude" aria-hidden="true"><svg viewBox="0 0 24 24"><path d={QC_LOCAL_APP_LOGO_MARKS.claude.path} /></svg></span>;
  if (provider === "antigravity") {
    // One gradient across the arch's width. The mark runs a spectrum along its
    // own arc - blue at both feet, green and yellow up the shoulders, orange at
    // the apex - and an SVG gradient is a straight line, so a horizontal sweep
    // is the closest a single paint server gets. It lands because the figure is
    // symmetric about the apex: both feet reach blue at the same rate.
    const arch = `${gradientPrefix}-antigravity`;
    return <span className="local-app-logo logo-antigravity" aria-hidden="true"><svg viewBox="0 0 512 512">
      <defs><linearGradient id={arch} x1="0" y1="0" x2="1" y2="0">
        {QC_ANTIGRAVITY_GRADIENT_STOPS.map((stop) => <stop key={stop.offset} {...stop} />)}
      </linearGradient></defs>
      <path fill={`url(#${arch})`} d={QC_LOCAL_APP_LOGO_MARKS.antigravity.path} />
    </svg></span>;
  }
  if (provider === "lm-studio") {
    // Bionic's four bars: each bar 18 of 32 units long and 4 tall, alternating
    // a 3-unit indent so the stack staggers, each carrying its own
    // left-to-right gradient. Scaled by 24/32 that is 13.5 x 3 on a 3.75
    // pitch, which is what the geometry below states.
    return <span className="local-app-logo logo-lm-studio" aria-hidden="true"><svg viewBox="0 0 24 24">
      <defs>{QC_BIONIC_BARS.map((bar, index) => <linearGradient key={bar.from} id={`${gradientPrefix}-bionic-${index}`} x1="0" y1="0" x2="1" y2="0">
        <stop offset="0" stopColor={`var(--qc-palette-${bar.from})`} /><stop offset="1" stopColor={`var(--qc-palette-${bar.to})`} />
      </linearGradient>)}</defs>
      {QC_BIONIC_BARS.map((bar, index) => <rect key={bar.from} x={bar.x} y={bar.y} {...QC_BIONIC_BAR_SIZE} fill={`url(#${gradientPrefix}-bionic-${index})`} />)}
    </svg></span>;
  }
  // Jan ships the waving-hand emoji as its mark, so the tile sets the character
  // and lets the platform's own emoji font draw it. Tracing that artwork into a
  // path here would copy one vendor's emoji drawing rather than Jan's brand,
  // and would freeze that vendor's rendering into this registry.
  if (provider === "jan") return <span className="local-app-logo logo-jan" aria-hidden="true"><b>{JAN_WAVING_HAND}</b></span>;
  if (provider === "other") return <span className="local-app-logo logo-other" aria-hidden="true"><svg viewBox="0 0 24 24"><path d={QC_LOCAL_APP_LOGO_MARKS.other.path} /></svg></span>;
  return <span className="local-app-logo logo-chatgpt" aria-hidden="true"><svg viewBox="0 0 24 24"><path d={QC_LOCAL_APP_LOGO_MARKS.chatgpt.path} /></svg></span>;
}

function screenVectorColor(role: QcScreenIconLayerRole) {
  const token = role.slice("reference.".length) as keyof typeof QC_COLORS.reference;
  return QC_COLORS.reference[token];
}

function QcScreenVector({ icon, className }: { icon: ScreenVectorName; className?: string }) {
  const vector = QC_SCREEN_ICON_VECTORS[icon];
  const layer = vector.layers[0];
  return <svg className={className} width={vector.width} height={vector.height} viewBox={`0 0 ${vector.width} ${vector.height}`} shapeRendering={vectorShapeRendering(icon)} aria-hidden="true"><path d={layer.path} fill={screenVectorColor(layer.role)} stroke="none" /></svg>;
}

function QcScreenVectorLayers({ icon, className }: { icon: ScreenVectorName; className?: string }) {
  return <QcScreenVectorGlyph icon={icon} className={className} />;
}

/**
 * A soft-edged registry entry drawn in the caller's own colour.
 *
 * `QcScreenVectorGlyph` paints each layer in its own palette role, which is
 * right for a glyph the device paints one way everywhere. A
 * control that inherits its colour from its row - a pin the browser tints, a
 * back arrow on a blue button - needs the same geometry in `currentColor`
 * instead, and the paint has to sit on the `<path>`: several screen rules set
 * `fill: none; stroke: currentColor` on the `svg` for the stroked art these
 * outlines replace, and an inherited value loses to a presentation attribute on
 * the element itself. `shape-rendering` is `curveArt` for the same reason the
 * registry entry declares `rendering: "auto"` - `crispEdges` would snap a
 * sub-pixel contour back onto whole pixels. `viewBox` is for a call site whose
 * CSS box is tighter than the 24-unit asset box.
 */
function QcTracedMark({ icon, className, viewBox }: { icon: ScreenVectorName; className?: string; viewBox?: string }) {
  const vector = QC_SCREEN_ICON_VECTORS[icon];
  return <svg className={className} viewBox={viewBox ?? `0 0 ${vector.width} ${vector.height}`} shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">{vector.layers.map((layer) => <path key={layer.role} d={layer.path} fill="currentColor" stroke="none" />)}</svg>;
}

/**
 * The selector triangle, pointed the way the call site needs it.
 *
 * The unit draws one triangle and points it three ways, so the registry holds
 * one entry - `interface.down` - and this turns it. Both turns are exact
 * quarter and half rotations of hard-cornered geometry, so no orientation is a
 * second trace that could drift from the first; see
 * QC_SELECTOR_TRIANGLE_TRANSFORMS for the two transforms and for the storage
 * row's one-pixel crop offset.
 *
 * What does not rotate is the ink. `role` paints the palette role for that
 * call site - the category selector is reference.utilityMark on a black
 * header where the directory header is reference.primaryText - and omitting
 * it takes the caller's own colour, the way QcTracedMark does, which is what
 * a caret on a settings row that inherits its row's ink needs. Getting the
 * ink from the shape rather than from the site is what fixed the category
 * selector's colour once the role was separated out.
 */
function QcSelectorTriangle({ orientation, role, className, viewBox, size }: { orientation: QcSelectorTriangleOrientation; role?: QcScreenIconLayerRole; className?: string; viewBox?: string; size?: number }) {
  const layer = QC_SCREEN_ICON_VECTORS["interface.down"].layers[0];
  return <svg className={className} data-qc-icon={orientation} width={size} height={size} viewBox={viewBox ?? "0 0 24 24"} shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
    <path data-qc-role={role} d={layer.path} transform={QC_SELECTOR_TRIANGLE_TRANSFORMS[orientation]} fill={role ? screenVectorColor(role) : "currentColor"} stroke="none" />
  </svg>;
}

/**
 * How one registry entry wants to be rasterised.
 *
 * A pixel-row trace is a 1-bit mask of the unit's own framebuffer: every edge
 * already lies on a pixel boundary, so `crispEdges` reproduces it and
 * antialiasing would only soften edges the unit paints hard. A smooth-edged
 * entry is the opposite - its geometry is a soft-edged contour, and
 * `crispEdges` would snap that back onto whole pixels and lose it. Such an
 * entry declares `rendering: "auto"` itself, beside its geometry, so the two
 * kinds coexist while families migrate and this resolver never learns an
 * icon's name.
 */
function vectorShapeRendering(icon: ScreenVectorName) {
  return (QC_SCREEN_ICON_VECTORS[icon] as { rendering?: QcScreenIconRendering }).rendering ?? QC_ICON_SHAPE_RENDERING.pixelTrace;
}

export function QcScreenVectorGlyph({ icon, className, x, y, width, height, rungScale }: { icon: ScreenVectorName; className?: string; x?: number; y?: number; width?: number; height?: number; rungScale?: number }) {
  const vector = QC_SCREEN_ICON_VECTORS[icon];
  const shapeRendering = vectorShapeRendering(icon);
  return <svg className={className} data-qc-icon={icon.slice(icon.lastIndexOf(".") + 1)} x={x} y={y} width={width ?? vector.width} height={height ?? vector.height} viewBox={`0 0 ${vector.width} ${vector.height}`} shapeRendering={shapeRendering} aria-hidden="true">{vector.layers.map((layer) => <path key={layer.role} data-qc-role={layer.role} d={layer.path} fill={rungScale === undefined ? screenVectorColor(layer.role) : qcDimToRungs(screenVectorColor(layer.role), rungScale)} stroke="none" />)}</svg>;
}

/** The seven 32px oscillator marks the Overlord Synth editor draws. */
export function QcSynthWaveformGlyph({ kind, className }: { kind: QcSynthWaveformName; className?: string }) {
  const icon = `synth.wave-${kind}` as ScreenVectorName;
  const vector = QC_SCREEN_ICON_VECTORS[icon];
  return <svg className={className ?? "qc-synth-waveform"} width={vector.width} height={vector.height} viewBox={`0 0 ${vector.width} ${vector.height}`} shapeRendering={vectorShapeRendering(icon)} aria-hidden="true">{vector.layers.map((layer) => <path key={layer.role} d={layer.path} fill="currentColor" stroke="none" />)}</svg>;
}

/** Footer marks from the Synth's own screen. */
export function QcSynthFooterIcon({ kind }: { kind: "global-amp" | "filter" }) {
  if (kind === "filter") return <svg className="qc-synth-footer-icon" viewBox="0 0 34 24" aria-hidden="true"><path fill="currentColor" d={QC_SYNTH_FOOTER_PATHS.filter} /></svg>;
  return <svg className="qc-synth-footer-icon" viewBox="0 0 30 30" aria-hidden="true"><path fill="currentColor" d={QC_SYNTH_FOOTER_PATHS["global-amp"]} /></svg>;
}

/** Exact two-tone bracket/link used by every expression assignment target. */
export function QcExpressionAssignmentLink({ assigned = false }: { assigned?: boolean }) {
  return <svg className={`qc-expression-assignment-link${assigned ? " is-assigned" : ""}`} viewBox="0 0 24 24" shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace} aria-hidden="true">
    <path data-qc-role="brackets" d={QC_EXPRESSION_ASSIGNMENT_LINK_PATHS.brackets} />
    <path data-qc-role="link" d={QC_EXPRESSION_ASSIGNMENT_LINK_PATHS.link} />
  </svg>;
}

function screenVectorPath(icon: ScreenVectorName, index = 0) {
  return QC_SCREEN_ICON_VECTORS[icon].layers[index].path;
}

function directoryReferenceIcon(kind: QcDirectoryIconName): "directory.download" | "directory.cloud" | "directory.cloud-upload-header" | "directory.trash" | undefined {
  return kind === "download" ? "directory.download" : kind === "cloud" ? "directory.cloud" : kind === "cloud-upload" ? "directory.cloud-upload-header" : kind === "trash" ? "directory.trash" : undefined;
}

// `confirm` is not a mark of its own. The unit draws one tick, and the editor
// header, the directory header and the settings header all show it. It has
// one entry.
function editorScreenVector(kind: QcEditorIconName): ScreenVectorName | undefined {
  const icons = {
    change: "editor.change", paste: "editor.paste", reset: "editor.reset",
    save: "editor.save", expression: "editor.expression", mute: "editor.mute",
    "model-update": "editor.model-update", "model-downgrade": "editor.model-downgrade",
    remove: "editor.remove", confirm: "interface.check", bypass: "editor.bypass",
    "band-power": "editor.band-power", footswitch: "editor.footswitch", momentary: "editor.momentary",
    waveform: "editor.waveform"
  } as const;
  return kind in icons ? icons[kind as keyof typeof icons] : undefined;
}

/**
 * The scene shuttle, in the direction the caller needs.
 *
 * `editor.scene-next` is the drawing and `editor.scene-previous` is no longer an
 * entry: turned over about its box centre, next's two arrowheads land exactly
 * on previous's, so the two names are one drawing at two orientations. Both
 * layers keep their own palette role - the inner arrowhead reference.iconPrimary,
 * the outer one reference.sceneControlMuted - because the unit draws the
 * shuttle's two halves in two inks, and the rotation moves them without
 * swapping them.
 */
function QcSceneShuttle({ direction }: { direction: "next" | "previous" }) {
  const vector = QC_SCREEN_ICON_VECTORS["editor.scene-next"];
  return <svg data-qc-icon={`scene-${direction}`} width={vector.width} height={vector.height} viewBox={`0 0 ${vector.width} ${vector.height}`} shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
    {vector.layers.map((layer) => <path key={layer.role} data-qc-role={layer.role} d={layer.path} transform={direction === "previous" ? QC_SCENE_SHUTTLE_REVERSE : undefined} fill={screenVectorColor(layer.role)} stroke="none" />)}
  </svg>;
}

export function QcIoIcon({ kind, className }: { kind: QcIoIconName; className?: string }) {
  return <QcScreenVectorLayers icon={`io.${kind}` as ScreenVectorName} className={className} />;
}

export function QcEqIcon({ kind, className }: { kind: QcEqIconName; className?: string }) {
  return <QcScreenVectorLayers icon={`eq.${kind}` as ScreenVectorName} className={`qc-eq-icon qc-eq-icon-${kind}${className ? ` ${className}` : ""}`} />;
}

/*
 * The same control drawn at icon size - the Master Volume Knob row's mark on
 * the system settings list.
 *
 * It shares this file because it shares the sweep, and nothing else:
 *
 *   centre         11.72, 11.72     not 12 - the art sits flush to the box's
 *                                   top left
 *   face fill      r <= 7.78
 *   value scale    r 9.75 .. 11.75  exactly 2.00 wide
 *   pointer        2.00 wide, radial, from r 1.05 out through the rim
 *
 * The sweep is the dial's own: the scale spans QC_ROTARY_START's 135 degrees
 * through to 314 degrees, and the pointer's axis sits at 315.0, which is
 * 135 + 2/3 x 270.
 *
 * What it does *not* share is the radial construction, which is why this is not
 * QcRotaryDial with a flag. Scaled to that dial's 72-unit box the proportions
 * differ throughout: the black rim is absorbed into the face, and the pointer
 * is nearly twice as wide in proportion. The polarity is inverted outright: the
 * dial is a dark face carrying a light pointer, and this is a light face with
 * the pointer cut out of it.
 *
 * That cut is not a dark stroke: the pointer's pixels are whatever the row
 * underneath is - the list's own ground normally, the selected-row ground
 * when that row is highlighted, and a dialog's own scrim colour under an
 * overlay - the unit substituting surfaces rather than blending. So the
 * pointer is a hole and the ground shows through it on any row; painting it
 * would need this component to know a background it has no business knowing.
 *
 * Two rendering choices decide how the rest is drawn:
 *
 * The face is a `<circle>` behind a clip, not an outline with the pointer taken
 * out of it: a circle rasterises more cleanly in Chrome than an arc built from
 * one long `A` command. Clipping the pointer away also matches the unit better
 * than cutting it out of the fill, because the unit's own edge there is the
 * two-step composite a clip makes, and not the single geometric edge a hole in
 * a fill makes. A `<mask>` composites the same way, but a luminance mask has to
 * name a black and a white, and a registry source carries palette roles and no
 * colour literals at all.
 *
 * The scale is split into segments of at most 90 degrees. At this instance's
 * own value the sweep is exactly 180, and a 180 degree arc has no determined
 * centre - the chord is the diameter, so SVG's radius correction fires on any
 * rounding and walks the whole arc. Splitting it avoids that.
 *
 * Nothing here sets shapeRendering, for the reason the dial gives above.
 */
const KNOB_CENTRE = QC_SETTINGS_KNOB_GEOMETRY.centre;
const KNOB_FACE_RADIUS = QC_SETTINGS_KNOB_GEOMETRY.faceRadius;
const KNOB_SCALE_INNER = QC_SETTINGS_KNOB_GEOMETRY.scaleInner;
const KNOB_SCALE_OUTER = QC_SETTINGS_KNOB_GEOMETRY.scaleOuter;
const KNOB_SCALE_WIDTH = KNOB_SCALE_OUTER - KNOB_SCALE_INNER;
const KNOB_SCALE_RADIUS = (KNOB_SCALE_INNER + KNOB_SCALE_OUTER) / 2;
const KNOB_POINTER_WIDTH = QC_SETTINGS_KNOB_GEOMETRY.pointerWidth;
const KNOB_POINTER_INNER = QC_SETTINGS_KNOB_GEOMETRY.pointerInner;

/** The pointer runs out past the rim; the face it is cut from is what ends it. */
const KNOB_POINTER_OUTER = KNOB_FACE_RADIUS + 1;

const knobRound = (value: number) => Number(value.toFixed(3));

function knobPoint(radius: number, degrees: number) {
  const radians = (degrees * Math.PI) / 180;
  return `${knobRound(KNOB_CENTRE + radius * Math.cos(radians))} ${knobRound(KNOB_CENTRE + radius * Math.sin(radians))}`;
}

/** The scale, from the sweep's own minimum round to the pointer. */
function knobScalePath(degrees: number) {
  const swept = degrees - QC_ROTARY_START;
  const segments = Math.max(2, Math.ceil(Math.abs(swept) / 90));
  let path = `M${knobPoint(KNOB_SCALE_RADIUS, QC_ROTARY_START)}`;
  for (let segment = 1; segment <= segments; segment += 1) {
    path += `A${KNOB_SCALE_RADIUS} ${KNOB_SCALE_RADIUS} 0 0 1 `
      + knobPoint(KNOB_SCALE_RADIUS, QC_ROTARY_START + (swept * segment) / segments);
  }
  return path;
}

/**
 * The clip that carries the pointer: the whole box, with the pointer's bar
 * subtracted from it by the even-odd rule. Intersected with the face it is
 * applied to, that is the face minus the pointer - and the bar may safely run
 * past the rim, because the face is what ends it.
 */
function knobPointerClipPath(degrees: number) {
  const radians = (degrees * Math.PI) / 180;
  const across = [-Math.sin(radians), Math.cos(radians)] as const;
  const half = KNOB_POINTER_WIDTH / 2;
  const corner = (side: number, radius: number) => {
    const centre = knobPoint(radius, degrees).split(" ").map(Number);
    return `${knobRound(centre[0] + side * half * across[0])} ${knobRound(centre[1] + side * half * across[1])}`;
  };
  return `M0 0H24V24H0Z`
    + `M${corner(1, KNOB_POINTER_INNER)}L${corner(1, KNOB_POINTER_OUTER)}`
    + `L${corner(-1, KNOB_POINTER_OUTER)}L${corner(-1, KNOB_POINTER_INNER)}Z`;
}

export type QcKnobGlyphProps = {
  className?: string;
  /** Position along the unit's 270 degree sweep, 0..1. Drives pointer and scale together. */
  value?: number;
  /** The same state spelled as a pointer angle in degrees clockwise from +x. */
  angle?: number;
  /** Rendered edge length in px. Defaults to the asset's own 24. */
  size?: number;
  /** The knob body. */
  face?: string;
  /** The swept scale. */
  scale?: string;
};

/**
 * The settings list's volume knob, at any value and in any pair of inks.
 *
 * Colours are presentation attributes on the marks themselves, not inherited:
 * several screen rules set `fill`/`stroke` on the `svg` for the stroked art
 * these outlines sit beside, and an inherited value loses to an attribute on
 * the element.
 */
export function QcKnobGlyph({
  className,
  value,
  angle,
  size,
  face = QC_COLORS.reference.iconPrimary,
  scale = QC_COLORS.reference.looperRing
}: QcKnobGlyphProps) {
  const position = value !== undefined
    ? Math.max(0, Math.min(1, value))
    : angle !== undefined ? qcRotaryValue(angle) : 0;
  const pointer = qcRotaryAngle(position);
  const edge = size ?? 24;
  // Two of these can share a document, at different values, so the clip cannot
  // carry a fixed id. React's is unique per instance; the sanitising is because
  // its punctuation has changed across majors and `url(#...)` has to parse.
  const cut = `qc-knob-cut-${useId().replace(/[^A-Za-z0-9_-]/g, "")}`;
  return <svg
    className={className}
    data-qc-icon="volume"
    width={edge}
    height={edge}
    viewBox="0 0 24 24"
    aria-hidden="true"
  >
    <clipPath id={cut} clipPathUnits="userSpaceOnUse">
      <path clipRule="evenodd" d={knobPointerClipPath(pointer)} />
    </clipPath>
    {position > 0 && <path
      d={knobScalePath(pointer)}
      fill="none"
      stroke={scale}
      strokeWidth={KNOB_SCALE_WIDTH}
      strokeLinecap="butt"
    />}
    <circle cx={KNOB_CENTRE} cy={KNOB_CENTRE} r={KNOB_FACE_RADIUS} fill={face} stroke="none" clipPath={`url(#${cut})`} />
  </svg>;
}

/**
 * Where the settings list's volume knob sits, wherever it draws it.
 *
 * The unit's own reading, not a placeholder: the pointer's axis is 315.0
 * degrees and the sweep starts at 135, so this is exactly two thirds along.
 * The unit draws this mark at a fixed value rather than tracking the actual
 * master volume, so it is pinned here rather than threaded through as a
 * variable.
 */
const QC_SETTINGS_VOLUME_KNOB_VALUE = QC_SETTINGS_KNOB_GEOMETRY.value;

/*
 * The Device Storage row's mark.
 *
 * It is a pie with one slice pulled out of it, drawn analytically rather than
 * as a pixel mask:
 *
 *   centre        12, 12          the asset box's own centre
 *   radius        12              the disc fills the 24-unit box exactly, and
 *                                 the panel clips it at all four edges
 *   slice radius  9.2             the pulled slice is drawn smaller than the pie
 *   slice offset  2 * sqrt(2)     along the slice's own bisector, which at a
 *                                 quarter is exactly two units in each axis
 *   slice sweep   90 degrees      one quarter, see the pin below
 *
 * The arc is split into 90 degree segments for the same reason QcKnobGlyph's is:
 * a half-turn arc has no determined centre, and a renderer is free to pick either.
 */
const QC_STORAGE_CENTRE = QC_SETTINGS_STORAGE_GEOMETRY.centre;
const QC_STORAGE_RADIUS = QC_SETTINGS_STORAGE_GEOMETRY.radius;
const QC_STORAGE_SLICE_RADIUS = QC_SETTINGS_STORAGE_GEOMETRY.sliceRadius;
const QC_STORAGE_SLICE_OFFSET = QC_SETTINGS_STORAGE_GEOMETRY.sliceOffset;

const storageRound = (value: number) => Number(value.toFixed(3));

function storagePoint(cx: number, cy: number, radius: number, degrees: number) {
  const radians = (degrees * Math.PI) / 180;
  return `${storageRound(cx + radius * Math.cos(radians))} ${storageRound(cy - radius * Math.sin(radians))}`;
}

/** A filled sector: centre, out to the start angle, round the arc, closed. */
function storageSector(cx: number, cy: number, radius: number, start: number, sweep: number) {
  const segments = Math.max(1, Math.ceil(sweep / 90));
  let path = `M${storageRound(cx)} ${storageRound(cy)}L${storagePoint(cx, cy, radius, start)}`;
  for (let segment = 1; segment <= segments; segment += 1) {
    path += `A${storageRound(radius)} ${storageRound(radius)} 0 0 0 `
      + storagePoint(cx, cy, radius, start + (sweep * segment) / segments);
  }
  return `${path}Z`;
}

export type QcStorageGlyphProps = {
  className?: string;
  /** Share of the pie the pulled-out slice takes, 0..1. Drives both marks together. */
  value?: number;
  /** Rendered edge length in px. Defaults to the asset's own 24. */
  size?: number;
  /** The capacity still free: the pie the slice was cut from. */
  free?: string;
  /** The capacity in use: the slice pulled out of it. */
  used?: string;
};

/**
 * The settings list's Device Storage mark, at any share and in any pair of inks.
 *
 * Colours are presentation attributes on the marks themselves, not inherited:
 * several screen rules set `fill`/`stroke` on the `svg` for the stroked art
 * these sectors sit beside, and an inherited value loses to an attribute on the
 * element.
 */
export function QcStorageGlyph({
  className,
  value = 0,
  size,
  free = QC_COLORS.reference.iconPrimary,
  used = QC_COLORS.reference.looperRing
}: QcStorageGlyphProps) {
  const share = Math.max(0, Math.min(1, value));
  const sweep = 360 * share;
  const bisector = ((sweep / 2) * Math.PI) / 180;
  const sliceX = QC_STORAGE_CENTRE + QC_STORAGE_SLICE_OFFSET * Math.cos(bisector);
  const sliceY = QC_STORAGE_CENTRE - QC_STORAGE_SLICE_OFFSET * Math.sin(bisector);
  const edge = size ?? 24;
  return <svg className={className} data-qc-icon="storage" width={edge} height={edge} viewBox="0 0 24 24" aria-hidden="true">
    {share < 1 && <path
      d={storageSector(QC_STORAGE_CENTRE, QC_STORAGE_CENTRE, QC_STORAGE_RADIUS, sweep, 360 - sweep)}
      fill={free}
      stroke="none"
    />}
    {share > 0 && <path
      d={storageSector(sliceX, sliceY, QC_STORAGE_SLICE_RADIUS, 0, sweep)}
      fill={used}
      stroke="none"
    />}
  </svg>;
}

/**
 * How full the settings list draws the Device Storage pie, wherever it draws
 * it.
 *
 * The unit's own reading: the slice's two radial cuts land on x = 14.00 and
 * y = 10.00 exactly, one quarter of the turn. The unit draws this mark at a
 * fixed share rather than tracking real free space, so it is pinned here
 * rather than threaded through as a variable.
 */
const QC_SETTINGS_STORAGE_VALUE = QC_SETTINGS_STORAGE_GEOMETRY.value;

// `volume` and `storage` are not registry paths, and cannot be: the Master
// Volume Knob's mark is a rotary control and the Device Storage mark is a pie,
// so each one's geometry is a function of the value it is showing rather than a
// fixed outline. They route to the components that draw those controls the way
// `QcUiIcon` already routes its own kinds to dedicated ones. Every other row
// here is a fixed mark and stays in the registry.
export function QcSettingsIcon({ kind, className }: { kind: QcSettingsIconName; className?: string }) {
  if (kind === "volume") return <QcKnobGlyph className={className} value={QC_SETTINGS_VOLUME_KNOB_VALUE} />;
  if (kind === "storage") return <QcStorageGlyph className={className} value={QC_SETTINGS_STORAGE_VALUE} />;
  return <QcScreenVectorLayers icon={`settings.${kind}` as ScreenVectorName} className={className} />;
}

export function QcCaptureFilterIcon({ kind, className }: { kind: QcCaptureFilterIconName; className?: string }) {
  const icons: Record<QcCaptureFilterIconName, ScreenVectorName> = {
    Default: "capture-filter.default",
    Amp: "capture-filter.amp",
    "Combo Amp": "capture-filter.combo-amp",
    "Amp + Cab": "capture-filter.amp-cab",
    Cab: "capture-filter.cab",
    Overdrive: "capture-filter.overdrive",
    Fuzz: "capture-filter.fuzz",
    Compressor: "capture-filter.compressor"
  };
  return <QcScreenVectorLayers icon={icons[kind]} className={className ?? "qc-capture-filter-icon"} />;
}

export function QcPresetStackIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true">
    <path d={QC_PRESET_STACK_PATHS.top} fill="currentColor" />
    <path d={QC_PRESET_STACK_PATHS.layers} fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" />
  </svg>;
}

/** Shared app/CorOS glyph vocabulary. Text characters must not be used as icons. */
export function QcUiIcon({ kind, className }: { kind: QcUiIconName; className?: string }) {
  const classes = `qc-ui-icon qc-ui-icon-${kind}${className ? ` ${className}` : ""}`;
  // The device's power symbol is one mark: this reuses editor.bypass rather
  // than a second drawing of the same ring and stem.
  if (kind === "power-toggle") return <QcTracedMark icon="editor.bypass" className={classes} />;
  if (kind === "phase-invert") return <QcTracedMark icon="interface.phase-invert" className={classes} />;
  if (kind === "pin") return <QcTracedMark icon="interface.pin" className={classes} />;
  if (kind === "plugin-lock") return <QcTracedMark icon="interface.plugin-lock" className={classes} />;
  if (kind === "arrow-right") return <QcTracedMark icon="interface.arrow-right" className={classes} />;
  // The unit's disclosure mark is 9x14 of ink inside a 24-unit asset box; the
  // storage row's CSS box is tighter than that box, so this window keeps the
  // element close to the size the row already reserves while the geometry stays
  // in the registry's own box. official-settings-device.css carries the
  // matching 1.25cqw x 1.875cqw, which renders the ink at its device size.
  if (kind === "disclosure-right") return <QcSelectorTriangle orientation="right" className={classes} viewBox="6 5 10 15" />;
  // The device draws the directory disclosure as the filled triangle
  // interface.down owns, not as a chevron; one entry, both names. It is the
  // *closed* selector; the open state is the same drawing turned over, which
  // is why it is an orientation below and not an entry.
  if (kind === "collapse") return <QcTracedMark icon="interface.down" className={classes} />;
  // The looper caption's caret is not that triangle at either orientation: the
  // device draws a hollow chevron where both of those are solid. It is a
  // screen mark with its own role, so it resolves that role here rather than
  // taking the caption's ink.
  if (kind === "close-caret") return <QcScreenVectorLayers icon="looper.close-caret" className={classes} />;
  if (kind === "previous") return <QcTracedMark icon="interface.previous" className={classes} />;
  if (kind === "more") return <QcTracedMark icon="interface.more" className={classes} />;
  // The up selector is the same triangle turned over, and it is a screen mark
  // with its own role, so it names that role here rather than taking the
  // caller's ink - the down-pointing triangle the directory header draws uses
  // a different ink, so the orientation has to be the parameter, and the ink
  // has to stay here. Its three app-owned callers - the chassis bank arrow,
  // the IR loader's step arrow and the looper's close caret - are chrome that
  // inherits its row's ink, and each pins `path { fill: currentColor }` in its
  // own sheet, the way .coros-physical-confirmation already re-inks
  // interface.add.
  if (kind === "up") return <QcSelectorTriangle orientation="up" role="reference.utilityMark" className={classes} size={24} />;
  if (kind === "settings" || kind === "check" || kind === "down" || kind === "refresh") return <QcScreenVectorLayers icon={`interface.${kind}` as ScreenVectorName} className={classes} />;
  const referenceIcons = {
    add: "interface.add", "create-new": "interface.create-new", "plugin-presets": "interface.plugin-presets", file: "interface.file", document: "interface.document", "capture-note": "interface.capture-note", "menu-midi": "interface.midi", modes: "interface.modes",
    tempo: "interface.tempo", close: "interface.close", backspace: "interface.backspace"
  } as const;
  if (kind in referenceIcons) return <QcScreenVector icon={referenceIcons[kind as keyof typeof referenceIcons]} className={classes} />;
  if (kind === "cab-previous" || kind === "cab-next") {
    const icon = kind === "cab-previous" ? "interface.previous-cab" : "interface.next-cab";
    return <svg className={classes} viewBox="0 0 24 24" shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace} aria-hidden="true"><path d={screenVectorPath(icon, 0)} style={{ fill: QC_COLORS.reference.cabArrowDark, stroke: "none" }} /><path d={screenVectorPath(icon, 1)} style={{ fill: QC_COLORS.reference.cabArrowLight, stroke: "none" }} /></svg>;
  }
  if (kind === "favorite") return <QcScreenVectorLayers icon="interface.favorite" className={classes} />;
  if (kind === "save-as") return <QcScreenVectorLayers icon="interface.save-as" className={classes} />;
  if (kind === "edit") return <QcScreenVectorLayers icon="interface.edit" className={classes} />;
  // grid-context-menu and block-context crop the same swap arrows one pixel
  // apart, so this is editor.change read through a box shifted one unit left
  // rather than a second copy of the geometry. The device's ink core is a solid
  // 22x20 pair of arrows; the muted ink is only the ten pixels of their tips.
  if (kind === "next")
    return (
      <svg className={classes} viewBox="1 0 24 24" shapeRendering={vectorShapeRendering("editor.change")} aria-hidden="true">
        <path fill={QC_COLORS.reference.primaryText} stroke="none" d={screenVectorPath("editor.change", 0)} />
        <path fill={QC_COLORS.reference.editorMuted} stroke="none" d={screenVectorPath("editor.change", 1)} />
      </svg>
    );
  if (kind === "delete") return <QcScreenVectorLayers icon="interface.delete" className={classes} />;
  // The context menu's Neural Capture mark, rasterised against the menu's
  // darker ground: three of its seven ink partitions match the Captures
  // header's exactly, so those three are drawn from that entry rather than
  // copied. The other four differ because the darkest stripe reads differently
  // against a darker ground, which is why this is not simply the same entry
  // in another palette.
  if (kind === "capture")
    return (
      <svg className={classes} viewBox="0 0 24 24" shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace} aria-hidden="true">
        <path fill={QC_COLORS.reference.primaryText} stroke="none" d={screenVectorPath("library.capture-header", 6)} />
        <path fill={QC_COLORS.reference.contextCaptureLight} stroke="none" d={screenVectorPath("library.capture-header", 5)} />
        <path fill={QC_COLORS.reference.contextCaptureSoft} stroke="none" d={screenVectorPath("library.capture-header", 4)} />
        <path fill={QC_COLORS.reference.contextCaptureMuted} stroke="none" d={QC_CONTEXT_CAPTURE_PATHS.muted} />
        <path fill={QC_COLORS.reference.contextCaptureMid} stroke="none" d={QC_CONTEXT_CAPTURE_PATHS.mid} />
        <path fill={QC_COLORS.reference.contextCaptureDark} stroke="none" d={QC_CONTEXT_CAPTURE_PATHS.dark} />
        <path fill={QC_COLORS.reference.contextCaptureBlack} stroke="none" d={QC_CONTEXT_CAPTURE_PATHS.black} />
      </svg>
    );
  if (kind === "cpu")
    return (
      <svg className={classes} viewBox="0 0 24 24" shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace} aria-hidden="true">
        <path fill={QC_COLORS.reference.primaryText} stroke="none" d={QC_CPU_GLYPH.path} />
      </svg>
    );
  if (kind === "subtract")
    return (
      <svg className={classes} viewBox="0 0 24 24" aria-hidden="true">
        <path d="M4 12h16" />
      </svg>
    );
  return null;
}

export function QcHardwareIcon({ kind, className }: { kind: QcHardwareIconName; className?: string }) {
  if (kind === "power-led")
    return (
      <svg className={className} viewBox="3 2 18 20" aria-hidden="true">
        <path d="M12 3v8M7.3 6.4a7.5 7.5 0 1 0 9.4 0" fill="none" stroke={QC_COLORS.hardware.whiteLed} />
      </svg>
    );
  return (
    <svg className={className} viewBox="0 0 16 16" aria-hidden="true">
      <path d="M9 1 3.5 8H7l-1 7 6.5-8H9z" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function QcScreenHeaderGlyph({ kind }: { kind: QcScreenHeaderGlyphName }) {
  // Each of these is positioned in the 800x480 screen frame, so the entry holds
  // the mark in its own device asset box and the translate places it.
  if (kind === "nav-undo") return <path shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} fill={QC_COLORS.reference.headerUndo} d={screenVectorPath("screen-header.undo")} transform="translate(604 8)" />;
  if (kind === "nav-save") return <path shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace} fill={QC_COLORS.reference.primaryText} d={QC_SCREEN_HEADER_SAVE_GLYPH.path} />;
  // `export` is not a mark of its own. The floppy-with-arrow is the same
  // drawing the unit shows in the main-menu popup, and `interface.save-as`
  // already owns it. So the header draws that entry rather than a second
  // copy of the same drawing.
  if (kind === "export") return <path shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} fill={QC_COLORS.reference.headerSave} d={screenVectorPath("interface.save-as")} transform="translate(704 12)" />;
  // `menu` is not a mark of its own either. The navigation bar's three dots are
  // interface.more; both draw the same three circles on the same pitch, and
  // only the offset differs, not the drawing. So the translate is 752,11
  // rather than 757,10 - and the ink stays this row's own reference.headerMenu
  // rather than the entry's reference.iconPrimary.
  return <path shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} fill={QC_COLORS.reference.headerMenu} d={screenVectorPath("interface.more")} transform="translate(752 11)" />;
}

/** A complete, reusable viewport for navigation-bar artwork. */
export function QcNavigationBarIcon({ kind, className }: { kind: QcScreenHeaderGlyphName; className?: string }) {
  const viewBox = kind === "nav-undo" ? "600 0 48 48" : kind === "menu" ? "748 0 48 48" : "695 0 48 48";
  return <svg className={`qc-navigation-bar-icon qc-navigation-bar-icon-${kind}${className ? ` ${className}` : ""}`} viewBox={viewBox} aria-hidden="true"><QcScreenHeaderGlyph kind={kind} /></svg>;
}

const ROUTE_STEREO_OUTPUT_VECTOR = QC_ROUTE_GLYPH_GEOMETRY.stereoOutput.path;

/** Shared CorOS routing glyph vocabulary used by both app hosts. */
export function QcRouteGlyph({ side, label }: { side: "input" | "output"; label: string }) {
  // The device marks an unassigned route with a filled plate and a muted plus,
  // not a stroked outline: drawn as an outline it inherits the row's own text
  // colour and reads as a bright white ring the unit never shows.
  if (label === "Not In Use")
    return (
      <svg className="route-plus-plate" viewBox="0 0 32 32" shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
        <path d={screenVectorPath("route.internal", 0)} fill={QC_COLORS.reference.routeGlyphSurface} stroke="none" />
        <path d={screenVectorPath("route.internal", 1)} fill={QC_COLORS.reference.utilityMark} stroke="none" />
      </svg>
    );
  if (side === "input" && /^(?:In|Input) \d+$/.test(label))
    return (
      <svg viewBox="0 0 32 32" shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
        <path d={screenVectorPath("route.analog-input", 0)} fill={QC_COLORS.reference.routeGlyphSurface} stroke="none" />
        <path d={screenVectorPath("route.analog-input", 1)} fill={QC_COLORS.reference.primaryText} stroke="none" />
      </svg>
    );
  if (label === "Internal")
    return (
      <svg className="route-internal-glyph" viewBox="0 0 32 32" shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
        <path d={screenVectorPath("route.internal", 0)} fill={QC_COLORS.reference.routeGlyphSurface} stroke="none" />
        <path d={screenVectorPath("route.internal", 1)} fill={QC_COLORS.reference.utilityMark} stroke="none" />
      </svg>
    );
  if (label.startsWith("USB "))
    return (
      <svg viewBox="0 0 32 32" shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
        <path d={QC_ROUTE_GLYPH_GEOMETRY.usb.mark} fill={QC_COLORS.reference.utilityMark} stroke="none" />
        <path d={QC_ROUTE_GLYPH_GEOMETRY.usb.ink} fill={QC_COLORS.reference.primaryText} stroke="none" />
      </svg>
    );
  if (label.startsWith("Return "))
    return (
      <svg viewBox="0 0 32 32" shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
        <path d={QC_ROUTE_GLYPH_GEOMETRY.return.surface} fill={QC_COLORS.reference.routeGlyphSurface} stroke="none" />
        <path d={QC_ROUTE_GLYPH_GEOMETRY.return.ink} fill={QC_COLORS.reference.primaryText} stroke="none" />
      </svg>
    );
  if (label === "Multi Out" || label === "Multiple Outputs")
    return (
      <svg viewBox="0 0 32 32" shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
        <path d={ROUTE_STEREO_OUTPUT_VECTOR} fill={QC_COLORS.reference.primaryText} stroke="none" />
      </svg>
    );
  if (label.startsWith("Send "))
    return (
      <svg viewBox="0 0 32 32" shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
        <path d={screenVectorPath("route.send", 0)} fill={QC_COLORS.reference.routeGlyphSurface} stroke="none" />
        <path d={screenVectorPath("route.send", 1)} fill={QC_COLORS.reference.primaryText} stroke="none" />
      </svg>
    );
  if (label === "Out 1/2")
    return (
      <svg viewBox="0 0 32 32" shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
        <path d={ROUTE_STEREO_OUTPUT_VECTOR} fill={QC_COLORS.reference.primaryText} stroke="none" />
      </svg>
    );
  if (label === "Out 1" || label === "Out 2")
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <circle {...QC_ROUTE_GLYPH_GEOMETRY.xlrOutput.body} />
        <circle {...QC_ROUTE_GLYPH_GEOMETRY.xlrOutput.pinTop} />
        <circle {...QC_ROUTE_GLYPH_GEOMETRY.xlrOutput.pinLeft} />
        <circle {...QC_ROUTE_GLYPH_GEOMETRY.xlrOutput.pinRight} />
      </svg>
    );
  // Row 3/4 is a paired grid-row destination, not a stereo hardware output.
  // Keep row destinations ahead of the generic slash-delimited route case so
  // every Row entry uses the same reference return-path glyph.
  if (label.startsWith("Row "))
    return (
      <svg viewBox="0 0 32 32" shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
        <path d={QC_ROUTE_GLYPH_GEOMETRY.row.path} fill={QC_COLORS.reference.primaryText} stroke="none" />
      </svg>
    );
  if (label.includes("/"))
    return (
      <svg viewBox="0 0 32 32" shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
        <path d={ROUTE_STEREO_OUTPUT_VECTOR} fill={QC_COLORS.reference.primaryText} stroke="none" />
      </svg>
    );
  if (label.startsWith("Out "))
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path d={QC_ROUTE_GLYPH_GEOMETRY.output.jack} />
        <path d={QC_ROUTE_GLYPH_GEOMETRY.output.arrow} />
      </svg>
    );
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <circle {...QC_ROUTE_GLYPH_GEOMETRY.fallback.jack} />
      <path d={side === "input" ? QC_ROUTE_GLYPH_GEOMETRY.fallback.input : QC_ROUTE_GLYPH_GEOMETRY.fallback.output} />
    </svg>
  );
}

export function QcModeGlyph({ mode }: { mode: PresetSnapshot["mode"] }) {
  const icon = `mode.${mode.toLowerCase()}` as ScreenVectorName;
  const vector = QC_SCREEN_ICON_VECTORS[icon];
  // HYBRID is a composition, not a drawing: the device shows a preset box, a
  // join, and a scene box side by side within one 74x26 mode box. The two mode
  // marks are delegated to their own entries and only the join is authored
  // here. Drawing them as nested <svg> keeps one art unit on one device pixel.
  if (mode === "HYBRID") {
    const join = vector.layers[0];
    return <svg width={vector.width} height={vector.height} viewBox={`0 0 ${vector.width} ${vector.height}`} className="qc-mode-glyph qc-mode-glyph-hybrid" aria-hidden="true">
      <QcScreenVectorGlyph icon="mode.preset" x={1} y={0} />
      <path d={join.path} fill={screenVectorColor(join.role)} stroke="none" shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace} />
      <QcScreenVectorGlyph icon="mode.scene" x={48} y={0} />
    </svg>;
  }
  // Sized from the vector, not a fixed 24: scene and stomp are 26 units and
  // hybrid is 74x26, so a hardcoded 24 squeezed crispEdges pixel art onto a
  // fractional scale wherever the glyph is placed inside the SVG canvas. The
  // rasterisation comes from the entry too: scene and stomp are soft-edged
  // contours now, and crispEdges would snap them back onto whole pixels.
  return <svg width={vector.width} height={vector.height} viewBox={`0 0 ${vector.width} ${vector.height}`} className={`qc-mode-glyph qc-mode-glyph-${mode.toLowerCase()}`} shapeRendering={vectorShapeRendering(icon)} aria-hidden="true">
    {vector.layers.map((layer) => <path key={layer.role} d={layer.path} fill={screenVectorColor(layer.role)} stroke="none" />)}
  </svg>;
}

export function QcDirectoryIcon({ kind, number }: { kind: QcDirectoryIconName; number?: number }) {
  const classes = `qc-directory-icon qc-directory-icon-${kind}`;
  const referenceIcon = directoryReferenceIcon(kind);
  // Every layer, not just the first: the bin carries its cross in a second ink.
  if (referenceIcon) return <QcScreenVectorLayers icon={referenceIcon} className={classes} />;
  // The Presets category mark and the PRESET mode glyph are the same device
  // drawing, pixel for pixel. The rounded rects drew their first and last rows
  // a unit in from the device's, so the mark measured 23x22 where the device
  // is 24x24.
  if (kind === "grid")
    return (
      <svg className={classes} viewBox="0 0 24 24" shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace} aria-hidden="true">
        {/* Fills itself in the row colour. Leaving the fill to the stylesheet
            worked on the directory screens and made the mark vanish on
            settings-storage, where `.storage-captured span svg` sets
            `fill: none; stroke: currentColor` for the stroked icons around it -
            and this path's own `stroke="none"` beat that stroke, leaving
            nothing drawn at all. */}
        <path fill="currentColor" stroke="none" d={screenVectorPath("mode.preset")} />
      </svg>
    );
  // The unit draws a solid folder with its number knocked out of the fill, not
  // a number set beside a tab: the same silhouette repeats at every folder
  // row. The digit stays text because the number varies; only the silhouette
  // is the glyph, so only it is registered. The icon box's rows start at
  // y=60 on a 60px pitch, and the box is row + 16, which three assets that ink
  // all 24 of their rows pin exactly. The silhouette therefore begins one unit
  // down its box, and the digit rides with it.
  if (kind === "folder")
    return (
      <svg className={classes} viewBox="0 0 24 24" shapeRendering={QC_ICON_SHAPE_RENDERING.curveArt} aria-hidden="true">
        {/* Carries its own ink, like the `grid` branch above and for the same
            reason: left bare, the path took the `stroke: currentColor` the
            Directory navs set on the svg, and half a stroke-width outside a
            soft-edged contour is a pixel of false ink on every side. The
            `fill` has to come with the `stroke="none"`, or the mark vanishes
            wherever a sheet sets `fill: none` on its svgs.
            The unit draws two folders: FolderDirTable's solid silhouette, which
            is this art, and FolderTable's hollow two-unit outline on the nested,
            filter, captures, IR and plugin lists. That one is `folder-outline`
            below - its own trace, not this contour stroked. */}
        <path d={screenVectorPath("directory.folder")} fill="currentColor" stroke="none" />
        {number !== undefined && (
          <text className="folder-number" x="12" y="15" textAnchor="middle" fontFamily={QC_TYPOGRAPHY.devicePlain} fontSize={number === 0 ? "8" : "9"} fontWeight="700" transform={number === 0 ? "translate(0 -2.8) scale(1 1.4)" : undefined}>
            {number}
          </text>
        )}
      </svg>
    );
  // Both folders below sit in the shared neutral 24px glyph area, the same
  // 24-unit box every other entry in this registry uses, so neither carries a
  // box of its own at a call site.
  //
  // This folder is not the solid silhouette above with a stroke on it: the
  // unit draws its own two-unit outline, which stroking the solid contour
  // does not reproduce faithfully. It carries its own ink and `stroke="none"`,
  // like the silhouette above, so no sheet can stroke it back.
  if (kind === "folder-outline") return <QcTracedMark icon="directory.folder-outline" className={classes} />;
  // The same outline with a plus in its lower-left quadrant. The unit begins
  // it on the cell's third row where it begins the plain folder on the
  // second, and the art now carries that unit, which is what retired
  // official-directory.css's compensating `translate: 0 2px`.
  if (kind === "new-folder") return <QcTracedMark icon="directory.new-folder" className={classes} />;
  if (kind === "filter") return <QcScreenVectorLayers icon="directory.filter" className={classes} />;
  if (kind === "sort") return <QcScreenVectorLayers icon="directory.sort" className={classes} />;
  if (kind === "arrange" || kind === "upload") return <QcScreenVectorLayers icon="directory.arrange" className={classes} />;
  // The funnel and the magnifier were the manual's own vector primitives.
  if (kind === "search") return <QcTracedMark icon="directory.search" className={classes} />;
  if (kind === "done") return <QcScreenVectorLayers icon="interface.check" className={classes} />;
  // `trash` is handled by directoryReferenceIcon above; all directory icon
  // variants must resolve explicitly so a new variant cannot silently render
  // as an unrelated glyph.
  return null;
}

export function QcLibraryIcon({ kind, className }: { kind: QcLibraryIconName; className?: string }) {
  const classes = `qc-library-icon qc-library-icon-${kind}${className ? ` ${className}` : ""}`;
  if (kind === "clock") return <QcScreenVector icon="library.clock" className={classes} />;
  if (kind === "capture-library") return <QcScreenVector icon="library.capture-library" className={classes} />;
  // Seven inks, and every one of them a neutral grey on a neutral grey ground.
  // The unit drew this stripe stack hard-edged, so the shape is its own 1-bit
  // mask, taken per ink by nearest palette entry.
  if (kind === "capture-header") return <QcScreenVectorLayers icon="library.capture-header" className={classes} />;
  if (kind === "impulse-response") return <QcScreenVector icon="library.impulse-response" className={classes} />;
  if (kind === "binoculars") return <QcScreenVector icon="library.binoculars" className={classes} />;
  // The unit knocks the bolt out of the fill rather than stroking it over the
  // top, so the mark is one filled region with a hole and needs no second ink -
  // which also retires a stroke drawn in the raised-panel colour over a ground
  // that is not the raised panel.
  if (kind === "broken-heart") return <QcScreenVector icon="library.broken-heart" className={classes} />;
  // The category mark and the tab mark are one drawing and share one entry.
  return <QcTracedMark icon="library.heart" className={classes} />;
}

export function QcEditorIcon({ kind, className }: { kind: QcEditorIconName; className?: string }) {
  // A bypassed block swaps the power symbol for a barred ring, and the unit
  // tints only the ring: editor-digital-flanger draws it in the block accent
  // (the Modulation browser colour) while the assignment screens draw it muted,
  // with the crossing bar white on both.
  if (kind === "bypass-muted") {
    const vector = QC_SCREEN_ICON_VECTORS["editor.bypass-muted"];
    return <svg className={className} data-qc-icon="bypass-muted" width={vector.width} height={vector.height} viewBox={`0 0 ${vector.width} ${vector.height}`} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace} aria-hidden="true">
      <path d={vector.layers[0].path} fill="var(--editor-color, currentColor)" stroke="none" />
      <path d={vector.layers[1].path} fill={QC_COLORS.reference.primaryText} stroke="none" />
    </svg>;
  }
  // The unlit power symbol shares that same per-row tint, not a fixed role -
  // reference-parameter-editor.css already expects it (its own
  // `.is-bypassed header .editor-bypass { color: ... }` rule has had nothing
  // to repaint since this fell through to QcScreenVectorLayers, which always
  // painted reference.primaryText regardless). The lit power button needs the
  // same currentColor treatment: its own `color`
  // (lane-control-panel.tsx's `.lane-control-power`) is a near-black token for
  // exactly this reason, since its ring sits on a green fill, not white.
  if (kind === "bypass") {
    const vector = QC_SCREEN_ICON_VECTORS["editor.bypass"];
    return <svg className={className} data-qc-icon="bypass" width={vector.width} height={vector.height} viewBox={`0 0 ${vector.width} ${vector.height}`} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace} aria-hidden="true">
      <path d={vector.layers[0].path} fill="currentColor" stroke="none" />
    </svg>;
  }
  if (kind === "scene-next" || kind === "scene-previous") return <QcSceneShuttle direction={kind === "scene-next" ? "next" : "previous"} />;
  // Copy is interface.file, one pixel right: the two menus that use this mark
  // draw the same two-sheets glyph a column apart, so this is that entry read
  // through a shifted box rather than a second copy of the geometry, exactly
  // as `next` reads editor.change for the same pair of screens. Shifting the
  // window's origin left by one unit moves the ink one pixel right inside the
  // same box, and the mark's own extent has the room.
  if (kind === "copy")
    return (
      <svg className={className} viewBox="-1 0 24 24" shapeRendering={vectorShapeRendering("interface.file")} aria-hidden="true">
        <path fill={QC_COLORS.reference.primaryText} stroke="none" d={screenVectorPath("interface.file")} />
      </svg>
    );
  const referenceIcon = editorScreenVector(kind);
  if (referenceIcon) return <QcScreenVectorLayers icon={referenceIcon} />;
  if (kind === "looper") {
    return <svg viewBox="0 0 26 24" shapeRendering={vectorShapeRendering("editor.looper")} aria-hidden="true"><path d={screenVectorPath("editor.looper", 1)} fill={QC_COLORS.reference.looperRing} stroke="none" /><path d={screenVectorPath("editor.looper", 0)} fill={QC_COLORS.reference.iconPrimary} stroke="none" /></svg>;
  }
  // The EXP badge is ours: the unit labels its expression ports as plain
  // `EXP 1` / `EXP 2` text and draws no badge beside an assigned parameter. The
  // mark it does draw for expression is the pedal on the block context menu's
  // Expression row, which is `editor.expression`. So the badge borrows that
  // drawing rather than carrying a second pedal of its own, in the caller's
  // own colour.
  if (kind === "assignment-expression") return <QcTracedMark icon="editor.expression" />;
  return null;
}

/**
 * Settings rows the unit draws as one fixed, single-ink mark.
 *
 * `system-volume` and `system-storage` used to live here as well, and could
 * not: the unit draws both in *two* inks - `reference.iconPrimary` over
 * `reference.looperRing` - and a mask filled with `currentColor` has one. So
 * the two controls route to QcKnobGlyph and QcStorageGlyph instead, which
 * draw the second ink because their geometry is a function of the value the
 * row is showing.
 */
const REFERENCE_SETTINGS_GLYPH_PATHS: Partial<Record<QcScreenGlyphName, string>> = QC_SETTINGS_GLYPH_PATHS;

/**
 * The unit's signed-out cloud-account mark: a person in the muted ink with a
 * red cross at the upper left, traced from the walked header at 24 square
 * (head and shoulders centred on x=15, the cross over x=1..9, y=4..12).
 */
export function QcAccountSignedOutGlyph() {
  return <svg className="directory-account-status" viewBox="0 0 24 24" fill="none" strokeWidth="1.5" aria-hidden="true">
    <circle className="status-person" cx="15.2" cy="7.5" r="4.1" />
    <path className="status-person" d="M6.3 24V21.5A8.6 8.6 0 0 1 23.5 21.5V24" />
    <path className="status-error" strokeLinecap="square" d="M1.2 4.2L8.6 11.6M8.6 4.2L1.2 11.6" />
  </svg>;
}

export function QcScreenGlyph({ kind, className }: { kind: QcScreenGlyphName; className?: string }) {
  const frame = { className: `qc-screen-glyph qc-screen-glyph-${kind}${className ? ` ${className}` : ""}`, viewBox: "0 0 24 24", "data-qc-screen-glyph": kind, shapeRendering: QC_ICON_SHAPE_RENDERING.curveArt, "aria-hidden": true } as const;
  const referenceSettingsPath = REFERENCE_SETTINGS_GLYPH_PATHS[kind];
  if (referenceSettingsPath) return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={referenceSettingsPath} /></svg>;
  if (kind === "status") return <svg {...frame}><path className="status-error" d={QC_SCREEN_GLYPH_GEOMETRY.status.error} /><path className="status-bars" d={QC_SCREEN_GLYPH_GEOMETRY.status.bars} /></svg>;
  if (kind === "contact") return <svg {...frame}><rect {...QC_SCREEN_GLYPH_GEOMETRY.contact.frame} /><path d={QC_SCREEN_GLYPH_GEOMETRY.contact.flap} /></svg>;
  if (kind === "global-bypass") return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={QC_SCREEN_GLYPH_GEOMETRY["global-bypass"]} /></svg>;
  if (kind === "scene-bypass") return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={QC_SCREEN_GLYPH_GEOMETRY["scene-bypass"]} /></svg>;
  if (kind === "stomp-bypass") return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={QC_SCREEN_GLYPH_GEOMETRY["stomp-bypass"]} /></svg>;
  if (kind === "hold") return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={QC_SCREEN_GLYPH_GEOMETRY.hold} /></svg>;
  if (kind === "latency") return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={QC_SCREEN_GLYPH_GEOMETRY.latency} /></svg>;
  if (kind === "headphones") return <svg {...frame}><path d={QC_SCREEN_GLYPH_GEOMETRY.headphones.band} /><rect {...QC_SCREEN_GLYPH_GEOMETRY.headphones.left} /><rect {...QC_SCREEN_GLYPH_GEOMETRY.headphones.right} /></svg>;
  if (kind === "swap") return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={QC_SCREEN_GLYPH_GEOMETRY.swap} /></svg>;
  if (kind === "gig-view") return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={QC_SCREEN_GLYPH_GEOMETRY["gig-view"]} /></svg>;
  if (kind === "device") return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={QC_SCREEN_GLYPH_GEOMETRY.device} /></svg>;
  if (kind === "about") return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={QC_SCREEN_GLYPH_GEOMETRY.about} /></svg>;
  if (kind === "info") return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={QC_SCREEN_GLYPH_GEOMETRY.info} /></svg>;
  if (kind === "report") return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={QC_SCREEN_GLYPH_GEOMETRY.report} /></svg>;
  if (kind === "diagnostics") return <svg {...frame}><path d={QC_SCREEN_GLYPH_GEOMETRY.diagnostics} fill="currentColor" stroke="none" /></svg>;
  if (kind === "licenses") return <svg {...frame} shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace}><path fill="currentColor" stroke="none" d={QC_SCREEN_GLYPH_GEOMETRY.licenses} /></svg>;
  if (kind === "scene-copy") return <svg {...frame}><rect {...QC_SCREEN_GLYPH_GEOMETRY["scene-copy"].front} /><path d={QC_SCREEN_GLYPH_GEOMETRY["scene-copy"].back} /></svg>;
  if (kind === "progress") return <svg {...frame}><path d={QC_SCREEN_GLYPH_GEOMETRY.progress.arc} /><path d={QC_SCREEN_GLYPH_GEOMETRY.progress.head} /></svg>;
  if (kind === "shift") return <svg {...frame}><path d={QC_SCREEN_GLYPH_GEOMETRY.shift} /></svg>;
  if (kind === "route-usb") return <svg {...frame}><path d={QC_SCREEN_GLYPH_GEOMETRY["route-usb"]} /></svg>;
  if (kind === "route-input") return <svg {...frame}><path d={QC_SCREEN_GLYPH_GEOMETRY["route-input"]} /></svg>;
  if (kind === "route-row") return <svg {...frame}><path d={QC_SCREEN_GLYPH_GEOMETRY["route-row"]} /></svg>;
  if (kind === "route-output") return <svg {...frame}><path d={QC_SCREEN_GLYPH_GEOMETRY["route-output"]} /></svg>;
  // The capture wizard's two header arrows and the progress rail's are the
  // registry's `interface.previous` / `interface.arrow-right`, not three more
  // stroked chevrons.
  //
  // A previous fix moved the entries instead of fixing their placement: -1 on
  // every x of `interface.previous`, +3 on every x of `interface.arrow-right`.
  // The -1 holds; the +3 was a placement correction written into the drawing
  // itself, which pushed the tip outside the entry's own declared width.
  //
  // What inked left of the unit was the box, not the art. `.name-editor-next`
  // let its glyph take the `#root#root` 1em default and never centred it, so
  // a 24-unit drawing was painted at the wrong size; +3 bought back one edge
  // of that and lost the other. With the x's back at the device's and that
  // button bound to a centred 24px box, this reads correctly.
  //
  // The four `capture-connections` steps after intro still sit one pixel off
  // from the unit: their next button is a different width from
  // `capture-intro`'s own `is-wide` geometry. That one is the button's to
  // fix, not the entry's; do not nudge the entry back from here.
  if (kind === "capture-back") return <svg {...frame}><path d={screenVectorPath("interface.previous")} fill="currentColor" stroke="none" /></svg>;
  if (kind === "capture-next") return <svg {...frame}><path d={screenVectorPath("interface.arrow-right")} fill="currentColor" stroke="none" /></svg>;
  if (kind === "capture-skip") return <svg {...frame}><path d={QC_SCREEN_GLYPH_GEOMETRY["capture-skip"]} /></svg>;
  if (kind === "capture-step-check") return <svg {...frame}><path d={QC_SCREEN_GLYPH_GEOMETRY["capture-step-check"]} /></svg>;
  if (kind === "capture-step-next") return <svg {...frame}><path d={screenVectorPath("interface.arrow-right")} fill="currentColor" stroke="none" /></svg>;
  return <svg {...frame}><path d={QC_SCREEN_GLYPH_GEOMETRY.lock.shackle} /><rect {...QC_SCREEN_GLYPH_GEOMETRY.lock.body} fill="currentColor" /></svg>;
}

// The device's Wi-Fi meter is six 2px bars on a 4px pitch, rising in threes from
// a shared baseline. The bars the signal reaches are inked; the rest stay in the
// row's own ground, so both states are drawn and only the fill differs.
export function QcWifiSignalGlyph({ strength }: { strength: number }) {
  return <svg className="qc-wifi-signal-glyph" viewBox="0 0 22 18" shapeRendering={QC_ICON_SHAPE_RENDERING.pixelTrace} aria-hidden="true">
    {QC_WIFI_SIGNAL_GEOMETRY.bars.map((bar) => <rect key={bar} className={bar < strength ? "is-lit" : ""} x={bar * QC_WIFI_SIGNAL_GEOMETRY.pitch} y={QC_WIFI_SIGNAL_GEOMETRY.baseline - bar * QC_WIFI_SIGNAL_GEOMETRY.rise} width={QC_WIFI_SIGNAL_GEOMETRY.width} height={QC_WIFI_SIGNAL_GEOMETRY.minHeight + bar * QC_WIFI_SIGNAL_GEOMETRY.rise} />)}
  </svg>;
}

export function QcLooperActionGlyph({ kind }: { kind: QcLooperActionGlyphName }) {
  const frame = { className: `qc-looper-action-glyph qc-looper-action-glyph-${kind}`, viewBox: "0 0 24 24", "data-qc-looper-action-glyph": kind, shapeRendering: QC_ICON_SHAPE_RENDERING.curveArt, "aria-hidden": true } as const;
  if (kind === "duplicate") return <svg {...frame}><circle {...QC_LOOPER_ACTION_GLYPH_GEOMETRY.duplicate.loop} /><path d={QC_LOOPER_ACTION_GLYPH_GEOMETRY.duplicate.marks} /></svg>;
  if (kind === "one-shot") return <svg {...frame}><ellipse {...QC_LOOPER_ACTION_GLYPH_GEOMETRY["one-shot"].loop} /><path d={QC_LOOPER_ACTION_GLYPH_GEOMETRY["one-shot"].arrow} /></svg>;
  if (kind === "half-speed") return <svg {...frame}><text {...QC_LOOPER_ACTION_GLYPH_GEOMETRY["half-speed"].label} textAnchor="middle" fill="currentColor" stroke="none" fontFamily={QC_TYPOGRAPHY.devicePlain} fontSize="11.5" fontWeight="700">1/2</text></svg>;
  if (kind === "punch-in") return <svg {...frame}><rect {...QC_LOOPER_ACTION_GLYPH_GEOMETRY["punch-in"].capsule} fill="currentColor" stroke="none" /><path d={QC_LOOPER_ACTION_GLYPH_GEOMETRY["punch-in"].stand} /></svg>;
  if (kind === "record") return <svg {...frame}><circle {...QC_LOOPER_ACTION_GLYPH_GEOMETRY.record.dot} fill="currentColor" /></svg>;
  if (kind === "play") return <svg {...frame}><path d={QC_LOOPER_ACTION_GLYPH_GEOMETRY.play} fill="currentColor" /></svg>;
  if (kind === "reverse") return <svg {...frame}><path d={QC_LOOPER_ACTION_GLYPH_GEOMETRY.reverse} fill="currentColor" /></svg>;
  return <svg {...frame}><path d={QC_LOOPER_ACTION_GLYPH_GEOMETRY.undo} /></svg>;
}

/**
 * The three capture types the shared device art has no mark for.
 *
 * `capture-save`'s selector shows six of the eight capture types
 * `directory-filter` lists, and three of its six are library art at 1:1 in the
 * tile's own 40-unit box, not marks of this screen's own: Default is
 * `device-category.capture`, Amp is `device-category.amp` and Cab is
 * `device-category.cab`. Those three now come from the registry through
 * `QcDeviceTile`.
 *
 * The other three are drawn larger than their 24-unit `capture-filter` twins
 * and differently - the unit's Combo Amp tile has no speaker where
 * `capture-filter.combo-amp` does, and its Overdrive tile is a pedal where
 * `capture-filter.overdrive` is a waveform - so they keep their own geometry
 * here, in the 40-unit box the tile gives them.
 *
 * They are named for the tile rather than for the type, because
 * `QcCaptureFilterIconName` already owns "Combo Amp", "Amp + Cab" and
 * "Overdrive" for the 24-unit marks the Captures filter lists, and one name
 * must mean one drawing.
 */
export type QcCaptureTypeGlyphName = "combo-amp-tile" | "amp-cab-tile" | "overdrive-tile";

export function QcCaptureTypeGlyph({ kind }: { kind: QcCaptureTypeGlyphName }) {
  const frame = { viewBox: "0 0 40 40", "data-qc-capture-type": kind, shapeRendering: QC_ICON_SHAPE_RENDERING.curveArt, "aria-hidden": true } as const;
  if (kind === "combo-amp-tile") return <svg {...frame}><rect {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["combo-amp-tile"].cabinet} /><path d={QC_CAPTURE_TYPE_GLYPH_GEOMETRY["combo-amp-tile"].panel} /><circle {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["combo-amp-tile"].lamp} fill="currentColor" stroke="none" /></svg>;
  if (kind === "amp-cab-tile") return <svg {...frame}><path d={QC_CAPTURE_TYPE_GLYPH_GEOMETRY["amp-cab-tile"].head} /><rect {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["amp-cab-tile"].cabinet} /><circle {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["amp-cab-tile"].cone} /><circle {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["amp-cab-tile"].dustCap} fill="currentColor" stroke="none" /></svg>;
  // The control's pointer is a slot cut out of a solid cap, so the notch is
  // painted in the tile's own ground rather than left as a gap between arcs.
  return <svg {...frame}><rect {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["overdrive-tile"].body} /><rect {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["overdrive-tile"].jackLeft} fill="currentColor" stroke="none" /><rect {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["overdrive-tile"].jackRight} fill="currentColor" stroke="none" /><circle {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["overdrive-tile"].knobLeft} fill="currentColor" stroke="none" /><circle {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["overdrive-tile"].knobMiddle} fill="currentColor" stroke="none" /><circle {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["overdrive-tile"].knobRight} fill="currentColor" stroke="none" /><circle {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["overdrive-tile"].cap} fill="currentColor" stroke="none" /><path d={QC_CAPTURE_TYPE_GLYPH_GEOMETRY["overdrive-tile"].notch} stroke={`var(--qc-device-tile-ground, ${QC_COLORS.reference.screen})`} strokeWidth="1" /><circle {...QC_CAPTURE_TYPE_GLYPH_GEOMETRY["overdrive-tile"].led} fill="currentColor" stroke="none" /></svg>;
}

/*
 * `QcIoPortGlyph` stood here and drew its own lit INPUT 1 for io-input: two
 * stroked circles, three holes and a pair of diagonal slashes, in a 48-unit box
 * where the unit draws the socket in 68. It was a second drawing of a socket
 * the registry already traces - and the unit does not draw a second socket. It
 * lights the same io.input every other place draws unlit, inking the metal
 * layer accent and adding one rim on the body's outer edge, both of which
 * QcIoJack measures. The lit headphone jack's own entry went with it.
 */

export function QcHeadphonesGlyph() {
  return <QcScreenGlyph kind="headphones" />;
}

/**
 * The badge the unit stamps on the folder it downloads into: a dark rounded
 * plate, a light inset and a dark arrow at the folder glyph's lower-right corner. One
 * drawing for every screen that shows it: the live Directory and the Directory fixture both import it.
 */
export function DirectoryDownloadBadge() {
  return <i className="directory-nav-badge">
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <rect className="badge-plate" x="0" y="0" width="16" height="16" rx="4.5" />
      <rect className="badge-disc" x="2" y="2" width="12" height="12" rx="4" />
      <path className="badge-arrow" d="M7 4h2v4.6h2L8 11.7 5 8.6h2z" />
    </svg>
  </i>;
}

// The badge CorOS stamps on the folder it has downloaded to the device,
// bottom-right of the folder glyph: a dark rounded plate, a light disc, a dark
// arrow. The plugin directory's logo and the device browser's empty pane draw
// the same zigzag.
export function QcDeviceWaveformGlyph({ className }: { className: string }) {
  return <svg className={className} viewBox="0 0 120 140" aria-hidden="true"><path d={QC_DEVICE_WAVEFORM_GLYPH.path} /></svg>;
}

/**
 * The slot the device browser is adding into, drawn once for every browser.
 *
 * The mark inside it is `interface.add`, not a cross of this file's own: it is
 * the same 22x22 of flat utilityMark the unit draws in every port, and the
 * 24-unit asset box sits at 23 in these 70 units. The stroked pair this
 * replaces landed on the right box but filled it with a gradient the unit
 * never shows.
 */
export function QcDeviceEmptySlotGlyph({ className }: { className: string }) {
  return <svg className={className} viewBox="0 0 70 70" aria-hidden="true"><rect {...QC_DEVICE_EMPTY_SLOT_PLATE} fill="var(--qc-screen-surface-base)" /><QcScreenVectorGlyph icon="interface.add" x={23} y={23} /></svg>;
}

/** The sweep mark the Directory shows on its multi-select toolbar. */
export function QcDirectoryBroomIcon() {
  return <svg className="qc-directory-icon qc-directory-icon-broom" viewBox="0 0 24 24" aria-hidden="true"><path d={QC_DIRECTORY_BROOM_PATHS.handle} /><path d={QC_DIRECTORY_BROOM_PATHS.head} /></svg>;
}
