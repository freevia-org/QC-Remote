import { formatFixed } from "./format-number.ts";
const clampNormalized = (value: number) => Math.max(0, Math.min(1, value));

/** CorOS stores Global EQ frequency as a logarithmic 20 Hz–20 kHz value. */
export function globalEqFrequencyHz(normalized: number) {
  return 20 * Math.pow(1000, clampNormalized(normalized));
}

export function formatGlobalEqFrequency(normalized: number) {
  const hz = globalEqFrequencyHz(normalized);
  if (hz < 1000) return `${Math.round(hz)} Hz`;
  const khz = hz / 1000;
  return `${khz < 10 ? formatFixed(khz) : Math.round(khz)} kHz`;
}

/** CorOS stores Global EQ Q as a normalized 0.10–10.00 value. */
export function globalEqQ(normalized: number) {
  return 0.1 + clampNormalized(normalized) * 9.9;
}
