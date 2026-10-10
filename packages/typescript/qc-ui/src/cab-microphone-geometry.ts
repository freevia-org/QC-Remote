/**
 * Where the unit puts a cab microphone's puck inside its speaker box.
 *
 * Measured on a physical Cab editor (Position 2.8 / Distance 0.5 on the left
 * mic, Position 1.5 / Distance 2.5 on the right) and on the corpus frame that
 * shows Position 4.0 / Distance 0.0: the puck leaves the speaker's centre, not
 * its top-left corner. Position slides it sideways away from the centre by
 * 11.2 px per display unit in the 241 px box (4.64% of it) and Distance slides
 * it down by the same, with the range 0..10 spanning 46.5% of the box either
 * way. The left mic approaches from the left (trail dots point to the centre,
 * `direction: "right"`) and the right one is its mirror image.
 */
export const CAB_MICROPHONE_TRAVEL = 46.5;

export type CabMicrophoneSide = 0 | 1;

const clamp01 = (value: number) => Math.min(1, Math.max(0, value));

/** Puck centre in percent of the speaker box, from normalised 0..1 values. */
export function cabMicrophonePlacement(side: CabMicrophoneSide, position: number, distance: number): { x: number; y: number } {
  const direction = side === 0 ? -1 : 1;
  return { x: 50 + direction * CAB_MICROPHONE_TRAVEL * clamp01(position), y: 50 + CAB_MICROPHONE_TRAVEL * clamp01(distance) };
}

/** The inverse, from a pointer given as 0..1 of the speaker box. */
export function cabMicrophoneValuesAt(side: CabMicrophoneSide, fractionX: number, fractionY: number): { position: number; distance: number } {
  const direction = side === 0 ? -1 : 1;
  return {
    position: clamp01((direction * (fractionX * 100 - 50)) / CAB_MICROPHONE_TRAVEL),
    distance: clamp01((fractionY * 100 - 50) / CAB_MICROPHONE_TRAVEL)
  };
}
