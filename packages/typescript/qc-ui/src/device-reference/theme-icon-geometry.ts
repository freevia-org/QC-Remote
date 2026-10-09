/**
 * Glyph geometry drawn by `../theme-icons.tsx`, kept as data alone.
 *
 * Everything here is device-reference material: outlines traced row by row or
 * from the antialiasing of the Quad Cortex's own screen frames, measurements
 * read off those frames, marks taken from Neural DSP's published manual
 * artwork, or geometry whose provenance could not be shown to be independent
 * of either. It lives in this directory so that the renderer modules carry
 * original code only. The components, their names, their unions and the
 * reasoning behind each drawing stay in theme-icons.tsx; nothing here is
 * redrawn, rounded or reordered on the way in.
 */

/** QcSynthFooterIcon: the footer marks from the Overlord Synth's own screen. */
export const QC_SYNTH_FOOTER_PATHS = {
  filter: "M5.19 5.5L5.5 5.19H21.5L22.5 5.93L22.93 6.5L24.02 8.5L24.74 9.5L25.15 10.5L25.91 11.5L26.38 12.5L28.81 16.5L28.5 17.09L28.09 17.5L27.5 17.81L27.17 17.5L25.98 15.5L25.19 14.5L24.85 13.5L24.07 12.5L22.98 10.5L22.26 9.5L21.85 8.5L21.02 7.5L20.5 7.12H5.5L5 6.5Z",
  "global-amp": "M13.48 5.5L14.5 5.13L16.5 5H17.5L19.5 5.28L20.5 5.77L22.5 6.48L23.5 7.09L24.79 8.5L23.5 9.72L21.5 8.29L20.5 7.91L19.5 7.33L17.5 7.03H15.5L14.5 7.17L13.52 7.5L11.51 8.5L10.5 9.23L9.33 10.5L8.5 11.68L8.01 12.5L7.17 14.5L7.03 17.5L7.43 19.5L8.17 21.5L9.72 23.5L8.5 24.79L7.13 23.5L6.03 21.5L5.35 19.5L5.09 18.5L5 17.5L5.05 15.5L5.21 14.5L6.21 11.5L6.94 10.5L7.48 9.5L9.5 7.48L11.5 6.21ZM13.89 9.5L14.5 9.21L16.5 8.94L18.5 9.09L19.5 9.35L20.5 9.94L21.17 10.5L20.5 11.43L17.43 14.5L17.01 15.5L17.5 16.03L18.5 16.29L22.5 12.29L23.5 12.97L23.87 13.5L24.33 15.5L24.43 17.5L24.06 19.5L23.65 20.5L22.94 21.5L21.5 22.94L19.5 23.97L17.5 24.43H16.5L13.5 23.91L12.5 23.29L10.5 21.39L9.49 19.5L9.09 18.5L9 15.5L9.21 14.5L10.09 12.5L12.5 10.13Z"
} as const;

/** QcExpressionAssignmentLink: the two-tone bracket and the link inside it. */
export const QC_EXPRESSION_ASSIGNMENT_LINK_PATHS = {
  brackets: "M4 2h4v2H6v16h2v2H4V2Zm12 0h4v20h-4v-2h2V4h-2V2Z",
  link: "M7 11h10v2H7Z"
} as const;

/** QcKnobGlyph: the settings list's volume knob, measured in its 24-unit box. */
export const QC_SETTINGS_KNOB_GEOMETRY = {
  centre: 11.72,
  faceRadius: 7.78,
  scaleInner: 9.75,
  scaleOuter: 11.75,
  pointerWidth: 2,
  pointerInner: 1.05,
  /** The fixed position the unit draws the settings row's knob at. */
  value: 2 / 3
} as const;

/** QcStorageGlyph: the settings list's Device Storage pie, in its 24-unit box. */
export const QC_SETTINGS_STORAGE_GEOMETRY = {
  centre: 12,
  radius: 12,
  sliceRadius: 9.2,
  sliceOffset: 2 * Math.SQRT2,
  /** The fixed share the unit draws the settings row's pie at. */
  value: 1 / 4
} as const;

/** QcPresetStackIcon: the device-presets stack. */
export const QC_PRESET_STACK_PATHS = {
  top: "m12 3.5 7 3.5-7 3.5L5 7l7-3.5Z",
  layers: "m19 11-7 3.5L5 11m14 4-7 3.5L5 15"
} as const;

/** QcUiIcon `capture`: the four partitions not shared with library.capture-header. */
export const QC_CONTEXT_CAPTURE_PATHS = {
  muted: "M15 0h1v1h-1ZM4 3h8v1h-8ZM21 4h1v1h-1ZM1 7h11v1h-11ZM23 8h1v1h-1ZM0 11h12v1h-12ZM1 15h11v1h-11ZM3 19h9v1h-9Z",
  mid: "M12 2h6v1h-6ZM12 6h10v1h-10ZM12 10h11v1h-11ZM12 14h11v1h-11ZM12 18h10v1h-10ZM19 21h1v1h-1ZM12 22h6v1h-6ZM11 23h1v1h-1Z",
  dark: "M5 2h7v1h-7ZM18 2h1v1h-1ZM1 6h11v1h-11ZM0 10h12v1h-12ZM23 10h1v1h-1ZM0 14h12v1h-12ZM23 14h1v1h-1ZM2 18h10v1h-10ZM20 20h1v1h-1ZM7 22h5v1h-5ZM9 23h2v1h-2Z",
  black: "M16 0h1v1h-1ZM18 1h1v1h-1ZM4 2h1v1h-1ZM3 3h1v1h-1ZM22 5h1v1h-1ZM22 6h1v1h-1ZM0 15h1v1h-1ZM2 19h1v1h-1ZM6 22h1v1h-1ZM18 22h1v1h-1ZM8 23h1v1h-1Z"
} as const;

/** QcUiIcon `cpu`: pixel trace. */
export const QC_CPU_GLYPH = { path: "M7 0h2v1h-2ZM15 0h2v1h-2ZM7 1h2v1h-2ZM15 1h2v1h-2ZM3 2h18v1h-18ZM2 3h20v1h-20ZM2 4h3v1h-3ZM19 4h3v1h-3ZM2 5h2v1h-2ZM20 5h2v1h-2ZM2 6h2v1h-2ZM20 6h2v1h-2ZM0 7h4v1h-4ZM7 7h10v1h-10ZM20 7h4v1h-4ZM0 8h4v1h-4ZM7 8h10v1h-10ZM20 8h4v1h-4ZM2 9h2v1h-2ZM7 9h2v1h-2ZM15 9h2v1h-2ZM20 9h2v1h-2ZM2 10h2v1h-2ZM7 10h2v1h-2ZM15 10h2v1h-2ZM20 10h2v1h-2ZM2 11h2v1h-2ZM7 11h2v1h-2ZM15 11h2v1h-2ZM20 11h2v1h-2ZM2 12h2v1h-2ZM7 12h2v1h-2ZM15 12h2v1h-2ZM20 12h2v1h-2ZM2 13h2v1h-2ZM7 13h2v1h-2ZM15 13h2v1h-2ZM20 13h2v1h-2ZM2 14h2v1h-2ZM7 14h2v1h-2ZM15 14h2v1h-2ZM20 14h2v1h-2ZM0 15h4v1h-4ZM7 15h10v1h-10ZM20 15h4v1h-4ZM0 16h4v1h-4ZM7 16h10v1h-10ZM20 16h4v1h-4ZM2 17h2v1h-2ZM20 17h2v1h-2ZM2 18h2v1h-2ZM20 18h2v1h-2ZM2 19h3v1h-3ZM19 19h3v1h-3ZM2 20h20v1h-20ZM3 21h18v1h-18ZM7 22h2v1h-2ZM15 22h2v1h-2ZM7 23h2v1h-2ZM15 23h2v1h-2Z" } as const;

/** QcScreenHeaderGlyph `nav-save`: placed in the 800x480 screen frame. */
export const QC_SCREEN_HEADER_SAVE_GLYPH = { path: "M706 13h17v1h-17ZM705 14h19v1h-19ZM705 15h2v18h-2ZM722 15h3v1h-3ZM723 16h3v1h-3ZM709 17h6v3h-6ZM717 17h3v3h-3ZM724 17h3v1h-3ZM725 18h2v15h-2ZM709 20h11v3h-11ZM705 33h22v1h-22ZM706 34h20v1h-20Z" } as const;

/** QcRouteGlyph: the marks it dispatches to from the device's route labels. */
export const QC_ROUTE_GLYPH_GEOMETRY = {
  stereoOutput: { path: "M22.96 4.5L23.5 3.96L24.5 3.87L26.74 6.5L28.19 8.5V9.5L26.74 11.5L24.5 14.13L23.5 14.04L22.96 13.5L23.26 12.5L24.96 10.5L24.5 10.06L4.5 10L4 9.5V8.5L4.5 8L24.5 7.94L24.96 7.5L23.26 5.5ZM22.96 18.5L23.5 17.96L24.5 17.87L26.74 20.5L28.19 22.5V23.5L26.74 25.5L24.5 28.13L23.5 28.04L22.96 27.5L23.26 26.5L24.96 24.5L24.5 24.06L4.5 24L4 23.5V22.5L4.5 22L24.5 21.94L24.96 21.5L23.26 19.5Z" },
  usb: { mark: "M15 8.5L15.5 7.5H16.5L17 8.5V18.5L17.5 19H19.5L20 19.5L19.5 20.01H17.5L17.01 20.5L17 24.5L16.5 25H15.5L15 24.5V23.5L14.5 23H10.5L10 22.5L10.5 22H14.5L15 21.5ZM23.22 14.5L23.5 14.22L24.5 14L25 14.5L25.08 15.5L24.83 16.5L24.5 16.87L23.5 17L22 18.5L21.5 19.62L20.5 19L20 18.5L20.5 17.92L21.24 17.5L22.5 16.38L23.5 16L24 15.5ZM7.5 15.5L8.5 15.38L8.62 15.5V17.5L8.78 18.5L8 19.5L8.5 20L9.5 20.29L9.71 20.5L10 21.5L9.5 22L8.5 21L7.5 20.78L7.22 20.5L6.96 19.5L7 16.5ZM13.5 30.5L14.5 31L15 31.5V32H13.99V31.5ZM18.5 30.5L18.01 31.5V32H17V31.5L17.5 31Z", ink: "M15.04 2.5L15.5 1.77H16.5L17.41 3.5L18.5 4.6L19 5.5L19.89 6.5L19.5 7.35L16.5 7.47L12.5 7.35L12.11 6.5L13 5.5L13.5 4.6L14.59 3.5ZM22 9.5L22.5 9H26.5L27 9.5L26.96 13.5L26.5 13.96L25.5 14.06L22.5 13.96L22.04 13.5ZM6.73 10.5L7.5 10.06L9.5 11.08L9.92 11.5L9.98 13.5L9.21 14.5L8.5 14.98L7.5 15.27L6.5 15.12L5.13 13.5L5.02 12.5L5.5 11.71ZM23.47 15.5L23.5 15.47L23.53 15.5L23.5 15.53ZM23.41 17.5L23.5 17.41L23.59 17.5L23.5 17.62L22.5 18.53ZM17.47 18.5L19.5 18.41L20.53 19.5L19.5 18.59ZM8.47 19.5L8.5 19.47L8.53 19.5L8.5 19.53ZM8.47 21.5L8.5 21.47L8.53 21.5L8.5 21.53ZM12.27 23.5L13.5 23.41H14.5L14.59 23.5L14.5 23.59ZM15.19 25.5L15.5 25.19H16.5L18.94 27.5L19 29.5L18.31 30.5L16.94 31.5V32H15.06V31.5L13.69 30.5L13 29.5L13.06 27.5L14.5 26.04Z" },
  return: { surface: "M4.2 2.5L4.5 2.22L5.5 3L6.5 2H26.5L27.5 2.22L28.5 3L30.03 4.5L30.78 5.5L30 6.5L31 7.5L30.5 8H29.5L29 7.5V5.5L27.5 4H4.5L3 5.5V7.5L2.5 8H1.5L1 7.5V6.5L1.22 5.5L2 4.5L3.5 2.97ZM8 12.5L8.5 12L9.22 12.5L8.5 13ZM10.86 12.5L11.5 12.09L11.91 12.5L11.5 13.14ZM14.04 12.5L14.5 12.04L15.19 12.5L14.5 12.96ZM27 13.5L27.5 13L28 13.5L27.5 14ZM4 14.5L4.5 14H6.5L7.28 14.5L6.5 15H4.5L3.5 15.91L3.09 15.5ZM15 15.5L15.5 15L16 15.5L15.5 16ZM30 15.5L30.5 15L31 15.5L30.5 16ZM3.09 18.5L3.5 18.09L3.91 18.5V21.5L3.5 21.91L3.09 21.5ZM15.04 18.5L15.5 18.04L15.96 18.5L15.5 19.19ZM29.78 18.5L30.5 18L31 18.5L30.5 19ZM9 19.5L9.5 19L10 19.5L9.5 20.25ZM12 19.5L12.5 18.75L13 19.5L12.5 20ZM27.09 20.5L27.5 20.09L27.91 20.5L27.5 20.91ZM11 21.5L11.5 20.57L12 21.5L11.5 22ZM17 21.5L17.5 21L18 21.5L17.5 22ZM1 26.5L1.5 26H2.5L3 26.5V28.5L4.5 30H27.5L29 28.5V26.5L29.5 26H30.5L31 26.5V27.5L30.78 28.5L30 29.5L28.5 31.03L27.8 31.5V32H27.22V31.5L26.5 31L26 31.5V32H4.22V31.5L3.5 31L1.97 29.5L1.22 28.5L2 27.5Z", ink: "M1.43 12.5L2.5 12.22H6.5L7.05 12.5L7.53 13.5L6.5 14H4.5L3.5 14.19L3.19 14.5L3.09 15.5L3.5 15.95L6.5 16.22L6.78 16.5L6.92 17.5L6.5 17.92L3.5 18.09L3.09 18.5V21.5L2.5 22L1.5 21.92L1.08 21.5V13.5ZM9.22 12.5L9.5 12.22H10.5L11.78 13.5L12.5 14.8L13.5 14.75L14.08 13.5L15.5 12.22L16.57 12.5L16.4 13.5L15.92 14.5L15 15.5L14.53 16.5V17.5L15.04 18.5L16.5 20.61L16.96 21.5L16.5 21.96L15.5 22L14.7 21.5L13.5 19.53L12.5 18.7L11.57 20.5L10.92 21.5L10.5 21.92L9.5 22L8.7 21.5L10.7 18.5L11.08 17.5L11.22 16.5L9.5 13.5ZM25.43 12.5L25.5 12.43L26.5 13.08L27.5 14L29.5 15.13L30.5 16.08L31.2 16.5L31.28 17.5L27.5 20.09L26.5 20.92L25.5 21.5L25 20.5V13.5Z" },
  xlrOutput: {
    body: { cx: "12", cy: "12", r: "8" },
    pinTop: { cx: "12", cy: "7.6", r: "1.15" },
    pinLeft: { cx: "8.2", cy: "14.2", r: "1.15" },
    pinRight: { cx: "15.8", cy: "14.2", r: "1.15" }
  },
  row: { path: "M26.35 3.5L26.5 3.35L28.5 4.78L29.5 5.3L31.11 6.5L31.34 7.5L29.5 8.81L28.5 9.26L26.5 10.74L26.26 10.5L26 9.5L26.02 8.5L25.5 8H2.5L2 7.5V6.5L2.5 6H25.5L26.04 5.5L26.06 4.5ZM30 16.5L30.5 16H32V26H8.5L7.98 26.5L8 27.5L7.74 28.5L7.5 28.74L5.5 27.26L4.5 26.81L2.66 25.5L2.89 24.5L4.5 23.3L5.5 22.78L7.5 21.35L7.65 21.5L7.94 22.5L7.96 23.5L8.5 24H29.5L30 23.5Z" },
  output: { jack: "M15 5.5a7 7 0 1 0 0 13", arrow: "M8 12h12m-3-3 3 3-3 3" },
  fallback: {
    jack: { cx: "7", cy: "12", r: "3" },
    input: "M10 12h10l-3-3m3 3-3 3",
    output: "M14 12H4l3-3m-3 3 3 3"
  }
} as const;

/** QcScreenGlyph: the settings rows the unit draws as one fixed single-ink mask. */
export const QC_SETTINGS_GLYPH_PATHS = {
  system: "M11 0h4v1h-4ZM10 1h6v1h-6ZM10 2h2v1h-2ZM14 2h2v1h-2ZM5 3h2v1h-2ZM9 3h2v1h-2ZM15 3h2v1h-2ZM19 3h2v1h-2ZM4 4h7v1h-7ZM15 4h7v1h-7ZM3 5h2v3h-2ZM21 5h2v1h-2ZM21 6h3v1h-3ZM21 7h2v1h-2ZM4 8h1v1h-1ZM11 8h4v1h-4ZM21 8h1v1h-1ZM4 9h2v2h-2ZM10 9h6v1h-6ZM20 9h2v2h-2ZM9 10h2v4h-2ZM15 10h2v4h-2ZM2 11h3v1h-3ZM21 11h3v1h-3ZM1 12h3v1h-3ZM22 12h2v1h-2ZM1 13h2v2h-2ZM23 13h1v2h-1ZM10 14h6v1h-6ZM2 15h3v1h-3ZM11 15h4v1h-4ZM22 15h2v1h-2ZM2 16h5v1h-5ZM19 16h5v1h-5ZM5 17h3v1h-3ZM18 17h3v1h-3ZM6 18h2v3h-2ZM18 18h2v3h-2ZM11 20h4v1h-4ZM6 21h6v1h-6ZM14 21h6v1h-6ZM8 22h3v1h-3ZM15 22h4v1h-4Z",
  wifi: "M9 3h6v1h-6ZM6 4h12v1h-12ZM4 5h4v1h-4ZM16 5h4v1h-4ZM2 6h3v1h-3ZM19 6h3v1h-3ZM2 7h2v1h-2ZM20 7h2v1h-2ZM9 8h6v1h-6ZM7 9h10v1h-10ZM5 10h4v1h-4ZM15 10h4v1h-4ZM5 11h2v1h-2ZM17 11h2v1h-2ZM10 13h4v1h-4ZM8 14h8v1h-8ZM8 15h2v1h-2ZM14 15h2v1h-2ZM11 18h2v1h-2ZM10 19h4v2h-4ZM11 21h2v1h-2Z",
  "system-updates": "M13 1h1v1h-1ZM11 2h3v1h-3ZM9 3h6v1h-6ZM9 4h8v1h-8ZM11 5h3v1h-3ZM15 5h3v1h-3ZM4 6h2v2h-2ZM13 6h1v1h-1ZM17 6h2v1h-2ZM18 7h2v1h-2ZM3 8h2v2h-2ZM19 8h2v2h-2ZM2 10h2v5h-2ZM20 10h1v1h-1ZM20 11h2v5h-2ZM3 15h1v1h-1ZM3 16h2v2h-2ZM20 16h1v1h-1ZM19 17h2v2h-2ZM4 18h2v1h-2ZM5 19h2v1h-2ZM12 19h1v1h-1ZM19 19h1v1h-1ZM6 20h3v1h-3ZM12 20h3v1h-3ZM7 21h10v1h-10ZM9 22h8v1h-8ZM12 23h3v1h-3Z",
  "system-brightness": "M11 1h2v4h-2ZM4 4h2v1h-2ZM18 4h2v1h-2ZM4 5h3v1h-3ZM17 5h3v1h-3ZM5 6h2v1h-2ZM17 6h2v1h-2ZM10 8h3v1h-3ZM9 9h6v1h-6ZM9 10h1v1h-1ZM14 10h2v3h-2ZM1 11h4v2h-4ZM8 11h2v3h-2ZM19 11h4v2h-4ZM14 13h1v1h-1ZM9 14h6v1h-6ZM11 15h3v1h-3ZM5 17h2v1h-2ZM17 17h2v1h-2ZM4 18h3v1h-3ZM17 18h3v1h-3ZM4 19h2v1h-2ZM11 19h2v4h-2ZM18 19h2v1h-2Z",
  "power-functions": "M2 0h20v1h-20ZM1 1h22v1h-22ZM0 2h24v1h-24ZM0 3h16v1h-16ZM17 3h7v1h-7ZM0 4h15v1h-15ZM16 4h8v3h-8ZM0 5h14v1h-14ZM0 6h12v1h-12ZM0 7h11v1h-11ZM15 7h9v2h-9ZM0 8h10v1h-10ZM0 9h9v1h-9ZM14 9h10v3h-10ZM0 10h8v1h-8ZM0 11h6v1h-6ZM0 12h10v3h-10ZM18 12h6v1h-6ZM16 13h8v1h-8ZM15 14h9v1h-9ZM0 15h9v2h-9ZM14 15h10v1h-10ZM13 16h11v1h-11ZM0 17h8v3h-8ZM12 17h12v1h-12ZM10 18h14v1h-14ZM9 19h15v1h-15ZM0 20h7v1h-7ZM8 20h16v1h-16ZM0 21h24v1h-24ZM1 22h22v1h-22ZM2 23h20v1h-20Z",
  factory: "M0 3h6v8h-6ZM11 7h1v1h-1ZM17 7h1v1h-1ZM23 7h1v1h-1ZM10 8h2v1h-2ZM16 8h2v1h-2ZM22 8h2v1h-2ZM8 9h4v1h-4ZM14 9h4v1h-4ZM20 9h4v1h-4ZM7 10h5v1h-5ZM13 10h5v1h-5ZM19 10h5v1h-5ZM0 11h24v4h-24ZM0 15h6v4h-6ZM9 15h3v4h-3ZM15 15h3v4h-3ZM21 15h3v4h-3ZM0 19h24v5h-24Z",
  "account-cloud": "M10 2h4v1h-4ZM8 3h8v1h-8ZM6 4h3v1h-3ZM15 4h3v1h-3ZM6 5h2v1h-2ZM16 5h2v1h-2ZM5 6h2v1h-2ZM17 6h2v1h-2ZM5 7h1v1h-1ZM18 7h1v1h-1ZM4 8h2v2h-2ZM18 8h2v2h-2ZM2 10h4v1h-4ZM18 10h4v1h-4ZM1 11h3v1h-3ZM20 11h3v1h-3ZM1 12h2v1h-2ZM21 12h2v1h-2ZM0 13h2v4h-2ZM22 13h2v4h-2ZM1 17h2v1h-2ZM21 17h2v1h-2ZM1 18h3v1h-3ZM20 18h3v1h-3ZM2 19h20v1h-20ZM4 20h16v1h-16Z",
  user: "M10 1h4v1h-4ZM9 2h6v1h-6ZM8 3h2v1h-2ZM14 3h2v1h-2ZM7 4h2v4h-2ZM15 4h2v4h-2ZM8 8h2v1h-2ZM14 8h2v1h-2ZM9 9h6v2h-6ZM7 11h10v1h-10ZM6 12h3v1h-3ZM15 12h3v1h-3ZM5 13h3v1h-3ZM16 13h3v1h-3ZM4 14h3v1h-3ZM17 14h3v1h-3ZM4 15h2v1h-2ZM18 15h2v1h-2ZM4 16h1v1h-1ZM19 16h1v1h-1ZM3 17h2v5h-2ZM19 17h2v5h-2Z",
  backup: "M10 2h4v1h-4ZM8 3h8v1h-8ZM6 4h12v1h-12ZM6 5h6v1h-6ZM13 5h5v1h-5ZM5 6h5v1h-5ZM13 6h6v1h-6ZM5 7h3v1h-3ZM15 7h4v1h-4ZM4 8h5v1h-5ZM17 8h3v2h-3ZM4 9h7v1h-7ZM13 9h1v1h-1ZM2 10h4v1h-4ZM9 10h6v1h-6ZM18 10h4v1h-4ZM1 11h5v2h-5ZM8 11h8v3h-8ZM18 11h5v2h-5ZM0 13h6v2h-6ZM18 13h6v2h-6ZM9 14h6v1h-6ZM0 15h7v2h-7ZM10 15h1v1h-1ZM13 15h2v1h-2ZM17 15h7v1h-7ZM15 16h9v1h-9ZM1 17h8v1h-8ZM16 17h7v1h-7ZM1 18h10v1h-10ZM14 18h9v1h-9ZM2 19h9v1h-9ZM12 19h10v1h-10ZM4 20h16v1h-16Z",
  "device-midi": "M10 1h5v1h-5ZM7 2h10v1h-10ZM5 3h5v1h-5ZM14 3h5v1h-5ZM4 4h6v1h-6ZM14 4h6v1h-6ZM3 5h2v1h-2ZM8 5h2v1h-2ZM14 5h2v1h-2ZM19 5h2v2h-2ZM3 6h1v1h-1ZM8 6h3v1h-3ZM13 6h3v1h-3ZM2 7h3v2h-3ZM8 7h8v2h-8ZM19 7h3v2h-3ZM1 9h22v1h-22ZM1 10h3v1h-3ZM5 10h14v1h-14ZM21 10h2v1h-2ZM1 11h1v2h-1ZM6 11h12v3h-12ZM22 11h1v2h-1ZM1 13h2v1h-2ZM21 13h2v1h-2ZM2 14h20v3h-20ZM3 17h18v1h-18ZM4 18h16v1h-16ZM5 19h4v1h-4ZM15 19h4v1h-4ZM6 20h2v1h-2ZM16 20h2v1h-2ZM16 21h1v1h-1Z"
};

/** QcScreenGlyph: every other screen glyph it draws. */
export const QC_SCREEN_GLYPH_GEOMETRY = {
  status: { error: "m2 3 6 6m0-6L2 9", bars: "M3 20v-3m4 3v-6m4 6v-9m4 9V8m4 12V5" },
  contact: { frame: { x: "1.5", y: "3.5", width: "21", height: "17", rx: ".75" }, flap: "m2.5 5 9.5 8L21.5 5" },
  "global-bypass": "M2 1h1v1h-1ZM21 1h1v1h-1ZM1 2h3v1h-3ZM20 2h3v1h-3ZM2 3h1v1h-1ZM10 3h4v1h-4ZM21 3h1v1h-1ZM8 4h8v1h-8ZM6 5h4v1h-4ZM15 5h3v1h-3ZM5 6h3v1h-3ZM16 6h3v1h-3ZM5 7h2v1h-2ZM17 7h2v1h-2ZM4 8h2v1h-2ZM18 8h2v2h-2ZM4 9h1v1h-1ZM3 10h2v4h-2ZM11 10h2v1h-2ZM19 10h2v4h-2ZM10 11h4v2h-4ZM11 13h2v1h-2ZM4 14h2v2h-2ZM19 14h1v1h-1ZM18 15h2v1h-2ZM5 16h2v1h-2ZM17 16h2v1h-2ZM5 17h3v1h-3ZM16 17h3v1h-3ZM6 18h3v1h-3ZM14 18h4v1h-4ZM8 19h8v1h-8ZM2 20h1v1h-1ZM10 20h4v1h-4ZM21 20h1v1h-1ZM1 21h3v1h-3ZM20 21h3v1h-3ZM2 22h1v1h-1ZM21 22h1v1h-1Z",
  "scene-bypass": "M11 2h2v9h-2ZM7 3h1v1h-1ZM16 3h1v1h-1ZM6 4h2v1h-2ZM16 4h2v1h-2ZM5 5h2v1h-2ZM17 5h2v1h-2ZM4 6h2v1h-2ZM18 6h2v1h-2ZM3 7h2v2h-2ZM19 7h2v2h-2ZM2 9h2v6h-2ZM20 9h2v6h-2ZM3 15h2v2h-2ZM19 15h2v2h-2ZM4 17h2v1h-2ZM18 17h2v1h-2ZM5 18h2v1h-2ZM17 18h2v1h-2ZM6 19h3v1h-3ZM15 19h3v1h-3ZM7 20h10v1h-10ZM9 21h6v1h-6Z",
  "stomp-bypass": "M19 4h2v1h-2ZM10 5h2v1h-2ZM18 5h2v1h-2ZM9 6h4v1h-4ZM16 6h3v1h-3ZM21 6h1v1h-1ZM8 7h10v1h-10ZM20 7h3v1h-3ZM6 8h12v1h-12ZM19 8h2v1h-2ZM5 9h12v1h-12ZM4 10h10v1h-10ZM19 10h3v1h-3ZM2 11h8v1h-8ZM15 11h4v1h-4ZM1 12h5v1h-5ZM11 12h4v1h-4ZM1 13h2v1h-2ZM8 13h3v1h-3ZM4 14h3v1h-3ZM1 15h3v1h-3ZM6 17h4v3h-4ZM3 20h10v3h-10Z",
  hold: "M11 1h2v2h-2ZM10 3h4v1h-4ZM7 4h10v1h-10ZM4 5h6v1h-6ZM12 5h9v2h-9ZM4 6h4v1h-4ZM4 7h3v1h-3ZM12 7h8v1h-8ZM3 8h3v1h-3ZM12 8h9v3h-9ZM3 9h2v2h-2ZM2 11h2v5h-2ZM12 11h10v2h-10ZM20 13h2v2h-2ZM19 15h3v1h-3ZM3 16h2v2h-2ZM19 16h2v1h-2ZM18 17h3v1h-3ZM4 18h2v1h-2ZM18 18h2v1h-2ZM5 19h3v1h-3ZM17 19h2v1h-2ZM6 20h4v1h-4ZM15 20h3v1h-3ZM7 21h10v1h-10ZM9 22h6v1h-6Z",
  latency: "M10 1h5v1h-5ZM7 2h10v1h-10ZM5 3h4v1h-4ZM16 3h3v1h-3ZM4 4h3v1h-3ZM17 4h3v1h-3ZM3 5h3v1h-3ZM18 5h3v1h-3ZM3 6h2v1h-2ZM11 6h2v6h-2ZM19 6h2v1h-2ZM2 7h2v1h-2ZM20 7h2v2h-2ZM2 8h1v1h-1ZM1 9h2v5h-2ZM21 9h1v1h-1ZM21 10h2v5h-2ZM11 12h3v1h-3ZM13 13h3v1h-3ZM2 14h1v1h-1ZM15 14h3v1h-3ZM2 15h2v2h-2ZM17 15h1v1h-1ZM21 15h1v1h-1ZM20 16h2v1h-2ZM3 17h2v1h-2ZM19 17h2v1h-2ZM3 18h3v1h-3ZM18 18h3v1h-3ZM4 19h3v1h-3ZM17 19h3v1h-3ZM5 20h3v1h-3ZM15 20h4v1h-4ZM7 21h10v1h-10ZM9 22h5v1h-5Z",
  headphones: {
    band: "M4 16v-4a8 8 0 0 1 16 0v4",
    left: { x: "2", y: "14", width: "5", height: "7", rx: "2" },
    right: { x: "17", y: "14", width: "5", height: "7", rx: "2" }
  },
  swap: "M16 3h2v1h-2ZM17 4h2v1h-2ZM18 5h2v1h-2ZM18 6h3v1h-3ZM1 7h21v1h-21ZM18 8h3v1h-3ZM18 9h2v1h-2ZM17 10h2v1h-2ZM16 11h2v1h-2ZM6 13h1v1h-1ZM5 14h2v1h-2ZM4 15h2v1h-2ZM2 16h3v1h-3ZM1 17h22v1h-22ZM2 18h3v1h-3ZM4 19h2v1h-2ZM5 20h2v1h-2ZM6 21h1v1h-1Z",
  "gig-view": "M0 0h11v2h-11ZM13 0h11v2h-11ZM0 2h4v2h-4ZM7 2h4v3h-4ZM13 2h3v7h-3ZM21 2h3v1h-3ZM22 3h2v1h-2ZM0 4h3v3h-3ZM18 4h1v1h-1ZM21 4h3v2h-3ZM5 5h1v1h-1ZM8 5h3v3h-3ZM18 6h1v1h-1ZM22 6h2v2h-2ZM0 7h2v2h-2ZM18 7h2v1h-2ZM4 8h3v1h-3ZM9 8h2v1h-2ZM21 8h3v1h-3ZM0 9h11v2h-11ZM13 9h11v2h-11ZM0 13h11v2h-11ZM13 13h11v2h-11ZM0 15h4v1h-4ZM8 15h3v2h-3ZM13 15h3v7h-3ZM21 15h3v1h-3ZM0 16h3v1h-3ZM22 16h2v5h-2ZM0 17h2v3h-2ZM5 17h6v1h-6ZM18 17h2v3h-2ZM4 18h7v2h-7ZM0 20h3v2h-3ZM5 20h2v1h-2ZM8 20h3v2h-3ZM18 20h1v1h-1ZM21 21h3v1h-3ZM0 22h5v1h-5ZM6 22h5v1h-5ZM13 22h11v2h-11ZM0 23h11v1h-11Z",
  device: "M2 0h20v1h-20ZM1 1h22v1h-22ZM0 2h2v20h-2ZM22 2h2v20h-2ZM7 5h10v5h-10ZM5 13h2v2h-2ZM11 13h2v2h-2ZM17 13h2v2h-2ZM5 18h2v2h-2ZM11 18h2v2h-2ZM17 18h2v2h-2ZM1 22h22v1h-22ZM2 23h20v1h-20Z",
  about: "M9 2h1v3H9ZM8 5h3v2H8ZM7 7h4v1H7ZM7 8h2v1H7ZM10 8h2v2h-2ZM6 9h2v2H6ZM11 10h1v1h-1ZM4 11h3v2H4ZM11 11h2v2h-2ZM17 11h3v2h-3ZM12 13h1v1h-1ZM16 13h2v2h-2ZM12 14h2v2h-2ZM15 15h2v2h-2ZM13 16h1v1h-1ZM13 17h3v2h-3ZM13 19h2v1h-2ZM14 20h1v2h-1Z",
  info: "M9 1h6v1H9ZM7 2h10v1H7ZM5 3h14v1H5ZM4 4h16v1H4ZM3 5h8v2H3ZM13 5h8v2h-8ZM2 7h20v2H2ZM1 9h22v1H1ZM1 10h10v5H1ZM13 10h10v5H13ZM2 15h9v2H2ZM13 15h9v2h-9ZM3 17h8v2H3ZM13 17h8v2h-8ZM4 19h16v1H4ZM5 20h14v1H5ZM7 21h10v1H7ZM9 22h6v1H9Z",
  report: "M5 2h4v2H5ZM15 2h4v2h-4ZM8 4h8v2H8ZM7 6h2v2H7ZM15 6h2v2h-2ZM8 8h8v2H8ZM3 9h2v1H3ZM19 9h2v1h-2ZM3 10h3v1H3ZM8 10h2v1H8ZM14 10h2v1h-2ZM18 10h3v1h-3ZM5 11h4v1H5ZM15 11h4v1h-4ZM6 12h2v2H6ZM15 12h3v1h-3ZM16 13h2v1h-2ZM3 14h5v2H3ZM16 14h5v2h-5ZM6 16h2v2H6ZM16 16h2v2h-2ZM5 18h4v1H5ZM16 18h3v1h-3ZM3 19h3v1H3ZM7 19h2v1H7ZM15 19h2v1h-2ZM18 19h3v1h-3ZM3 20h2v1H3ZM8 20h2v1H8ZM14 20h2v1h-2ZM19 20h2v1h-2ZM9 21h6v1H9ZM10 22h4v1h-4Z",
  diagnostics: "M10 3h4v18h-4zM3 10h18v4H3z",
  licenses: "M5 2h8v1H5ZM4 3h10v1H4ZM4 4h11v1H4ZM4 5h12v1H4ZM4 6h13v1H4ZM4 7h14v1H4ZM4 8h15v1H4ZM4 9h16v1H4ZM4 10h2v2H4ZM11 10h9v2h-9ZM4 12h16v2H4ZM4 14h2v2H4ZM15 14h5v2h-5ZM4 16h16v2H4ZM4 18h2v2H4ZM18 18h2v2h-2ZM4 20h16v1H4ZM5 21h14v1H5Z",
  "scene-copy": { front: { x: "3", y: "7", width: "14", height: "14", rx: "2" }, back: "M7 7V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2h-2" },
  progress: { arc: "M12 3a9 9 0 1 1-6.36 2.64", head: "M5.64 2.8v2.84H2.8" },
  shift: "m4 11 8-8 8 8h-5v9H9v-9Z",
  "route-usb": "M12 21V3m0 0L8 7m4-4 4 4m-4 5H7m0 0-3-3m3 3-3 3m8 4h6m0 0v-4m0 4 3-3",
  "route-input": "M2 8h20m0 0-4-4m4 4-4 4M2 16h20m0 0-4-4m4 4-4 4",
  "route-row": "M2 7h20l-4-4m4 4-4 4M22 17H2l4-4m-4 4 4 4",
  "route-output": "M2 12h20m0 0-5-5m5 5-5 5",
  "capture-skip": "m6 5 7 7-7 7m6-14 7 7-7 7",
  "capture-step-check": "m5 12 4 4L19 7",
  lock: { shackle: "M4 10V7a8 8 0 0 1 16 0v3", body: { x: "2", y: "10", width: "20", height: "12", rx: "2" } }
} as const;

/** QcLooperActionGlyph: the looper's action marks. */
export const QC_LOOPER_ACTION_GLYPH_GEOMETRY = {
  duplicate: { loop: { cx: "8", cy: "12", r: "4.75" }, marks: "m5.5 9.5 5 5m0-5-5 5M16 7v10m-2 0h5" },
  "one-shot": { loop: { cx: "11.5", cy: "12", rx: "8.5", ry: "5.75" }, arrow: "m18 6.25 3.5.25-1.9 3" },
  "half-speed": { label: { x: "12", y: "16.4" } },
  "punch-in": { capsule: { x: "8", y: "4", width: "8", height: "12", rx: "4" }, stand: "M5.5 11.5v1a6.5 6.5 0 0 0 13 0v-1M12 19v3m-4 0h8" },
  record: { dot: { cx: "12", cy: "12", r: "7" } },
  play: "m7 4 12 8-12 8Z",
  reverse: "m12 5-8 7 8 7Zm8 0-8 7 8 7Z",
  undo: "M8 7H3v-5M4 7a9 9 0 1 1-1 10"
} as const;

/** QcCaptureTypeGlyph: the three capture-save tiles, in the tile's 40-unit box. */
export const QC_CAPTURE_TYPE_GLYPH_GEOMETRY = {
  "combo-amp-tile": {
    cabinet: { x: "6", y: "6", width: "28", height: "28", rx: "1" },
    panel: "M6 15h28",
    lamp: { cx: "29.5", cy: "10.5", r: "1.5" }
  },
  "amp-cab-tile": {
    head: "M8 11V5h23v6",
    cabinet: { x: "5", y: "13", width: "30", height: "22", rx: "1" },
    cone: { cx: "20", cy: "24", r: "7" },
    dustCap: { cx: "20", cy: "24", r: "3" }
  },
  "overdrive-tile": {
    body: { x: "10", y: "6", width: "19", height: "28", rx: "3" },
    jackLeft: { x: "8", y: "12", width: "1", height: "7" },
    jackRight: { x: "30", y: "12", width: "1", height: "7" },
    knobLeft: { cx: "14.5", cy: "10.5", r: "1.5" },
    knobMiddle: { cx: "19.5", cy: "10.5", r: "1.5" },
    knobRight: { cx: "24.5", cy: "10.5", r: "1.5" },
    cap: { cx: "19.5", cy: "19.5", r: "4.5" },
    notch: "M19.5 15v4.5",
    led: { cx: "19.5", cy: "28.5", r: "1.5" }
  }
} as const;

/** QcWifiSignalGlyph: six 2px bars on a 4px pitch, rising in threes from one baseline. */
export const QC_WIFI_SIGNAL_GEOMETRY = {
  bars: [0, 1, 2, 3, 4, 5],
  pitch: 4,
  baseline: 15,
  rise: 3,
  minHeight: 3,
  width: "2"
} as const;

/** QcDeviceWaveformGlyph: the zigzag the plugin directory and empty browser pane draw. */
export const QC_DEVICE_WAVEFORM_GLYPH = { path: "M0 70H18L42 2 75 138 94 70H120" } as const;

/** QcDeviceEmptySlotGlyph: the slot plate, in its 70-unit box. */
export const QC_DEVICE_EMPTY_SLOT_PLATE = { width: "70", height: "70", rx: "14" } as const;

/** QcDirectoryBroomIcon: the Directory multi-select sweep mark. */
export const QC_DIRECTORY_BROOM_PATHS = {
  handle: "M4 20 11 13",
  head: "m12 9 6 5-4 5-7-4z"
} as const;
