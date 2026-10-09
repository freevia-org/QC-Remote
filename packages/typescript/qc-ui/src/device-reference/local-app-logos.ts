/**
 * Third-party application marks drawn by `QcLocalAppLogo` in
 * `../theme-icons.tsx`, kept as data alone.
 *
 * Each mark belongs to its owner and identifies the linked desktop app only;
 * none of it is QC Remote or CorOS iconography. The component that draws these,
 * and the notes on where each mark comes from, stay in theme-icons.tsx. The
 * values are moved here unchanged: geometry, gradient stops and bar layout.
 */
export const QC_LOCAL_APP_LOGO_MARKS = {
  claude: { path: "M10.5 2h3l-.5 6.2 4.9-3.8 1.5 2.6-5.6 2.7 6.1 1.4-.6 2.9-5.8-2.3 3.2 5.3-2.4 1.8-3.7-5 0 6.2h-3l.6-6.2-5 3.8L2.7 15l5.6-2.8-6.1-1.3.6-2.9 5.8 2.3L5.4 5l2.4-1.8 3.7 5Z" },
  antigravity: { path: "M55 427c49-43 68-105 88-183C168 145 199 69 266 69c69 0 102 77 128 176 20 77 39 139 88 182 12 11 6 31-11 31-65-1-104-69-140-132-19-33-38-48-65-48-26 0-47 15-66 48-36 63-75 131-140 132-17 0-23-20-5-31Z" },
  other: { path: "M8 3v5H5a3 3 0 0 0-3 3v2a3 3 0 0 0 3 3h3v5h3v-5h2v5h3v-5h3a3 3 0 0 0 3-3v-2a3 3 0 0 0-3-3h-3V3h-3v5h-2V3H8Zm-3 8h14v2H5v-2Z" },
  chatgpt: { path: "M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9847 5.9847 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.073Zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944Zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464ZM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.8956Zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.6669Zm2.0107-3.0231-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66ZM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813Zm1.0976-2.3654 2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z" }
} as const;

/** Antigravity's arch: one horizontal sweep across the mark's own palette. */
export const QC_ANTIGRAVITY_GRADIENT_STOPS = [
  { offset: "0", stopColor: "var(--qc-palette-2e8cef)" },
  { offset: "0.2", stopColor: "var(--qc-palette-4ca8c0)" },
  { offset: "0.36", stopColor: "var(--qc-palette-9ec87a)" },
  { offset: "0.47", stopColor: "var(--qc-palette-efd07f)" },
  { offset: "0.54", stopColor: "var(--qc-palette-f57d37)" },
  { offset: "0.68", stopColor: "var(--qc-palette-f2867f)" },
  { offset: "0.82", stopColor: "var(--qc-palette-7d89cf)" },
  { offset: "1", stopColor: "var(--qc-palette-267eff)" }
] as const;

/** Bionic's bar geometry and gradient pairs; see QcLocalAppLogo for the scale. */
export const QC_BIONIC_BARS = [
  { y: 5, x: 3.75, from: "71aaff", to: "2a6eda" },
  { y: 8.75, x: 6.75, from: "00c9cf", to: "009096" },
  { y: 12.5, x: 3.75, from: "f96550", to: "d2351f" },
  { y: 16.25, x: 6.75, from: "b762de", to: "8b35b4" },
] as const;

/** Each Bionic bar's size and corner, on the 24-unit box. */
export const QC_BIONIC_BAR_SIZE = { width: "13.5", height: "3", rx: "1.5" } as const;

/** U+1F44B, the mark Jan ships as its own app icon. */
export const JAN_WAVING_HAND = "\u{1F44B}";
