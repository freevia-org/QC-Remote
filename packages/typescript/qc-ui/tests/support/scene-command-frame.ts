/**
 * The first glyph of the unit's Copy Scene / Swap Scene title as it is drawn in a captured 800x480 frame on 4.1.0
 * (the C of "Copy", the S of "Swap"), 14 rows from the glyph's top, so a test can paint a frame the reader reads.
 * "#" is lit, "+" is a faint edge, "." is the dialog's grey.
 */
const GLYPHS = {
  copy: ["....#####+..", "...###++###.", "..##.....##+", ".+#+......+.", ".##.........", ".##.........", ".##.........", ".##.........", ".##.........", ".##.........", ".+#+......#.", "..##.....+#+", "..+##+++###.", "....#####+.."],
  swap: ["...######...", "..##+++###..", ".##.....+#..", ".##.........", ".##.........", "..##+.......", "..+#####....", ".....#####..", "........##..", ".........#+.", ".........##.", ".##.....+#+.", ".+##+++###..", "..+######..."]
} as const;

/**
 * A frame of the unit with its dialog open: the dimmed Grid, the dialog's 420x230 box in its grey at (190,125), its black
 * CANCEL button at (222,278), and the title's first glyph with its left edge at `x` (the glyph's column 1), top at row 172.
 * `dialog: false` is the same pixels without the dialog (the Grid, a frame that lags the tree).
 */
export function titleFrame(verb: keyof typeof GLYPHS | "none", x = 339, dialog = true): Uint8Array {
  const luma = new Uint8Array(800 * 480).fill(60);
  if (dialog) {
    for (let y = 125; y < 355; y += 1) luma.fill(16, y * 800 + 190, y * 800 + 610);
    for (let y = 278; y < 323; y += 1) luma.fill(0, y * 800 + 222, y * 800 + 578);
  }
  if (verb === "none") return luma;
  GLYPHS[verb].forEach((row, dy) => [...row].forEach((cell, dx) => { luma[(172 + dy) * 800 + (x - 1 + dx)] = cell === "#" ? 255 : cell === "+" ? 100 : 16; }));
  return luma;
}
