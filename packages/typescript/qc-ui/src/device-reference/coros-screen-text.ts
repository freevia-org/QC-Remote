// Device-derived reference text, kept apart from the screen code so the
// project LICENSE can carve it out by whole file: explanatory, help and
// dialog copy as CorOS shows it on the screens that coros-screen-fixtures.tsx
// reconstructs. Not licensed under MIT OR Apache-2.0; see
// LICENSES/LicenseRef-QC-Remote-Reference.txt. Arrays split a paragraph where
// the device breaks the line or sets an icon inside it.

export const DEVICE_COPY = {
  // Two paragraphs, each split where the unit sets its down triangle (the
  // BANK DOWN switch) inside the line (corpus settings-device-gig-view.png).
  gigView: [
    ["Enable this feature to toggle Gig View by pressing and holding ", " and TEMPO footswitches simultaneously."],
    ["When enabled, single-pressing ", "+ TEMPO to cycle MODES is triggered upon footswitches release."],
  ] as const,
  sceneBypassThird: "Do not overwrite bypass state when changing bypass state by any method.",
  resetSettings: "Reset Settings will restore Quad Cortex to its default settings. User data (Presets, Captures, etc.) will not be removed.",
  factoryReset: "Factory Reset will remove all user data and the device will be restored to factory settings.",
};

export const SHUTDOWN_DIALOG = ["POWER OFF?", "Any unsaved changes will be lost.", "POWER OFF"];

export const INPUT_CLIPPING_DIALOG = ["INPUT CLIPPING", "Reduce Input 1 gain to prevent unwanted distortion.", "OPEN I/O SETTINGS"];

export const DSP_LIMIT_DIALOG = ["DSP LIMIT REACHED", "There is not enough processing power to add this device.", "OK"];

export const RECOVERY_OPTIONS = [
  ["RESTART QUAD CORTEX", "Boot CorOS normally"],
  ["REINSTALL COROS", "Install the latest available system image"],
  ["FACTORY RESET", "Erase user data and restore defaults"],
  ["SHUT DOWN", "Power off safely"],
];

export const CAPTURE_TYPE_CHOICES = [
  ["Cab", "AMP + CAB", "A complete amplifier and cabinet"],
  ["Amp", "AMP", "Amplifier or preamp only"],
  ["Overdrive", "DRIVE", "Overdrive, distortion, or fuzz"],
  ["Utility", "OTHER", "Compressors and other devices"],
];

export const PASTE_OPTION_PROMPT = "Please select how you would like to paste these 1 Preset(s) into the banks in ";

export const PASTE_CONSECUTIVE_FRAGMENTS = [
  "Paste consecutively from the first ",
  " slot onwards"
] as const;

export const SCENE_COMMAND_PROMPT = "Press Scene destination footswitch.";

export const OVERWRITE_PRESET_BODY = "Preset Demo Acoustic Layer already exists. Overwrite?";

export const DELETE_PRESET_BODY = "Are you sure you want to delete this preset?";

export const RECOVERY_ENTRY_INSTRUCTION = "Keep footswitches A and H pressed while powering on Quad Cortex.";

export const RECOVERY_RELEASE_HINT = "Release the switches when the recovery menu appears.";

export const RECOVERY_OPTIONS_PROMPT = "Select an option to continue.";

export const ACTION_UNAVAILABLE_BODY = "Quad Cortex could not complete the request. Check the connection and try again.";

export const SAVING_PRESET_BODY = "Please wait. Do not disconnect or power off Quad Cortex.";

export const CAPTURE_SANITY_ERROR_TITLE = "There was an error during the Sanity Check stage";

export const CAPTURE_SANITY_ERROR_BODY = [
  "No signal detected, or signal too low. Please go back to the",
  "calibration screen and make sure the level meters are at a",
  "sensible level."
] as const;

export const CAPTURE_PROGRESS_BODY = [
  "The core of Neural Capture. Training a neural network to",
  "emulate the sound of your favorite device."
] as const;

export const CAPTURE_RESULT_BODY = [
  "Your Neural Capture is ready. Switch between the reference and Quad Cortex using",
  "the buttons below."
] as const;

export const CAPTURE_CALIBRATION_TIPS = [
  "Please verify your Quad Cortex is properly connected to the target device.",
  "Reduce levels if any of the meters detect clipping."
] as const;

export const CAPTURE_GROUND_LIFT_TIP = [
  "The IN 2 GROUND LIFT can mitigate noise caused by ground loops between Quad",
  "Cortex and the target device."
] as const;

export const CAPTURE_INTRO_BODY = "Create a digital replica of your amplifier, cabinet, or drive pedal.";

export const CAPTURE_INTRO_STEPS = [
  "Connect your gear",
  "Set levels and calibrate",
  "Capture, compare, and save"
] as const;

export const CAPTURE_TYPE_TITLE = "What would you like to capture?";

export const CAPTURE_TYPE_BODY = "Select the device type for the most accurate result.";

export const CAPTURE_ROUTING_TITLE = "Connect your equipment";

export const CAPTURE_ROUTING_BODY = "Follow the signal path below, then confirm that all cables are connected.";

export const CAPTURE_ROUTING_LOAD_BOX_TIP = "Use a load box when capturing an amplifier without a cabinet.";

export const ACCOUNT_LINKED_BODY = "You are ready to send & receive Presets & Neural Captures and use Cloud backups.";

export const BRIGHTNESS_BODY = [
  "Turn the",
  ", and TEMPO footswitches to adjust the",
  "brightness. Tap the Modes at the bottom to toggle dimmed",
  "LED lights for each one individually."
] as const;

export const GLOBAL_BYPASS_BODY = [
  "Globally bypass Cabs, IR Loaders, or Neural Captures of",
  "cabs* on any row. Globally bypassed devices will have a",
  "bypass icon",
  "but will not appear bypassed on The Grid."
] as const;

export const GLOBAL_BYPASS_FOOTNOTE = [
  "*Neural Captures need to have the Capture Type set to \"Cab\" to be",
  "bypassed."
] as const;

export const POWER_BUTTON_SENSITIVITY_BODY = [
  "Configure the sensitivity below. Tap the power button (above",
  "the volume knob) to see if it's responding as expected. If",
  "your touch is detected, all LEDs will illuminate."
] as const;

export const MASTER_VOLUME_ASSIGNMENT_BODY = "Master Volume can control different outputs";

export const STOMP_BYPASS_BODY = "A global setting for whether footswitches are automatically assigned to blocks' bypass settings. If you enable auto-assign, blocks will be assigned to footswitches in the order they are added to The Grid.";

export const SWAP_TEMPO_TUNER_BODY = "Enable this option to access the Tuner by double-tapping the bottom-right footswitch, and access the Tempo settings by holding the bottom-right footswitch.";

export const LATENCY_COMPENSATION_BODY = "Disabling Dynamic Latency Compensation may help if phasing occurs when bypassing a device.";

export const SCENE_BYPASS_BODY = [
  "This feature controls whether changes to the bypass state",
  "of a block in Scene Mode are automatically saved to the",
  "active Scene."
] as const;

export const SCENE_BYPASS_ALWAYS_OVERWRITE = "Always overwrite bypass state (default).";

export const SCENE_BYPASS_NON_STOMP_OVERWRITE = "Do not overwrite bypass state when changing bypass state via footswitches in Stomp Mode (including Hybrid Stomp Mode) or MIDI. Changes made with the touchscreen will be saved.";

export const SEND_REPORT_TITLE = "Thanks a lot for your help";

export const SEND_REPORT_BODY = "A diagnostic report is available to generate. Sending the report can take up to five minutes. You can continue to use your Quad Cortex during this time, but you may experience some slowness.";

export const BACKUPS_UTC_NOTE = "All timestamps are UTC";

export const HOLD_TIMING_BODY = "Sets how long a footswitch must be held down to trigger its assigned HOLD action.";

export const DEVICE_UPDATES_RUNNING = "Your Quad Cortex is currently running";

export const NEWS_BODY = [
  "Visit our blog and check out the latest news",
  "at neuraldsp.com/news",
  "or scan the QR code."
] as const;

export const RESTART_DEVICE_BODY = "Any unsaved changes will be lost";

export const MODES_CONFIGURATION_BODY = [
  "Drag a Mode on top of another to create a Hybrid",
  "Mode. Use a long press to break a Hybrid Mode apart."
] as const;

export const PLUGIN_REFRESH_NOTICE = "Refreshing the list can take 10-20 seconds.";

export const PLUGIN_LICENSE_NOT_FOUND = "Plugin license not found";

export const LOOPER_HINT_USE = "USE";

export const LOOPER_HINT_START_RECORDING = "TO START RECORDING";

export const LOOPER_HINT_CLOSE_VIEW = "TO CLOSE THE LOOPER VIEW";

export const EXPRESSION_CHOOSER_BODY = [
  "Please choose which parameters you wish to control.",
  "You can assign multiple at once."
] as const;

export const EXPRESSION_TRIM_HINT = [
  "Tap",
  "to trim"
] as const;

export const EXPRESSION_TRIM_DETAIL = "Max and Min values of the preferred parameter";

export const ASSIGN_FOOTSWITCH_PROMPT = "Press the target footswitch to assign";

export const VIRTUAL_DEVICE_PRESETS_TIP = "Tap the preset icon next to each virtual device to access its Factory and User Presets.";
