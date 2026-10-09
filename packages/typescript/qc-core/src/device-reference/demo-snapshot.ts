import type { PresetSnapshot } from "@qc-remote/client";
import { QC_SCENE_COLORS } from "./generated-device-colors.ts";

/**
 * The offline preset the apps render before a device connects, and the layout
 * Demo Mode starts from.
 *
 * Device-derived reference material (LICENSE, Schedule A): the block, model
 * and plugin names below are Neural DSP's product names, and the scene colours
 * are the device's own palette. It lives here, under qc-core's carved-out
 * device-reference/ directory, so that @qc-remote/client stays plain
 * MIT OR Apache-2.0.
 */
export const demoSnapshot: PresetSnapshot = {
  deviceName: "Quad Cortex",
  presetName: "Brit 2203",
  presetLocation: "1A",
  presetPosition: 0,
  setlistKey: "demo",
  setlistName: "Demo Presets",
  mode: "PRESET",
  modeSlots: [
    { slot: 0, label: "PRESET", mode: "PRESET" },
    { slot: 1, label: "SCENE", mode: "SCENE" },
    { slot: 2, label: "STOMP", mode: "STOMP" }
  ],
  footswitchModes: ["PRESET", "PRESET"],
  activeScene: 0,
  scenes: ["Clean", "Edge", "Crunch", "Lead", "Ambient", "Octave", "Solo +", "Mute"],
  sceneColors: [...QC_SCENE_COLORS],
  tempo: 120,
  tempoLedEnabled: true,
  masterVolume: 40,
  dirty: false,
  routes: [
    { row: 0, inputId: 1, outputId: 1, input: "In 1", output: "Out 1/2", splitMuted: false },
    { row: 1, inputId: 2, outputId: 2, input: "In 2", output: "Out 3/4", splitMuted: false },
    { row: 2, inputId: 4, outputId: 8, input: "Return 1", output: "Send 1", splitMuted: false },
    { row: 3, inputId: 8, outputId: 22, input: "USB 5", output: "USB 3/4", splitMuted: false }
  ],
  ioPorts: [{ kind: "input", id: 1, label: "In 1", plugged: true }],
  blocks: [
    { id: "in-1", name: "IN 1", kind: "input", row: 0, column: -1 },
    { id: "gate", name: "Adaptive Gate", kind: "utility", category: "Utility", row: 0, column: 0, bypassed: true },
    { id: "amp", name: "British 2203", kind: "amp", category: "Amp", row: 0, column: 1 },
    { id: "capture", name: "OD Capture", kind: "capture", category: "Neural Capture", row: 0, column: 2 },
    { id: "cab", name: "Plini Cab", kind: "cab", category: "Cab", row: 0, column: 3, plugin: true, pluginId: "plini-x" },
    { id: "overdrive", name: "Gojira OD", kind: "utility", category: "Overdrive", row: 0, column: 4, plugin: true, pluginId: "gojira-x" },
    { id: "delay", name: "SLO-100 Delay", kind: "delay", category: "Delay", row: 0, column: 5, plugin: true, pluginId: "slo100-x" },
    { id: "reverb", name: "Nolly Reverb", kind: "reverb", category: "Reverb", row: 0, column: 6, plugin: true, pluginId: "nolly-x" },
    { id: "compressor", name: "Cory Wong Compressor", kind: "utility", category: "Compressor", row: 0, column: 7, plugin: true, pluginId: "cory-x" },
    { id: "out-1", name: "OUT 1/2", kind: "output", row: 0, column: 8 },
    { id: "in-2", name: "IN 2", kind: "input", row: 1, column: -1 },
    { id: "pitch", name: "Dual Octaver", kind: "mod", category: "Pitch", row: 1, column: 0, plugin: true, pluginId: "misha-x" },
    { id: "modulation", name: "Harmonic Tremolo", kind: "mod", category: "Modulation", row: 1, column: 1, plugin: true, pluginId: "mayer-x" },
    { id: "morph", name: "Freeze", kind: "utility", category: "Morph", row: 1, column: 2 },
    { id: "synth", name: "Synth", kind: "utility", category: "Synth", row: 1, column: 3 },
    { id: "filter", name: "Envelope Filter", kind: "utility", category: "Filter", row: 1, column: 4 },
    { id: "equalizer", name: "Parametric-8", kind: "utility", category: "EQ", row: 1, column: 5 },
    { id: "ir-loader", name: "Custom IR", kind: "cab", category: "IR Loader", row: 1, column: 6 },
    { id: "wah", name: "Crying Wah", kind: "utility", category: "Wah", row: 1, column: 7 },
    { id: "out-2", name: "OUT 3/4", kind: "output", row: 1, column: 8 },
    { id: "in-return", name: "RETURN 1", kind: "input", row: 2, column: -1 },
    { id: "fx-loop", name: "FX Loop 1", kind: "utility", category: "FX Loop", row: 2, column: 0 },
    { id: "looper", name: "Looper X", kind: "utility", category: "Looper", row: 2, column: 1 },
    { id: "utility", name: "Gain", kind: "utility", category: "Utility", row: 2, column: 2 },
    { id: "plugin", name: "Rabea Device", kind: "utility", category: "Plugins", row: 2, column: 3, plugin: true, pluginId: "rabea-x" },
    { id: "out-send", name: "SEND 1", kind: "output", row: 2, column: 8 }
  ]
};
