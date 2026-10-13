import type { DeviceImage } from "@qc-remote/client";

/**
 * What the app can read of the unit's Copy Scene / Swap Scene dialog (zenUI::CopySceneDialog).
 *
 * The unit uses one class for both commands and its graphics tree carries only the dialog's CANCEL button: the title
 * ("Copy Scene A" / "Swap Scene A") and the prompt are drawn without a text node (measured on 4.1.0, the tree of
 * both commands is byte for byte the same). So the two facts the panel needs come from the two places they exist:
 *
 *  - which scene is the source: the Grid's own scene indicator, which stays in the tree under the dialog, and
 *  - which command it is: the first letter of the title in the unit's captured frame, "C" of Copy or "S" of Swap.
 *    The title is centred and white on the dialog's grey; the first glyph is the same pixels for every scene letter,
 *    only its x position moves, and its left edge differs: the C is lit down the middle of its left side, the S is
 *    not. A frame that is neither (mid-animation, another screen) reads as undefined, never as a guess.
 */

/** The unit's scene letters, A to H, as scene indexes. */
const SCENE_LETTERS = "ABCDEFGH";

/** The active scene, read from the Grid's scene indicator (`zenUI::SceneIndicator` and the single letter inside it). */
export function sceneIndexFromTree(tree: string): number | undefined {
  const lines = tree.split(/\r?\n/);
  const depth = (line: string) => line.length - line.trimStart().length;
  const start = lines.findIndex((line) => /^\s*zenUI::SceneIndicator\s*$/.test(line));
  if (start < 0) return undefined;
  const base = depth(lines[start]);
  for (let index = start + 1; index < lines.length; index += 1) {
    const line = lines[index];
    // A text value may hold a line break of its own; only a class line can end the indicator's subtree.
    if (/^\s*zenUI::/.test(line) && depth(line) <= base) return undefined;
    const letter = line.match(/^\s*text\s*:\s*'([A-H])'\s*$/)?.[1];
    if (letter) return SCENE_LETTERS.indexOf(letter);
  }
  return undefined;
}

export type SceneCommandVerb = "copy" | "swap";

/** A frame as grey levels, 0 to 255, row by row. */
export interface UnitFrameLuma { width: number; height: number; luma: ArrayLike<number> }

// The unit's own pixels (the frame is always 800x480): the dialog's title sits in rows 172 to 189 and is centred
// on x = 400 inside the dialog (x 190 to 610); the band below covers the title of every scene letter.
const BAND = { top: 160, bottom: 200, left: 300, right: 500 } as const;
const INK = 110;

/**
 * Whether the frame shows the dialog itself. A captured frame can lag the tree by a moment and still show the Grid; the
 * dialog's own pixels are exact (measured on 4.1.0): its grey margin either side of the CANCEL button (x 190 to 222 and
 * 578 to 610) and the button's black, on a row below the button's text. No other frame of the unit's corpus has them.
 */
function showsDialog({ width, luma }: UnitFrameLuma): boolean {
  const row = 282 * width;
  for (let x = 230; x <= 570; x += 8) if (luma[row + x] > 3) return false;
  for (const x of [196, 200, 204, 208, 212, 216, 584, 588, 592, 596, 600, 604]) if (luma[row + x] < 12 || luma[row + x] > 22) return false;
  return true;
}

/** "copy" or "swap" from the title's first letter, or undefined when the frame does not show a title that reads clearly. */
export function sceneCommandVerbFromLuma(frame: UnitFrameLuma): SceneCommandVerb | undefined {
  const { width, height, luma } = frame;
  if (width !== 800 || height !== 480 || !showsDialog(frame)) return undefined;
  const lit = (x: number, y: number) => luma[y * width + x] > INK;
  let left = -1;
  for (let x = BAND.left; x < BAND.right && left < 0; x += 1) for (let y = BAND.top; y < BAND.bottom; y += 1) if (lit(x, y)) { left = x; break; }
  if (left < 0) return undefined;
  let top = -1;
  for (let y = BAND.top; y < BAND.bottom && top < 0; y += 1) for (let x = left; x < left + 14; x += 1) if (lit(x, y)) { top = y; break; }
  if (top < 0) return undefined;
  // The middle of the first glyph's left edge: four rows, two columns.
  let ink = 0;
  for (let y = top + 6; y < top + 10; y += 1) for (let x = left; x < left + 2; x += 1) if (lit(x, y)) ink += 1;
  if (ink >= 6) return "copy";
  if (ink <= 1) return "swap";
  return undefined;
}

/** Decodes a PNG into grey levels. The default uses the browser's own decoder (both hosts are web views). */
export type UnitFrameDecoder = (pngBase64: string) => Promise<UnitFrameLuma | undefined>;

export const decodeUnitFrame: UnitFrameDecoder = async (pngBase64) => {
  let bitmap: ImageBitmap | undefined;
  try {
    const bytes = Uint8Array.from(atob(pngBase64), (character) => character.charCodeAt(0));
    bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    const canvas = typeof OffscreenCanvas === "function" ? new OffscreenCanvas(bitmap.width, bitmap.height) : Object.assign(document.createElement("canvas"), { width: bitmap.width, height: bitmap.height });
    const context = canvas.getContext("2d") as CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null;
    if (!context) return undefined;
    context.drawImage(bitmap, 0, 0);
    const { data } = context.getImageData(0, 0, bitmap.width, bitmap.height);
    const luma = new Uint8Array(bitmap.width * bitmap.height);
    for (let pixel = 0; pixel < luma.length; pixel += 1) luma[pixel] = (data[pixel * 4] * 77 + data[pixel * 4 + 1] * 150 + data[pixel * 4 + 2] * 29) >> 8;
    return { width: bitmap.width, height: bitmap.height, luma };
  } catch {
    return undefined;
  } finally {
    bitmap?.close();
  }
};

/** Reads Copy or Swap from a captured frame. */
export async function sceneCommandVerbFromFrame(frame: Pick<DeviceImage, "pngBase64">, decode: UnitFrameDecoder = decodeUnitFrame): Promise<SceneCommandVerb | undefined> {
  const decoded = await decode(frame.pngBase64);
  return decoded ? sceneCommandVerbFromLuma(decoded) : undefined;
}
