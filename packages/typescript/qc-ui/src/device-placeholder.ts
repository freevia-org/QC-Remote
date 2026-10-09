import type { PresetSnapshot } from "@qc-remote/client";

/**
 * Neutral Grid state while a host switches away from a demo device. Nothing
 * in this snapshot can be mistaken for either the saved demo or live hardware.
 */
export const CONNECTING_PLACEHOLDER_SNAPSHOT: PresetSnapshot = Object.freeze({
  deviceName: "",
  presetName: "Connecting to the Quad Cortex…",
  presetLocation: "--",
  presetPosition: 0,
  setlistKey: "",
  setlistName: "",
  mode: "PRESET",
  activeScene: 0,
  scenes: ["", "", "", "", "", "", "", ""],
  footswitchStates: [],
  blocks: [],
  routes: [],
  tempo: 120,
  masterVolume: 0,
  dirty: false
});

/** Neutral Grid state after a host deliberately stops its demo device. */
export const IDLE_PLACEHOLDER_SNAPSHOT: PresetSnapshot = Object.freeze({
  ...CONNECTING_PLACEHOLDER_SNAPSHOT,
  presetName: "No device connected"
});
