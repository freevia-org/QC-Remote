import { QC_MAXIMUM_TEMPO_BPM, QC_MINIMUM_TEMPO_BPM } from "@qc-remote/client";

export interface TapTempoResult {
  taps: number[];
  status: "need-more" | "invalid" | "ready";
  bpm?: number;
}

/**
 * Keep the visual beat origin on the beat messages CorOS pushes.
 *
 * Observed on a CorOS 4.1.0 unit on 2026-10-07 (metronome off): the unit
 * pushes one status message at the start of every beat, with `current_tick`
 * absent or `0`, and a second one about 23 ms later (a twentieth of a beat, so
 * about 14 ms at 212 BPM) with `current_tick` `1`. The tick is a pulse flag,
 * not a position inside the beat: no ticks-per-beat figure applies and a beat
 * position cannot be derived from it. Earlier code divided it by 24, which
 * placed the lamp up to a whole beat off.
 *
 * So only a beat-start message (tick `0`) carries a beat origin: the epoch is
 * the instant that message was stamped, taken to the nearest beat boundary of
 * the current epoch so a lamp already running keeps its beat count. Any
 * non-zero tick is a pulse flag and leaves the epoch as it was; before the
 * first beat message has been seen there is no epoch to leave, so the result
 * is `undefined` and the lamp waits for the next beat (at most one beat, about
 * 1.5 s at the slowest tempo).
 *
 * The renderer stamps each message when its native USB event arrives, so the
 * beat message's arrival is the closest shared clock available to the physical
 * lamp. It is applied in full rather than smoothed: even a few milliseconds of
 * smoothing leaves a stable visible phase error against the hardware.
 */
export function synchronizeTempoPulseEpoch(
  currentEpoch: number | undefined,
  observedAt: number,
  currentTick: number,
  bpm: number
): number | undefined {
  const validEpoch = currentEpoch !== undefined && Number.isFinite(currentEpoch)
    ? currentEpoch
    : undefined;
  if (Math.trunc(currentTick) !== 0) return validEpoch;
  if (validEpoch === undefined) return observedAt;

  const safeBpm = Math.max(QC_MINIMUM_TEMPO_BPM, Math.min(QC_MAXIMUM_TEMPO_BPM, bpm));
  const period = 60_000 / safeBpm;
  const rawDelta = observedAt - validEpoch;
  const phaseDelta = ((rawDelta + period / 2) % period + period) % period - period / 2;
  return validEpoch + phaseDelta;
}

export function recordTempoTap(previousTaps: readonly number[], now: number): TapTempoResult {
  const previous = previousTaps.at(-1);
  const taps = previous === undefined || now - previous > 2500
    ? [now]
    : [...previousTaps.slice(-4), now];
  if (taps.length < 2) return { taps, status: "need-more" };
  const intervals = taps.slice(1).map((time, index) => time - taps[index]);
  const usable = intervals.filter((interval) => interval >= 250 && interval <= 1500);
  if (!usable.length) return { taps: [now], status: "invalid" };
  const average = usable.reduce((sum, interval) => sum + interval, 0) / usable.length;
  return { taps, status: "ready", bpm: Math.round(Math.max(QC_MINIMUM_TEMPO_BPM, Math.min(QC_MAXIMUM_TEMPO_BPM, 60000 / average))) };
}
