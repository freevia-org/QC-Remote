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
  settingsRow: { x: 600, y: 406 },
  /** The Tempo row of the preset menu, which opens scrolled to the bottom (measured on 4.1.0). Its screen closes on the check button (743,30). */
  tempoRow: { x: 610, y: 286 },
  /** The CPU Monitor row of the same menu. The monitor is an overlay on the Grid and closes only on its own X, not the check button. */
  cpuRow: { x: 578, y: 346 },
  cpuClose: { x: 628, y: 32 },
  /** The preset title area of the Grid: opens the Preset Directory, which the check button (743,30) closes. The app never touches the directory's rows, banks or cloud button. */
  directoryOpen: { x: 320, y: 50 },
  /** The scene indicator: opens the scene popup (an overlay beside the Grid) and, tapped again, closes it. The app never touches a scene row. */
  sceneIndicator: { x: 668, y: 23 },
  /** The first row's input rail label: opens a popup list of inputs over the Grid and, tapped again, closes it. The app never touches a row of it (a pick rewrites the routing). The output label is not measured and not routed. */
  inputRail: { x: 30, y: 147 },
  /** The first row's output rail label, the same pattern as the input label: opens the picker, tapped again closes it. Never a row. */
  outputRail: { x: 772, y: 147 },
  /** Drags that scroll the preset menu, which remembers its scroll: the first (measured) shows its top rows, the reverse restores the bottom rows every other route taps. */
  menuScrollTop: { x: 600, y: 150, toX: 600, toY: 420 },
  menuScrollBottom: { x: 600, y: 420, toX: 600, toY: 150 },
  /** Rows of the menu scrolled to the top. Preset MIDI Out's dialog closes on the check (743,30); the delete button beside it is never touched. */
  midiOutRow: { x: 580, y: 406 },
  copySceneRow: { x: 582, y: 286 },
  swapSceneRow: { x: 582, y: 346 },
  /** Copy and Swap Scene share one dialog that waits for a footswitch; the app only ever taps CANCEL. */
  copySceneCancel: { x: 400, y: 300 },
  /** In the Directory: the search icon opens the search dialog (keyboard), whose X at the top left closes it (the check does nothing there). */
  directorySearch: { x: 634, y: 30 },
  searchClose: { x: 57, y: 30 },
  /** The kebab of a block editor that is a bottom panel over the Grid opens its context menu; an empty area outside the popup closes it. The app never touches a row. */
  blockMenuButton: { x: 30, y: 226 },
  blockMenuDismiss: { x: 450, y: 215 }
} as const;
