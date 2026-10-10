// Device-derived reference content, kept apart from the snapshot builders so
// the project LICENSE can carve it out by whole file: the preset, block and
// model names coros-screen-fixture-data.ts reproduces from device screens,
// keyed by the builder's own block ids. Not licensed under MIT OR Apache-2.0;
// see LICENSES/LicenseRef-QC-Remote-Reference.txt.

/** The factory preset grid-official-brit shows, as CorOS names it and its blocks. */
export const OFFICIAL_BRIT_2203_PRESET_NAME = "Brit 2203";

export const OFFICIAL_BRIT_2203_BLOCK_NAMES = {
  "brit-gate": "Adaptive Gate",
  "brit-wah": "Wah",
  "brit-plugin": "Plugin Drive",
  "brit-drive-a": "Drive",
  "brit-drive-b": "Boost",
  "brit-amp": "Brit 2203",
  "brit-utility": "Utility",
  "brit-cab": "412 Brit",
  "brit-row3-ir": "IR Loader",
  "brit-row3-utility": "Utility",
  "brit-delay-a": "Reference Delay A",
  "brit-delay-b": "Reference Delay B",
  "brit-row3-reverb": "Reverb",
  "brit-looper": "Looper X"
} as const;

/** The models the CorOS 4.1.0 fixture preset chains, as the device names them. */
export const COROS_410_BLOCK_NAMES = {
  "simple-gate": "Simple Gate",
  "chief-ds1": "Chief DS1",
  "digital-flanger": "Digital Flanger",
  "ukc30-topboost": "UK C30 TopBoost",
  "ukc30-cab": "212 UK C30 65 (M)",
  "parametric-8": "Parametric-8",
  "ambience": "Ambience"
} as const;
