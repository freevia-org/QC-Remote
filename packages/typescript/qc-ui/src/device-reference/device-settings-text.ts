// Device-derived reference text, kept apart from the screen code so the
// project LICENSE can carve it out by whole file: explanatory copy as CorOS
// shows it on its Settings screen. Not licensed under MIT OR Apache-2.0; see
// LICENSES/LicenseRef-QC-Remote-Reference.txt. Arrays split a paragraph where
// the device breaks the line.

export const DEVICE_SETTINGS_COPY = {
  gigView: `Enable this feature to toggle Gig View by pressing and holding BANK DOWN and TEMPO footswitches simultaneously.

When enabled, single-pressing + TEMPO to cycle MODES is triggered upon footswitches release.`,
  sceneBypassThird: "Do not overwrite bypass state when changing bypass state by any method."
};

export const GLOBAL_BYPASS_BODY_LINES = [
  "Globally bypass Cabs, IR Loaders, or Neural Captures of",
  "cabs* on any row. Globally bypassed devices will have a",
  "bypass icon but will not appear bypassed on The Grid."
] as const;

export const GLOBAL_BYPASS_FOOTNOTE_LINES = [
  "*Neural Captures need to have the Capture Type set to \"Cab\" to be",
  "bypassed."
] as const;

export const SCENE_BYPASS_OPTION_TEXT = {
  alwaysOverwrite: "Always overwrite bypass state (default).",
  nonstompOverwrite: "Do not overwrite bypass state when changing bypass state via footswitches in Stomp Mode (including Hybrid Stomp Mode) or MIDI. Changes made with the touchscreen will be saved."
} as const;

export const SCENE_BYPASS_BODY_LINES = [
  "This feature controls whether changes to the bypass state",
  "of a block in Scene Mode are automatically saved to the",
  "active Scene."
] as const;

export const STOMP_MODE_BYPASS_BODY = "A global setting for whether footswitches are automatically assigned to blocks' bypass settings. If you enable auto-assign, blocks will be assigned to footswitches in the order they are added to The Grid.";

export const SWAP_TEMPO_TUNER_BODY = "Enable this option to access the Tuner by double-tapping the bottom-right footswitch, and access the Tempo settings by holding the bottom-right footswitch.";

export const LATENCY_COMPENSATION_BODY = "Disabling Dynamic Latency Compensation may help if phasing occurs when bypassing a device.";

export const HOLD_TIMING_BODY = "Sets how long a footswitch must be held down to trigger its assigned HOLD action.";

export const BRIGHTNESS_BODY_LINES = [
  "Turn the",
  ", and TEMPO footswitches to adjust the",
  "brightness. Tap the Modes at the bottom to toggle dimmed",
  "LED lights for each one individually."
] as const;

export const MASTER_VOLUME_LEAD = "Master Volume can control different outputs";

// CorOS's Modes Configuration instruction, split where the device breaks it.
export const MODES_CONFIGURATION_INSTRUCTION_LINES = [
  "Drag a Mode on top of another to create a Hybrid",
  "Mode. Use a long press to break a Hybrid Mode apart."
] as const;
