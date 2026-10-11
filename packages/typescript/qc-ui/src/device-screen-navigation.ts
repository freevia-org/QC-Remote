/**
 * Device-screen gestures shared by every host: how the Quad Cortex's own touch
 * screen opens and closes I/O Settings, the only device screen with no command.
 * Observed on CorOS 4.1.0 (direct USB, 2026-10-06 and 2026-10-11): the unit
 * opens I/O Settings for ONE drag frame that travels more than about 245 px
 * downward on the Grid, from anywhere (shorter drags do nothing), and closes
 * it for ONE tap on the check button. The unit's screen is read from its
 * graphics tree (the I/O dialog's root is zenUI::CheatSheetDialog); no state
 * event announces it.
 */
export const QC_DEVICE_SCREEN_GESTURES = {
  openIo: { x: 400, y: 100, toX: 400, toY: 440 },
  done: { x: 743, y: 30 },
  /** The kebab at the top right of the Grid: opens and closes the preset menu (10 of 10 on 4.1.0). */
  menuButton: { x: 764, y: 22 },
  /** A long press on the mode indicator opens Modes Configuration. */
  modesHold: { x: 700, y: 70 },
  /** The Settings row of the preset menu when it opens scrolled to the bottom. Audited, not re-run on the unit. */
  settingsRow: { x: 600, y: 406 }
} as const;
