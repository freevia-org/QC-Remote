/**
 * Drawn pointer angle of the master-volume knob, in degrees clockwise from
 * 12 o'clock, as a function of the unit's 0..100 display value.
 *
 * Measured, not eyeballed: 19 labelled photographs of a real Quad Cortex's
 * physical knob, values 5 to 70 in steps of 5 (plus 66) and 75 to 95 in steps
 * of 5. The pointer tick was located in each photo by image analysis and its
 * direction corrected for the camera's foreshortening of the knob face (face
 * width/height about 0.93). The photos at displayed 0 and 100 are excluded:
 * past those readouts the knob keeps turning with the value pinned, so the
 * pointer there is not a point on the curve.
 *
 * Linear versus quadratic over the 19 unclamped points: the straight line
 * (2.44 degrees per unit, 12 o'clock at value 52.5) has max residual 5.5 and
 * rms 2.8 with a systematic over/under/over pattern, and leave-one-out rms
 * 3.1; the quadratic below has max residual 2.9, rms 1.5 and leave-one-out
 * rms 1.8 (F about 38 for the one extra term). Local slope rises from about
 * 2.3 degrees per unit at 5..30 to about 2.5-2.7 at 55..95. Curvature
 * survives dropping the 95 and 100 points and a +-0.05 change of the
 * foreshortening ratio, but it was not tested against per-photo tilt, so it
 * may partly be perspective. Each point is good to about +-2 degrees. The
 * curve crosses 12 o'clock at value 53.5 and draws -122.1 at 0 and +122.2 at
 * 100; the clamped photos read about -124 (0) and +118.5 (100).
 *
 * The knob turns freely past both ends (about 28 degrees past 0 and 31 past
 * 100 in photos whose value did not change), so the drawn angle clamps the
 * value to the edge of the live range and never extrapolates.
 */
export const MASTER_VOLUME_ANGLE_AT_ZERO_DEGREES = -122.107;
export const MASTER_VOLUME_ANGLE_LINEAR_DEGREES_PER_UNIT = 2.0938;
export const MASTER_VOLUME_ANGLE_QUADRATIC_DEGREES_PER_UNIT_SQUARED = 0.00349;
export const MASTER_VOLUME_MIN = 0;
export const MASTER_VOLUME_MAX = 100;

export function masterVolumeAngleDegrees(value: number): number {
  const clamped = Number.isFinite(value) ? Math.min(MASTER_VOLUME_MAX, Math.max(MASTER_VOLUME_MIN, value)) : MASTER_VOLUME_MIN;
  return MASTER_VOLUME_ANGLE_AT_ZERO_DEGREES
    + MASTER_VOLUME_ANGLE_LINEAR_DEGREES_PER_UNIT * clamped
    + MASTER_VOLUME_ANGLE_QUADRATIC_DEGREES_PER_UNIT_SQUARED * clamped * clamped;
}

/** Inverse of {@link masterVolumeAngleDegrees} on 0..100 (the curve is strictly increasing there). */
export function masterVolumeFromAngleDegrees(angle: number): number {
  const a = MASTER_VOLUME_ANGLE_QUADRATIC_DEGREES_PER_UNIT_SQUARED;
  const b = MASTER_VOLUME_ANGLE_LINEAR_DEGREES_PER_UNIT;
  const c = MASTER_VOLUME_ANGLE_AT_ZERO_DEGREES - angle;
  const value = (-b + Math.sqrt(b * b - 4 * a * c)) / (2 * a);
  return Math.min(MASTER_VOLUME_MAX, Math.max(MASTER_VOLUME_MIN, value));
}
