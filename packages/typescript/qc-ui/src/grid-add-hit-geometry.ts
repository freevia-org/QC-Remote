/** Empty-cell hit targets in the fixed 800x480 canvas coordinate space. The
 * canvas root scales as a unit, so these logical canvas pixels scale with the
 * rendered display while staying independent of AT-SPI's percentage sizing. */
export const GRID_ADD_HIT_SIZE = 70;

export function gridAddHitOrigin(centerX: number, centerY: number): { left: number; top: number } {
  const halfSize = GRID_ADD_HIT_SIZE / 2;
  return { left: centerX - halfSize, top: centerY - halfSize };
}
