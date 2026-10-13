/**
 * Every popup, menu, drawer, dialog and overlay the display can draw over its screen, as a fixture the
 * containment test renders and measures (the props that open each are in popup-props.tsx). A surface prop that
 * opens one must be named here with a fixture; popup-containment.test.ts fails on any other.
 *
 * `selectors` are the elements to measure (the popup itself and its scrim or dismiss layer); `fraction` is the
 * unit's own geometry where the corpus or the manual fixes it: the popup's size as a fraction of the 800x480
 * screen, within `tolerance`.
 */
export interface PopupFixture {
  id: string;
  /** The surface prop that opens it. */
  propKey: string;
  selectors: string[];
  fraction?: { width?: number; height?: number; centered?: boolean; tolerance: number };
  /** Measure at the first device model only (the Settings panes are the same whatever the chassis). */
  firstModelOnly?: boolean;
  /**
   * Every control the app draws to dismiss or cancel this popup, and every scrim that dismisses it by a tap beside it.
   * Each is hit-tested with document.elementFromPoint (a scrim at the points it covers): what is topmost there must be
   * the control itself, because a Grid hit layer stacked above it takes the click and does something else (the CPU
   * Monitor's X once pressed Undo, and later opened the Directory). The test also finds every close, cancel, done and
   * scrim element in the rendering on its own, so a control added later is tested whether or not it is listed here.
   */
  closeControls?: CloseControl[];
  /** Why the app draws nothing to close this popup (it is closed by the unit, or by the screen it is part of). */
  closedBy?: string;
  /** Rendered through the shared display with these host values (see display-host-entry.tsx) instead of the surface props. */
  scenario?: { view: string | null; open?: "deviceSettings" | "tempo"; popup?: "grid-menu" | "scene-menu" | "mode-menu" | null };
  /** Only the close controls are measured: the screen's size and text are held to the unit by the full-screen tests. */
  closeOnly?: boolean;
}

/** A control or scrim the registry names: matched by CSS selector, and by exact text when the label is not an aria-label. */
export interface CloseControl { selector: string; text?: string; scrim?: boolean }

const close = (selector: string, text?: string): CloseControl => ({ selector, ...(text ? { text } : {}) });
const scrim = (selector: string): CloseControl => ({ selector, scrim: true });

export const POPUP_FIXTURES: PopupFixture[] = [
  { id: "preset-menu", propKey: "popup", selectors: [".coros-screen-menu", ".coros-screen-dimmer"], closeControls: [scrim(".coros-screen-dimmer")] },
  { id: "scene-menu", propKey: "popup", selectors: [".scene-dropdown", ".scene-menu-backdrop"], closeControls: [scrim(".scene-menu-backdrop")] },
  { id: "mode-menu", propKey: "popup", selectors: [".scene-dropdown", ".scene-menu-backdrop"], closeControls: [scrim(".scene-menu-backdrop")] },
  { id: "input-picker", propKey: "routingPicker", selectors: [".coros-route-picker", ".coros-route-picker-dismiss"], closeControls: [scrim(".coros-route-picker-dismiss")] },
  { id: "output-picker", propKey: "routingPicker", selectors: [".coros-route-picker", ".coros-route-picker-dismiss"], closeControls: [scrim(".coros-route-picker-dismiss")] },
  { id: "add-device-drawer", propKey: "addDevicePicker", selectors: [".device-browser-official.is-live-picker"], closeControls: [scrim(".coros-add-device-dismiss"), close("button[aria-label=\"Close device browser\"]")] },
  // The unit's own dialog, measured on its 800x480 frame: 420x230 (52.5% x 47.9%) at (190,125), centred.
  { id: "copy-scene", propKey: "sceneCommand", selectors: [".scene-command-panel", ".scene-command-scrim"], fraction: { width: 0.525, height: 0.479, centered: true, tolerance: 0.02 }, closeControls: [close(".scene-command-panel button", "CANCEL")] },
  { id: "swap-scene", propKey: "sceneCommand", selectors: [".scene-command-panel", ".scene-command-scrim"], fraction: { width: 0.525, height: 0.479, centered: true, tolerance: 0.02 }, closeControls: [close(".scene-command-panel button", "CANCEL")] },
  { id: "scene-destination-picker", propKey: "sceneCommand", selectors: [".scene-command-panel", ".scene-command-scrim"], fraction: { width: 0.525, centered: true, tolerance: 0.02 }, closeControls: [close(".scene-command-panel button", "CANCEL")] },
  { id: "scene-label-editor", propKey: "sceneEditor", selectors: [".scene-label-dialog-panel"], fraction: { width: 1, height: 1, tolerance: 0.01 }, closeControls: [close("button[aria-label=\"Close\"]")] },
  { id: "cpu-monitor", propKey: "cpuMonitor", selectors: [".coros-cpu-overlay"], closeControls: [close("button[aria-label=\"Close CPU Monitor\"]")] },
  { id: "save-as", propKey: "savePreset", selectors: [".coros-save-preset"], closeControls: [close(".coros-save-preset button", "CANCEL")] },
  { id: "amp-editor-panel", propKey: "parameterEditor", selectors: [".coros-parameter-editor"], closeControls: [close("button[aria-label=\"Close parameter editor\"]")] },
  { id: "cab-editor-full-screen", propKey: "parameterEditor", selectors: [".coros-parameter-editor"], closeControls: [close("button[aria-label=\"Close parameter editor\"]")] },
  { id: "preset-directory", propKey: "presetDirectory", selectors: [".coros-directory"], closeControls: [close("button[aria-label=\"Return to Grid\"]")] },
  { id: "expression-assignment", propKey: "expressionAssignment", selectors: [".expression-bypass-official"], closeControls: [close("button[aria-label=\"Close expression assignment\"]"), close("button[aria-label=\"Done\"]")] },
  { id: "unit-mirror-picture", propKey: "screenContent", selectors: [".coros-unit-mirror", ".coros-unit-mirror > img"], fraction: { width: 1, height: 1, tolerance: 0.01 }, closedBy: "the unit: the picture is the unit's own screen, and a tap on it is sent to the unit" },
  { id: "lane-control-panel", propKey: "laneControlEditor", selectors: [".lane-control-panel", ".coros-lane-control-dismiss"], closeControls: [scrim(".coros-lane-control-dismiss"), close("button[aria-label=\"Close panel\"]")] }
];

const SETTINGS_PANES = { account: ["user", "backup"], system: ["wifi", "updates", "brightness", "power", "volume", "storage", "factory"], device: ["bypass", "scene-bypass", "stomp-bypass", "hold", "swap", "gig-access", "latency", "midi"], support: ["about", "info", "report", "diagnostics", "licenses"] } as const;
/** Every pane of the unit's Settings, as the app draws it (view-only): it must fill the display's box exactly, at the unit's 800x480 shape. */
for (const [section, rows] of Object.entries(SETTINGS_PANES)) {
  for (const row of rows) POPUP_FIXTURES.push({ id: `settings-${section}-${row}`, propKey: "screenContent", selectors: [".coros-settings-official"], fraction: { width: 1, height: 1, tolerance: 0.01 }, firstModelOnly: true, closeControls: [close('button[aria-label="Close Settings"]')] });
}

/**
 * The screens the app assembles from its shared display exactly as a host does (createQcDeviceDisplay, with a CPU reading, per-block loads
 * and an enabled Undo, as on a connected unit), each with the control that dismisses it: the header check, or the CPU Monitor's X.
 */
const SCREEN_CLOSES: Array<[string, NonNullable<PopupFixture["scenario"]>, string, string]> = [
  ["app-cpu-monitor", { view: "cpu-monitor" }, ".coros-cpu-overlay", "Close CPU Monitor"],
  ["app-tempo", { view: null, open: "tempo" }, ".coros-tempo", "Close Tempo and Metronome"],
  ["app-midi-out", { view: "midi-out" }, ".coros-midi-out", "Close Preset MIDI Out"],
  ["app-modes", { view: "modes" }, ".coros-modes-configuration", "Close Modes Configuration"],
  ["app-io", { view: "io-overview" }, ".coros-io-settings", "Close I/O Settings"],
  ["app-global-eq", { view: "global-eq" }, ".coros-global-eq", "Close Global EQ"],
  ["app-tuner", { view: "tuner" }, ".coros-tuner", "Close Tuner"],
  ["app-looper", { view: "looper-editor" }, ".coros-looper", "Close Looper X"],
  ["app-gig-view", { view: "gig" }, ".coros-gig-view", "Close Gig View"]
];
for (const [id, scenario, root, label] of SCREEN_CLOSES) {
  POPUP_FIXTURES.push({ id, propKey: "screenView", selectors: [root], firstModelOnly: true, scenario, closeOnly: true, closeControls: [close(`button[aria-label="${label}"]`)] });
}

/** Surface props that hold a popup's content. Anything that opens a screen, panel or popup belongs in POPUP_FIXTURES above. */
export const POPUP_PROPS = ["popup", "routingPicker", "addDevicePicker", "sceneCommand", "sceneEditor", "cpuMonitor", "savePreset", "parameterEditor", "presetDirectory", "expressionAssignment", "laneControlEditor", "screenContent"] as const;
