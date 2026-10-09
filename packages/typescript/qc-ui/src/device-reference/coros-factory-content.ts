// Device-derived reference content, kept apart from the screen code so the
// project LICENSE can carve it out by whole file: factory preset, Neural
// Capture, impulse response, model and plugin names, and the authors CorOS
// shows beside them, as the unit lists them on the screens that
// coros-screen-fixtures.tsx reconstructs. Not licensed under MIT OR
// Apache-2.0; see LICENSES/LicenseRef-QC-Remote-Reference.txt. Order is the
// device's own.

export const GIG_OFFICIAL_PRESET_TILES = [
  ["1A", "Brit 2203"],
  ["1B", "Brit\nPlexi100\nNormal"],
  ["1C", "US TWN\nVibrato"],
  ["1D", "Rols Jazz\nCH120"],
  ["1E", "California\nTremo\nRed"],
  ["1F", "EV101III\nRed"],
  ["1G", "Freeman\n100\nRhythm"],
  ["1H", "D-Cell H4\nCh3"],
];

export const GIG_OFFICIAL_SCENE_TILES = ["British\n2203", "+MX OD\n+Doubler", "+Brit\nGovernor", "+Rodent\nDrive", "Dry\nDouble", "+MX &\nDlys", "Solo\nBoost", "Juicy Low\nGain"];

export const GIG_OFFICIAL_STOMP_TILES = [
  ["A", "Crying\nWah"],
  ["B", "MX\nClassicOD 4"],
  ["C", "Brit\nGovernor"],
  ["D", "Rodent\nDrive"],
  ["E", "Looper X"],
  ["F", "Transpose"],
  ["G", "Multiple\ndevices (2)"],
  ["H", "Room"],
];

export const FACTORY_PRESET_BANK_1_ROWS = ["1A Brit 2203", "1B Brit Plexi100 Normal", "1C US TWN Vibrato", "1D Rols Jazz CH120", "1E California Tremo Red", "1F EV101III Red", "1G Freeman 100 Rhythm", "1H D-Cell H4 Ch3"];

export const DIRECTORY_SEARCH_RESULTS_AMP = [
  ["All Amps & Pedals dialed for Strats", "Neural DSP"],
  ["Amped SV", "Neural DSP"],
  ["Amped SV & Rodent", "Neural DSP"],
  ["Amped V4B", "Neural DSP"],
  ["Amped V4B & Rodent", "Neural DSP"],
  ["Demo Swell Example (Autoswell)", "Demo UserJazz"],
];

export const DIRECTORY_SEARCH_RESULTS_BRIT = [
  ["Blasted Brit", "Neural DSP"],
  ["Bright Brit", "Neural DSP"],
  ["Brit 2203", "Neural DSP"],
  ["Brit Bass 50 Normal", "Neural DSP"],
  ["Brit Bass 50 Patch", "Neural DSP"],
  ["Brit Plexi 50 Patch", "Neural DSP"],
];

export const CAPTURE_LIBRARY_FUZZ_ROWS = ["Beehive Wasp Fuzz 1", "Beehive Wasp Fuzz 2", "Beehive Wasp Fuzz 3", "Beehive Wasp Fuzz 4", "Beehive Wasp Fuzz 5", "Beehive Wasp Fuzz 6", "Iba Basic Fuzz 1"];

export const CAPTURE_LIBRARY_FUZZ_FOLDERS = ["Beehive Wasp Fuzz", "Iba Basic Fuzz", "Love Believer", "Love Bender Mk3", "Love Fuzz Lord III", "MKK Bass Lord"];

export const FAVORITE_CAPTURE_ROWS = ["Combo Deluxe 212"];

export const FACTORY_CAPTURE_ROWS = ["4-Comp Custom 1", "4-Comp Custom 2", "4-Comp Custom 3", "4-Comp Custom 4", "4-Comp Custom 5", "4-Comp Custom 6", "4-Comp Custom 7"];

export const MANUAL_IR_ROWS = ["IR21", "IR20", "IR19", "IR18", "IR17", "IR16", "IR15"];

export const DIRECTORY_CAPTURE_NAMES = ["Set Bass DI", "Brit Crunch 57", "Cali Recto Lead", "Chief OD Push", "Clean Twin 121", "German High Gain", "Vintage Fuzz"];

export const DIRECTORY_IR_NAMES = ["1x12 Blue Alnico", "2x12 UK C30 65", "4x10 Bass Modern", "4x12 Green 25", "Room Ribbon 160", "Studio 121 Dark", "User IR 01"];

export const DIRECTORY_PLUGIN_PRESET_NAMES = ["Cory Wong - Clean", "Gojira - Rhythm", "Nolly - Lead", "Plini - Crystal", "Parallax - Modern", "SLO-100 - Crunch", "Nameless - Grind"];

export const PLUGIN_LICENSES = [
  ["Archetype: Plini X", true],
  ["Archetype: Cory Wong X", false],
  ["Archetype: Gojira X", false],
  ["Archetype: John Mayer X", false],
  ["Archetype: Misha Mansoor X", false],
  ["Archetype: Nolly X", false],
  ["Archetype: Petrucci X", false],
  ["Archetype: Rabea X", false],
  ["Archetype: Tim Henson X", false],
  ["Fortin Nameless Suite X", false],
  ["Parallax X", false],
  ["Soldano SLO-100 X", false],
] as const;

export const PLINI_MODELS = [
  ["Plini Clean", "amp"],
  ["Plini Crunch", "amp"],
  ["Plini Lead", "amp"],
  ["Plini Cab (M)", "cab"],
  ["Plini Cab (ST)", "cab"],
  ["Plini Drive", "drive"],
] as const;

export const CORY_WONG_MODELS = [
  ["Cory Wong D.I. Funk Console", "amp"],
  ["Cory Wong The Amp Snob", "amp"],
  ["Cory Wong The Clean Machine", "amp"],
  ["Cory Wong Cab 1 (M)", "cab"],
  ["Cory Wong Cab 2 (M)", "cab"],
  ["Cory Wong Wah", "drive"],
] as const;

export const PLUGIN_BROWSER_ROWS = ["Archetype: Cory Wong X", "Archetype: Gojira X", "Archetype: Nolly X", "Archetype: Plini X", "Fortin Nameless Suite X", "Parallax X", "Soldano SLO-100 X"];

export const AMP_MODELS_FIRST_PAGE = ["Bogna Uber Clean", "Bogna Uber Lead", "Bogna Vishnu 20th Clean", "Brit 2203", "Brit 900 Clean", "Brit 900 Lead", "Brit Plexi 100 Bright"];

export const LOCKED_PLUGIN_MODEL_ROWS = [
  ["Cory Wong D.I. Funk Console", "amp"],
  ["Cory Wong The Amp Snob", "amp"],
  ["Cory Wong The Clean Machine", "amp"],
  ["Cory Wong Cab 1 (M)", "cab"],
  ["Cory Wong Cab 2 (M)", "cab"],
  ["Cory Wong Cab 3 (M)", "cab"],
] as const;

export const PLUGIN_MODEL_ROWS = [
  ["Plini Clean", "amp"],
  ["Plini Crunch", "amp"],
  ["Plini Lead", "amp"],
  ["Plini Cab (M)", "cab"],
  ["Plini Cab (ST)", "cab"],
  ["Plini Drive", "drive"],
] as const;

export const DEVICE_SEARCH_ROWS = [
  ["Chief DS1", "OVERDRIVE", true],
  ["Chief SD1", "OVERDRIVE", true],
  ["Chief OD1", "OVERDRIVE", true],
] as const;

export const OVERDRIVE_MODELS_FIRST_PAGE = ["Exotic Z Boost", "81 Creations Drive", "Brit Blues", "Brit Governor", "Chief BD2", "Chief DS1", "Chief MT"];

export const USER_DEVICE_PRESETS = ["Lead Tone", "Low Gain"];

export const FACTORY_AMP_DEVICE_PRESETS = ["Neural DSP® Default", "Balanced Crunch", "Basic Clean", "Bass Clean", "Bass Punk Drive", "Bass Tube Drive", "Bass Verge Of OD"];

export const FACTORY_OVERDRIVE_DEVICE_PRESETS = ["Neural DSP® Default", "Bass More Push", "Bass Push", "Bass Tube Drive", "Bass Tube OD", "Bigger Maker", "Bright Boost"];

export const COROS_OVERDRIVE_MODELS = ["Exotic Z Boost", "81 Creations Drive", "Brit Blues", "Brit Governor", "Chief BD2", "Chief DS1", "Chief MT", "Chief OD1", "Chief SD1", "Exotic", "Facial Fuzz", "Freeman BOD"];

/** Every amp the browser lists; the view shows the first seven and clips the
 *  rest. Only the first still fits the preset's remaining DSP, so CorOS
 *  disables the others. */
export const DSP_LIMITED_AMPS = ["Bogna Uber Clean", "Bogna Uber Lead", "Bogna Vishnu 20th Clean", "Brit 2203", "Brit 900 Clean", "Brit 900 Lead", "Brit Plexi 100 Bright", "Brit Plexi 100 Normal", "Brit Plexi 100 Patch", "Brit Plexi 50 Bright", "Brit Plexi 50 Normal", "Brit Plexi 50 Patch", "Brit TM45 Bright", "Brit TM45 Normal", "Brit TM45 Patch", "Brit UBL Lead", "Brit UBL Lead Clip", "CA 1Star Clean 100W Normal", "CA 1Star Clean 100W Tweed", "CA 1Star Clean 50W Normal", "CA 1Star Clean 50W Tweed", "CA 1Star Drive 100W Normal", "CA 1Star Drive 100W Tweed", "CA 1Star Drive 50W Normal", "CA 1Star Drive 50W Tweed", "CA Duo Ch3 Modern", "CA Duo Ch3 Raw", "CA Duo Ch3 Vintage", "CA John", "CA Tremo Orange", "CA Tremo Red", "Captain 50", "D-Cell H4 Ch1 Bright", "D-Cell H4 Ch1 Normal", "D-Cell H4 Ch2 Bright", "D-Cell H4 Ch2 Normal", "D-Cell H4 Ch3", "D-Cell H4 Ch4", "D-Cell Hisbert Ch1", "D-Cell Hisbert Ch2", "D-Cell Hisbert Ch3", "Dumbbell ODS", "EV101IIIS Blue 6L6 100W", "EV101IIIS Blue EL34 100W", "EV101IIIS Red 6L6 100W", "EV101IIIS Red EL34 100W", "Freeman 100 Clean", "Freeman 100 Lead", "Freeman 100 Rhythm", "Matchmore D30 Ch1", "Matchmore D30 Ch2", "Matchmore Jefe", "PV-505 Lead", "PV-505 Rhythm", "Rols Jazz CH120", "Solo 100 Crunch Bright", "Solo 100 Crunch Normal", "Solo 100 Lead", "UK C15 Normal", "UK C15 TopBoost", "UK C30 Normal", "UK C30 TopBoost", "US DLX 64 Vintage", "US DLX 65 Reissue", "US HP Tweed TWN Bright", "US HP Tweed TWN Bright Patch", "US HP Tweed TWN Normal", "US HP Tweed TWN Normal Patch", "US Prince", "US SPR Normal", "US SPR Vibrato", "US TWN Normal", "US TWN Vibrato", "US Tweed Basslad Bright", "US Tweed Basslad Bright Patch", "US Tweed Basslad Normal", "US Tweed Basslad Normal Patch", "Victor Squid Ch1", "Victor Squid Ch2", "Watt D103 Bright", "Watt D103 Normal"];

export const OVERDRIVE_MODELS = ["Exotic Z Boost", "81 Creations Drive", "Brit Blues", "Brit Governor", "Chief BD2", "Chief DS1", "Chief MT", "Chief OD1", "Chief SD1", "Exotic", "Facial Fuzz", "Freeman BOD"];

export const GIG_OFFICIAL_PRESET_TITLE = "1A Brit 2203";

export const DIRECTORY_SEARCH_QUERY_BRIT = "Brit";

export const FACTORY_CAPTURE_AUTHOR = "NeuralDSP";

export const PLUGIN_PRESET_AUTHOR_LABEL = "NEURAL DSP X";

export const CAPTURE_EDITOR_PRESET_NAME = "Combo Deluxe Reverb";

export const CAPTURE_EDITOR_CAPTURE_NAME = "Boutique Mod OD 3";

export const MODEL_BRIT_2203 = "Brit 2203";

export const MODEL_UK_C30_CAB = "2x12 UK C30 65 (M)";

export const MODEL_PARAMETRIC_8 = "Parametric-8";

export const MODEL_PARAMETRIC_5 = "Parametric-5";

export const MODEL_AMBIENCE = "Ambience";

export const MODEL_SIMPLE_GATE = "Simple Gate";

export const MODEL_ADAPTIVE_GATE = "Adaptive Gate";

export const MODEL_STUDIO_CAB = "Studio Cab";

export const MODEL_DIGITAL_DELAY = "Digital Delay";

export const MODEL_STUDIO_COMP = "Studio Comp";

export const MODEL_TRANSPOSE = "Transpose";

export const MODEL_OVERLORD_SYNTH = "Overlord Synth";

export const LOOPER_X_TITLE = "Looper X ";

export const PRESET_12_STRING_BASS = "12 String Bass";

export const PLUGIN_PARALLAX_X = "Parallax X";

export const PLUGIN_FOLDER_NEURAL_DSP = "Neural DSP";

export const DEVICE_SEARCH_QUERY_CHIEF = "Chief";

export const PLUGIN_ARCHETYPE_CORY_WONG = "Archetype: Cory Wong X";

export const PLUGIN_ARCHETYPE_PLINI = "Archetype: Plini X";

export const CLOUD_CAPTURE_SEARCH_RESULT = "Jam Demo Capture";

export const CLOUD_CAPTURE_AUTHOR = "Demo User";
