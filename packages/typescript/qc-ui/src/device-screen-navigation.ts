/**
 * Device-screen gestures shared by every host. These are the same physical
 * paths the Quad Cortex uses; hosts project the resulting screen locally only
 * after the hardware gesture succeeds. Demo sessions project it directly.
 */
export const QC_DEVICE_SCREEN_GESTURES = {
  openIo: { x: 400, y: 8, toX: 400, toY: 220 },
  done: { x: 744, y: 30 }
} as const;
