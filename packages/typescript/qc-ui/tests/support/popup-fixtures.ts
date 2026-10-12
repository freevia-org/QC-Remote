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
}

export const POPUP_FIXTURES: PopupFixture[] = [
  { id: "preset-menu", propKey: "popup", selectors: [".coros-screen-menu", ".coros-screen-dimmer"] },
  { id: "scene-menu", propKey: "popup", selectors: [".scene-dropdown", ".scene-menu-backdrop"] },
  { id: "mode-menu", propKey: "popup", selectors: [".scene-dropdown", ".scene-menu-backdrop"] },
  { id: "input-picker", propKey: "routingPicker", selectors: [".coros-route-picker", ".coros-route-picker-dismiss"] },
  { id: "output-picker", propKey: "routingPicker", selectors: [".coros-route-picker", ".coros-route-picker-dismiss"] },
  { id: "add-device-drawer", propKey: "addDevicePicker", selectors: [".device-browser-official.is-live-picker"] },
  { id: "copy-scene", propKey: "sceneCommand", selectors: [".scene-command-panel", ".scene-command-scrim"], fraction: { width: 0.525, centered: true, tolerance: 0.02 } },
  { id: "swap-scene", propKey: "sceneCommand", selectors: [".scene-command-panel", ".scene-command-scrim"], fraction: { width: 0.525, centered: true, tolerance: 0.02 } },
  { id: "scene-label-editor", propKey: "sceneEditor", selectors: [".scene-label-dialog-panel"], fraction: { width: 1, height: 1, tolerance: 0.01 } },
  { id: "cpu-monitor", propKey: "cpuMonitor", selectors: [".coros-cpu-overlay"] },
  { id: "save-as", propKey: "savePreset", selectors: [".coros-save-preset"] },
  { id: "amp-editor-panel", propKey: "parameterEditor", selectors: [".coros-parameter-editor"] },
  { id: "cab-editor-full-screen", propKey: "parameterEditor", selectors: [".coros-parameter-editor"] },
  { id: "preset-directory", propKey: "presetDirectory", selectors: [".coros-directory"] },
  { id: "expression-assignment", propKey: "expressionAssignment", selectors: [".expression-bypass-official"] },
  { id: "lane-control-panel", propKey: "laneControlEditor", selectors: [".lane-control-panel", ".coros-lane-control-dismiss"] }
];

/** Surface props that hold a popup's content. Anything that opens a screen, panel or popup belongs in POPUP_FIXTURES above. */
export const POPUP_PROPS = ["popup", "routingPicker", "addDevicePicker", "sceneCommand", "sceneEditor", "cpuMonitor", "savePreset", "parameterEditor", "presetDirectory", "expressionAssignment", "laneControlEditor"] as const;
