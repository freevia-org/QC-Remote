/**
 * Where the Tempo screen's VOLUME dial sits for a displayed dB.
 *
 * The unit's dial is turned by the raw 0..1 parameter, not by dB: 0.0 dB is at
 * raw 0.613 (about 1 o'clock), not at 87% of the sweep a linear -60..+9 scale
 * would put it. The gateway reports the dB the unit shows, so the dial needs
 * the same taper to land where the unit draws it. These are the knots the
 * Rust side measured on hardware (packages/rust/quadcortex/src/commands.rs,
 * TEMPO_VOLUME_KNOTS), as `[raw, dB]`; tests/tempo-volume-taper.test.ts keeps
 * the two tables identical.
 */
export const TEMPO_VOLUME_KNOTS: ReadonlyArray<readonly [number, number]> = [
  [0.000, -60.0],
  [0.050, -30.7],
  [0.150, -19.9],
  [0.250, -13.6],
  [0.350, -8.9],
  [0.435, -5.6],
  [0.500, -3.4],
  [0.550, -1.8],
  [0.613, 0.0],
  [0.650, 1.0],
  [0.725, 2.9],
  [0.800, 4.7],
  [0.870, 6.3],
  [0.900, 7.0],
  [0.950, 8.0],
  [1.000, 9.0]
];

/** The dial position, 0..1 along the sweep, for a dB (clamped to -60..+9). */
export function tempoVolumePosition(db: number): number {
  const last = TEMPO_VOLUME_KNOTS.length - 1;
  const clamped = Math.max(TEMPO_VOLUME_KNOTS[0][1], Math.min(TEMPO_VOLUME_KNOTS[last][1], db));
  for (let index = 1; index <= last; index += 1) {
    const [x1, y1] = TEMPO_VOLUME_KNOTS[index];
    if (clamped <= y1) {
      const [x0, y0] = TEMPO_VOLUME_KNOTS[index - 1];
      return x0 + (x1 - x0) * (clamped - y0) / (y1 - y0);
    }
  }
  return 1;
}
