// Device-derived reference data, kept apart from the demo code so the project
// LICENSE can carve it out by whole file: parameter names, ranges, option
// labels (including factory IR names) and model-specific parameter orders as
// the Quad Cortex reports them. Not licensed under MIT OR Apache-2.0; see
// LICENSES/LicenseRef-QC-Remote-Reference.txt.
import type { ParameterSeed } from "../demo-parameters.ts";

export const schemas: Record<string, ParameterSeed[]> = {
  plugins: [["INPUT", .5, 0, 10], ["DRIVE", .58, 0, 10], ["TONE", .52, 0, 10], ["OUTPUT", .5, -24, 24, "dB"], ["MIX", 1, 0, 100, "%"]],
  amp: [["GAIN", .5, 0, 10], ["BASS", .5, 0, 10], ["MID", .5, 0, 10], ["TREBLE", .5, 0, 10], ["MASTER", .8, 0, 10], ["PRESENCE", .15, 0, 10], ["OUTPUT", .5, -24, 24, "dB"]],
  "neural capture": [["GAIN", .5, -24, 24, "dB"], ["BASS", .5, -10, 10, "dB"], ["MID", .5, -10, 10, "dB"], ["TREBLE", .5, -10, 10, "dB"], ["OUTPUT", .5, -24, 24, "dB"], ["PHASE", 0, 0, 1, "", ["NORMAL", "INVERT"]]],
  cab: [
    ["bypass", 0, 0, 1, "", ["ON", "BYPASS"]],
    ["ir selector", .6, 0, 1, "", ["NG_212 UK C30 65_Condenser 184", "NG_212 UK C30 65_Condenser 414", "NG_212 UK C30 65_Dynamic 421", "NG_212 UK C30 65_Dynamic 57", "NG_212 UK C30 65_Ribbon 10", "NG_212 UK C30 65_Ribbon 160"]],
    ["LEVEL", .5, -40, 12, "dB"], ["PAN", .5, 0, 10], ["DISTANCE", 0, 0, 10], ["POSITION", .4, 0, 10],
    ["phi", 0, 0, 1, "", ["NORMAL", "INVERT"]], ["GRID MODE", 1, 0, 1, "", ["OFF", "ON"]],
    ["bypass", 0, 0, 1, "", ["ON", "BYPASS"]],
    ["ir selector", 1, 0, 1, "", ["NG_212 UK C30 65_Condenser 184", "NG_212 UK C30 65_Condenser 414", "NG_212 UK C30 65_Dynamic 421", "NG_212 UK C30 65_Dynamic 57", "NG_212 UK C30 65_Ribbon 10", "NG_212 UK C30 65_Ribbon 160"]],
    ["LEVEL", .5, -40, 12, "dB"], ["PAN", .5, 0, 10], ["DISTANCE", 0, 0, 10], ["POSITION", .4, 0, 10],
    ["phi", 0, 0, 1, "", ["NORMAL", "INVERT"]], ["GRID MODE", 1, 0, 1, "", ["OFF", "ON"]],
    ["HPF", 0, 20, 500, "Hz"], ["LPF", 1, 500, 20000, "Hz"], ["OUTPUT VOLUME", .889, -96, 12, "dB"]
  ],
  overdrive: [["DRIVE", .58, 0, 10], ["TONE", .52, 0, 10], ["LEVEL", .64, 0, 10], ["BASS", .5, 0, 10], ["MIX", 1, 0, 100, "%"]],
  delay: [["TIME", .38, 1, 2000, "ms"], ["FEEDBACK", .42, 0, 100, "%"], ["MIX", .3, 0, 100, "%"], ["SYNC", 0, 0, 1, "", ["OFF", "ON"]], ["TRAILS", 1, 0, 1, "", ["OFF", "ON"]], ["LOW CUT", .12, 20, 2000, "Hz"], ["HIGH CUT", .72, 1000, 20000, "Hz"], ["MOD", .18, 0, 100, "%"]],
  reverb: [["MIX", .28, 0, 100, "%"], ["DECAY", .52, .1, 30, "s"], ["PRE DELAY", .2, 0, 500, "ms"], ["LOW CUT", .1, 20, 2000, "Hz"], ["HIGH CUT", .72, 1000, 20000, "Hz"], ["MOD", .22, 0, 100, "%"], ["TRAILS", 1, 0, 1, "", ["OFF", "ON"]]],
  compressor: [["INPUT", .5, -24, 24, "dB"], ["THRESHOLD", .55, -60, 0, "dB"], ["RATIO", .4, 1, 20, ":1"], ["ATTACK", .2, .1, 200, "ms"], ["RELEASE", .45, 10, 2000, "ms"], ["OUTPUT", .5, -24, 24, "dB"], ["MIX", 1, 0, 100, "%"], ["SIDECHAIN", 0, 0, 1, "", ["OFF", "ON"]]],
  pitch: [["INTERVAL 1", .58, -24, 24, "st"], ["LEVEL 1", .5, -24, 12, "dB"], ["INTERVAL 2", .25, -24, 24, "st"], ["LEVEL 2", .4, -24, 12, "dB"], ["MIX", .7, 0, 100, "%"], ["TRACKING", .5, 0, 1, "", ["SMOOTH", "FAST"]]],
  modulation: [["RATE", .35, .01, 20, "Hz"], ["DEPTH", .62, 0, 100, "%"], ["MIX", .45, 0, 100, "%"], ["FEEDBACK", .3, -100, 100, "%"], ["SYNC", 0, 0, 1, "", ["OFF", "ON"]], ["STEREO", 1, 0, 1, "", ["MONO", "STEREO"]]],
  morph: [["MODE", .5, 0, 1, "", ["FREEZE", "MORPH"]], ["RISE", .25, 0, 5000, "ms"], ["FALL", .4, 0, 5000, "ms"], ["MIX", 1, 0, 100, "%"], ["LATCH", 1, 0, 1, "", ["MOMENTARY", "LATCH"]]],
  synth: [["WAVE", 0, 0, 1, "", ["SINE", "SAW", "SQUARE"]], ["PITCH", .5, -24, 24, "st"], ["ATTACK", .1, 0, 5000, "ms"], ["RELEASE", .35, 0, 5000, "ms"], ["FILTER", .62, 20, 20000, "Hz"], ["RESONANCE", .3, 0, 100, "%"], ["MIX", .5, 0, 100, "%"]],
  filter: [["FREQUENCY", .48, 20, 20000, "Hz"], ["RESONANCE", .35, 0, 100, "%"], ["SENSITIVITY", .56, 0, 100, "%"], ["ATTACK", .2, 0, 1000, "ms"], ["RELEASE", .45, 0, 2000, "ms"], ["MODE", 0, 0, 1, "", ["LOW PASS", "BAND PASS", "HIGH PASS"]], ["MIX", 1, 0, 100, "%"]],
  eq: [["LOW", .5, -12, 12, "dB"], ["LOW MID", .5, -12, 12, "dB"], ["MID", .5, -12, 12, "dB"], ["HIGH MID", .5, -12, 12, "dB"], ["HIGH", .5, -12, 12, "dB"], ["OUTPUT", .5, -24, 24, "dB"]],
  "ir loader": [["IR", 0, 0, 1, "", ["Factory IR", "User IR 1", "User IR 2"]], ["LOW CUT", .08, 20, 2000, "Hz"], ["HIGH CUT", .75, 1000, 20000, "Hz"], ["LEVEL", .5, -24, 12, "dB"], ["PHASE", 0, 0, 1, "", ["NORMAL", "INVERT"]]],
  wah: [["POSITION", .45, 0, 100, "%"], ["RANGE", .55, 0, 100, "%"], ["Q", .5, .1, 10], ["LEVEL", .5, -12, 12, "dB"], ["AUTO ENGAGE", 0, 0, 1, "", ["OFF", "ON"]]],
  "fx loop": [["SEND LEVEL", .5, -24, 12, "dB"], ["RETURN LEVEL", .5, -24, 12, "dB"], ["MIX", 1, 0, 100, "%"], ["PHASE", 0, 0, 1, "", ["NORMAL", "INVERT"]], ["STEREO", 0, 0, 1, "", ["MONO", "STEREO"]]],
  looper: [["PLAYBACK LEVEL", .8, -60, 6, "dB"], ["OVERDUB LEVEL", .76, -60, 0, "dB"], ["HIGH PASS", .08, 20, 2000, "Hz"], ["LOW PASS", .82, 1000, 20000, "Hz"], ["THRESHOLD", 0, 0, 1, "", ["OFF", "ON"]], ["RECORD MODE", 0, 0, 1, "", ["TOGGLE", "MOMENTARY"]], ["OVERDUB MODE", 0, 0, 1, "", ["TOGGLE", "MOMENTARY"]], ["DUPLICATE MODE", 0, 0, 1, "", ["FREE", "SYNC"]], ["PUNCH MODE", 0, 0, 1, "", ["TOGGLE", "MOMENTARY"]], ["ROUTING MODE", 0, 0, 1, "", ["GRID", "INPUT 1", "INPUT 2", "RETURNS 1/2", "OUTPUTS 1/2"]], ["QUANTIZE", 0, 0, 1, "", ["OFF", "1 BEAT", "2 BEATS", "4 BEATS", "8 BEATS", "16 BEATS"]], ["MIDI CLOCK START", 0, 0, 1, "", ["FREE", "SYNC"]], ["PRE ROLL", 0, 0, 1, "", ["OFF", "1 BAR", "2 BARS", "4 BARS"]], ["METRONOME PLAYBACK", 0, 0, 1, "", ["OFF", "ON"]], ["RECORDING LENGTH", 0, 0, 1, "", ["FREE", "1 BAR", "2 BARS", "4 BARS", "8 BARS", "16 BARS", "32 BARS"]]],
  utility: [["GAIN", .5, -24, 24, "dB"], ["PAN", .5, -100, 100, "%"], ["MUTE", 0, 0, 1, "", ["OFF", "ON"]], ["PHASE", 0, 0, 1, "", ["NORMAL", "INVERT"]], ["MIX", 1, 0, 100, "%"]]
};

// Exact model-specific orders come from the QC model catalog/protocol. These
// take precedence over the category fallback, which exists only to make every
// category inspectable while the desktop preview is disconnected.
export const modelSchemas: Record<string, ParameterSeed[]> = {
  "simple gate": [["THRESHOLD", 0, -60, 0, "dB"]],
  "chief ds1": [["DISTORTION", .58, 0, 10], ["TONE", .47, 0, 10], ["LEVEL", .5, 0, 10]],
  "digital flanger": [
    ["MIX", 1, 0, 100, "%"], ["RATE", .0095, .01, 20, "Hz", [], undefined, 2], ["SYNC", 1, 0, 1, "", ["On", "Off"], "switch"],
    // SYNC NOTE's seventeen notes and its rotarySwitch step count are the
    // catalogue's own, from docs/reference/qc-live-parameter-scales.json (model
    // 7009). editor-digital-flanger prints "1/4", which is index 9 of the
    // seventeen, and marks that detent on the ring.
    ["SYNC NOTE", 9 / 16, 0, 1, "", ["1/32", "1/16T", "1/32D", "1/16", "1/8T", "1/16D", "1/8", "1/4T", "1/8D", "1/4", "1/2T", "1/4D", "1/2", "1/1T", "1/2D", "1/1", "1/1D"]], ["DEPTH", 1, 0, 100, "%"],
    ["DELAY", 0, 1, 20, "ms", [], undefined, 2], ["FEEDBACK", .9, -100, 100, "%"],
    // POLARITY is Pos, not Neg: editor-digital-flanger draws its detent 11.29
    // from the pill's top - the first option's end - and prints "Pos" at full
    // ink against a dim "Neg". Seeded at 1 the shared pill drew the ring 24px
    // low, which the bare marker this control used to carry could not show.
    ["POLARITY", 0, 0, 1, "", ["Pos", "Neg"], "switch"],
    ["WIDTH", 1, 0, 100, "%"], ["DRIVE", 0, 0, 1, "", ["OFF", "ON"]]
  ],
  "uk c30 topboost": [["VOLUME", .38, 0, 10], ["BASS", .38, 0, 10], ["TREBLE", .58, 0, 10], ["TONE CUT", .45, 0, 10], ["BOOST", 0, 0, 1, "", ["COOL", "HOT"], "switch"], ["OUTPUT", .5, -24, 24, "dB"]],
  "ambience": [["MIX", .12, 0, 100, "%"], ["SIZE", .5, 0, 1, "", ["Small", "Med", "Large"]], ["PRE DELAY", .04, 0, 500, "ms"], ["HIGH PASS", 60 / 1980, 20, 2000, "Hz"], ["LOW PASS", 5000 / 19000, 1000, 20000, "Hz"], ["TRAILS", 0, 0, 1, "", ["On", "Off"], "switch"]],
  "adaptive gate": [["NOISE REDUCTION", .62, 0, 100, "%"]],
  "adaptive gate (s/c)": [["NOISE REDUCTION", .02, 0, 100, "%"], ["SOURCE", 0, 0, 1, "", ["Off", "Path 1 Input", "Path 2 Input", "Path 1 Return", "Path 2 Return"], "enum"]],
  "parametric-8": Array.from({ length: 8 }, (_, index) => {
    const band = index + 1;
    const frequency = ([80, 200, 500, 1000, 2500, 5000, 10000, 16000][index] - 20) / (20000 - 20);
    return [
      [`${band} GAIN`, .5, -12, 12, "dB"],
      [`${band} FREQ`, frequency, 20, 20000, "Hz"],
      [`${band} Q`, .09, .1, 10],
      [`${band} TYPE`, .5, 0, 1, "", ["Peak", "Hi Pass", "Lo Pass", "Hi Shelf", "Lo Shelf"]],
      [`${band} BYPASS`, 0, 0, 1, "", ["ON", "BYPASS"]]
    ] as ParameterSeed[];
  }).flat().concat([["OUTPUT", .5, -20, 20, "dB"]]),
  "parametric-3": Array.from({ length: 3 }, (_, index) => {
    const band = index + 1;
    return [
      [`${band} GAIN`, index === 1 ? .32 : .5, -12, 12, "dB"],
      [`${band} FREQ`, ([100, 1000, 10000][index] - 20) / (20000 - 20), 20, 20000, "Hz"],
      [`${band} Q`, .34, .1, 10],
      [`${band} TYPE`, .5, 0, 1, "", ["Peak", "Hi Pass", "Lo Pass", "Hi Shelf", "Lo Shelf"]],
      [`${band} BYPASS`, 0, 0, 1, "", ["ON", "BYPASS"]]
    ] as ParameterSeed[];
  }).flat().concat([["OUTPUT", .5, -20, 20, "dB"]]),
  "graphic-9": [["HPF", 0, 19, 500, "Hz"], ...["65Hz", "125Hz", "250Hz", "500Hz", "1kHz", "2kHz", "4kHz", "8kHz", "16kHz"].map((name, index) => [name, [.48, .53, .58, .51, .44, .49, .55, .47, .52][index], -12, 12, "dB"] as ParameterSeed), ["LPF", 1, 1000, 20001, "Hz"], ["Q", .09, 0, 10], ["OUTPUT", .5, -20, 20, "dB"]],
  "low-high cut": [["HPF SLOPE", 0, 0, 8, "dB/oct", ["Flat", "-6", "-12", "-18", "-24", "-30", "-36", "-42", "-48"]], ["HPF FREQ", .08, 20, 20000, "Hz"], ["LPF SLOPE", 0, 0, 8, "dB/oct", ["Flat", "-6", "-12", "-18", "-24", "-30", "-36", "-42", "-48"]], ["LPF FREQ", .82, 20, 20000, "Hz"], ["OUTPUT", .5, -20, 20, "dB"]],
  "harmonic tremolo": [
    ["RATE", .3, .05, 20, "Hz"],
    ["DEPTH", .72, 0, 100, "%"],
    ["WAVEFORM", 0, 0, 1, "", ["SINE", "TRIANGLE", "SQUARE", "SAW UP", "SAW DN"]],
    ["DUTY CYCLE", .5, 0, 100, "%"],
    ["SMOOTHING", .28, 0, 100, "%"],
    ["LFO ACTIVE", 1, 0, 1, "", ["OFF", "ON"]],
    ["FADE IN", 0, 0, 5000, "ms"],
    ["FADE OUT", 0, 0, 5000, "ms"],
    ["BOOST", .5, 0, 100, "%"],
    ["LP XOVER", .34, 20, 20000, "Hz"],
    ["HP XOVER", .58, 20, 20000, "Hz"],
    ["SYNC", 0, 0, 1, "", ["OFF", "ON"]],
    ["SYNC NOTE", .5, 0, 1, "", ["1/16", "1/8T", "1/16D", "1/8", "1/4T", "1/8D", "1/4", "1/2T", "1/4D", "1/2", "1/1T", "1/2D", "1/1", "1/1D"]],
    ["SYNC ON", 0, 0, 1, "", ["LOWS", "HIGHS"]]
  ]
};
