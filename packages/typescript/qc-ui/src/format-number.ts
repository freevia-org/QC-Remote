/**
 * The one place a fixed-decimal reading is formatted.
 *
 * `Number.prototype.toFixed` keeps the sign of a value that rounds to zero, so -0.04 and -0.0004 print as "-0.0".
 * The unit never draws a negative zero ("0.0 dB"), so every readout that formats a number goes through here.
 * Non-finite values are returned as the platform prints them; callers decide whether to show them at all.
 */
export function formatFixed(value: number, digits = 1): string {
  const rendered = value.toFixed(digits);
  return Number(rendered) === 0 ? (0).toFixed(digits) : rendered;
}

/** A decibel reading as the unit draws it: one decimal, a unit, never a negative zero. */
export function formatDb(value: number, digits = 1): string {
  return `${formatFixed(value, digits)} dB`;
}
