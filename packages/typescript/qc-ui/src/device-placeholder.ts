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

export type QcConnectingPhase = "opening" | "handshaking" | "synchronizing";

const CONNECTING_PHASE_TITLES: Record<QcConnectingPhase, string> = {
  opening: "Opening the Quad Cortex…",
  handshaking: "Handshaking with the Quad Cortex…",
  synchronizing: "Synchronizing the Quad Cortex…"
};

/** The connecting placeholder carrying the phase the connection has reached. One wording for every host. */
export function connectingPlaceholderSnapshot(phase: QcConnectingPhase): PresetSnapshot {
  return { ...CONNECTING_PLACEHOLDER_SNAPSHOT, presetName: CONNECTING_PHASE_TITLES[phase] };
}

/** Whether the Grid is showing a connecting placeholder (and so has no preset of its own to lose). */
export function isConnectingPlaceholder(snapshot: Pick<PresetSnapshot, "presetName" | "presetLocation">): boolean {
  return snapshot.presetLocation === CONNECTING_PLACEHOLDER_SNAPSHOT.presetLocation
    && (snapshot.presetName === CONNECTING_PLACEHOLDER_SNAPSHOT.presetName || Object.values(CONNECTING_PHASE_TITLES).includes(snapshot.presetName));
}
