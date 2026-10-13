import { inflateSync } from "node:zlib";

/** Decodes an 8-bit, non-interlaced RGB or RGBA PNG (what a browser screenshot is) to RGBA bytes. */
export function decodePng(png: Buffer): { width: number; height: number; data: Uint8Array } {
  let offset = 8, width = 0, height = 0, channels = 4;
  const parts: Buffer[] = [];
  while (offset < png.length) {
    const length = png.readUInt32BE(offset), type = png.toString("ascii", offset + 4, offset + 8), body = png.subarray(offset + 8, offset + 8 + length);
    if (type === "IHDR") {
      width = body.readUInt32BE(0); height = body.readUInt32BE(4);
      if (body[8] !== 8 || body[12] !== 0 || (body[9] !== 2 && body[9] !== 6)) throw new Error("only 8-bit non-interlaced RGB/RGBA PNGs are decoded");
      channels = body[9] === 6 ? 4 : 3;
    } else if (type === "IDAT") parts.push(body);
    offset += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(parts));
  const stride = width * channels, out = new Uint8Array(width * height * 4), previous = new Uint8Array(stride), line = new Uint8Array(stride);
  for (let y = 0; y < height; y += 1) {
    const filter = raw[y * (stride + 1)], start = y * (stride + 1) + 1;
    for (let x = 0; x < stride; x += 1) {
      const a = x >= channels ? line[x - channels] : 0, b = previous[x], c = x >= channels ? previous[x - channels] : 0, value = raw[start + x];
      const paeth = () => { const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c); return pa <= pb && pa <= pc ? a : pb <= pc ? b : c; };
      line[x] = (value + (filter === 0 ? 0 : filter === 1 ? a : filter === 2 ? b : filter === 3 ? (a + b) >> 1 : paeth())) & 255;
    }
    previous.set(line);
    for (let x = 0; x < width; x += 1) for (let k = 0; k < 4; k += 1) out[(y * width + x) * 4 + k] = k < channels ? line[x * channels + k] : 255;
  }
  return { width, height, data: out };
}

/** How many pixels of a rectangle (device pixels) differ between two decoded screenshots, and by how much at most. */
export function diffRegion(a: { width: number; data: Uint8Array }, b: { width: number; data: Uint8Array }, [left, top, width, height]: number[]): { pixels: number; max: number } {
  let pixels = 0, max = 0;
  for (let y = Math.floor(top); y < Math.ceil(top + height); y += 1) for (let x = Math.floor(left); x < Math.ceil(left + width); x += 1) {
    let worst = 0;
    for (let k = 0; k < 3; k += 1) worst = Math.max(worst, Math.abs(a.data[(y * a.width + x) * 4 + k] - b.data[(y * b.width + x) * 4 + k]));
    if (worst) { pixels += 1; max = Math.max(max, worst); }
  }
  return { pixels, max };
}
