import type { BlockParameter } from "@qc-remote/client";
import { QC_COLORS } from "@qc-remote/theme";

export const PARAMETER_ENCODER_ROLES = [
  "footswitch:A", "footswitch:B", "footswitch:C", "footswitch:D", "footswitch:E",
  "bank:down", "footswitch:F", "footswitch:G", "footswitch:H", "tempo"
] as const;

export function parameterEditorPageSlots<T extends { displayPosition?: number }>(parameters: ReadonlyArray<T>, page: number, pageSize: number): Array<T | undefined> {
  const slots = Array<T | undefined>(pageSize).fill(undefined);
  parameters.forEach((parameter, index) => {
    const position = parameter.displayPosition ?? index;
    if (Math.floor(position / pageSize) === page) slots[position % pageSize] = parameter;
  });
  return slots;
}

export function parameterEditorPageCount(parameters: ReadonlyArray<{ displayPosition?: number }>, pageSize: number): number {
  const lastPosition = parameters.reduce((maximum, parameter, index) => Math.max(maximum, parameter.displayPosition ?? index), -1);
  return Math.max(1, Math.floor(lastPosition / pageSize) + 1);
}

export function parameterEditorControlSlots<T extends { index: number; displayPosition?: number }>(parameters: ReadonlyArray<T>, category: string, page: number, pageSize: number): Array<T | undefined> {
  if (parameterEditorFamily(category) !== "cab") return parameterEditorPageSlots(parameters, page, pageSize);
  const indexes = page === 0
    ? [5, 4, 2, 3, undefined, 13, 12, 10, 11, undefined]
    : [16, 18, 17, undefined, undefined, undefined, undefined, undefined, undefined, undefined];
  return indexes.map((index) => index === undefined ? undefined : parameters.find((parameter) => parameter.index === index));
}

export type ParameterEditorFamily =
  | "standard" | "compressor" | "delay" | "eq" | "modulation"
  | "pitch" | "reverb" | "synth" | "cab" | "ir" | "looper";

export type ParameterControlKind = "knob" | "select" | "switch" | "button" | "fader" | "meter";

/** Which family of screens a control that is not a BlockDetails parameter belongs to. */
export type DeviceSettingSurface = "io" | "general" | "tuner" | "masterVolume";

/**
 * The construction the unit actually draws. ParameterControlKind says which of
 * our six renderers is right; this says what the device shows, because "knob"
 * covers both the continuous arc and the detent ring and they are not the same
 * shape, and "switch" covers both the labelled option pair and the radio tile.
 */
export type DeviceSettingDrawing =
  | "arc-rotary" | "stepped-rotary" | "option-pair" | "radio-tile"
  | "check-tile" | "icon-button" | "dropdown" | "segmented-bar";

export type DeviceSettingControl = {
  /** Stable lookup key. */
  key: string;
  surface: DeviceSettingSurface;
  /** The label the unit prints beside the control, or the family name where it carries several. */
  label: string;
  /** How many of this control the device carries. */
  instances: number;
  /** Which renderer draws it. */
  kind: ParameterControlKind;
  /** Null where the control's construction is not established. */
  draws: DeviceSettingDrawing | null;
  /** Detents or segments. Null for a continuous control, and where the count is not established. */
  detents: number | null;
  /** Reserved for a per-setting contract naming the value domain. Always null: no such contract ships in this repository. */
  domain: string | null;
};

/**
 * The unit's I/O, general-settings, tuner and master-volume controls, and how
 * it draws each one.
 *
 * This table is hand-maintained on purpose. Every other control on the device
 * arrives as BlockDetails, where parameterControlKind below is a total function
 * of the ModelRepo's declared type and needs no table. These do not:
 *
 *  - They are not BlockDetails at all. They come back as IoSettings,
 *    GeneralSettings, TunerSettings and MasterVolume payloads, none of which
 *    carries a type word.
 *  - The public device metadata does not reliably describe the visible control
 *    kind or its detents for these settings.
 *
 * So there is no reliable rule to derive; each row below is declared explicitly
 * from the public device documentation and visible UI behavior. `domain` stays
 * null where no public row-level source is available. Where a row's construction
 * is not established it says so rather than guessing a rendering.
 */
const DEVICE_SETTING_CONTROL_TABLE = [
  {
    key: "io.input.level", surface: "io", label: "IN 1 LEVEL", instances: 2,
    kind: "knob", draws: "arc-rotary", detents: null,
    domain: null,
  },
  {
    key: "io.input.impedance", surface: "io", label: "IMPEDANCE", instances: 2,
    kind: "knob", draws: "stepped-rotary", detents: 9,
    domain: null,
  },
  {
    key: "io.input.type", surface: "io", label: "TYPE", instances: 2,
    kind: "switch", draws: "option-pair", detents: 2,
    domain: null,
  },
  {
    key: "io.input.phantom", surface: "io", label: "PHANTOM 48V", instances: 2,
    kind: "switch", draws: "option-pair", detents: 2,
    domain: null,
  },
  {
    key: "io.input.groundLift", surface: "io", label: "GROUND LIFT", instances: 2,
    kind: "switch", draws: "option-pair", detents: 2,
    domain: null,
  },
  {
    key: "io.output.level", surface: "io", label: "OUT 1 LEVEL", instances: 8,
    kind: "knob", draws: "arc-rotary", detents: null,
    domain: null,
  },
  {
    key: "io.output.groundLift", surface: "io", label: "GROUND LIFT", instances: 3,
    kind: "switch", draws: "option-pair", detents: 2,
    domain: null,
  },
  {
    key: "io.output.mute", surface: "io", label: "MUTE", instances: 6,
    kind: "switch", draws: "option-pair", detents: 2,
    domain: null,
  },
  {
    key: "io.output.pairing", surface: "io", label: "output link", instances: 2,
    kind: "button", draws: null, detents: null,
    domain: null,
  },
  {
    key: "io.headphones.level", surface: "io", label: "HP LEVEL", instances: 1,
    kind: "knob", draws: "arc-rotary", detents: null,
    domain: null,
  },
  {
    key: "io.headphones.feed", surface: "io", label: "MULTI OUT", instances: 10,
    kind: "knob", draws: "arc-rotary", detents: null,
    domain: null,
  },
  {
    key: "io.usb.level", surface: "io", label: "USB LEVEL", instances: 1,
    kind: "knob", draws: "arc-rotary", detents: null,
    domain: null,
  },
  {
    key: "io.usb.headphonesSource", surface: "io", label: "HP SOURCE", instances: 1,
    kind: "knob", draws: "stepped-rotary", detents: 4,
    domain: null,
  },
  {
    key: "io.usb.dryWet", surface: "io", label: "DRY - WET", instances: 1,
    kind: "switch", draws: null, detents: null,
    domain: null,
  },
  {
    key: "io.midi.thru", surface: "io", label: "MIDI Thru", instances: 1,
    kind: "switch", draws: "option-pair", detents: 2,
    domain: null,
  },
  {
    key: "general.screenBrightness", surface: "general", label: "Screen", instances: 1,
    kind: "fader", draws: "segmented-bar", detents: 32,
    domain: null,
  },
  {
    key: "general.ledBrightness", surface: "general", label: "LEDs", instances: 1,
    kind: "fader", draws: "segmented-bar", detents: 32,
    domain: null,
  },
  {
    key: "general.dimmedLedBrightness", surface: "general", label: "Dimmed LEDs", instances: 1,
    kind: "fader", draws: "segmented-bar", detents: 32,
    domain: null,
  },
  {
    key: "general.holdTiming", surface: "general", label: "Hold Timing", instances: 1,
    kind: "fader", draws: "segmented-bar", detents: 6,
    domain: null,
  },
  {
    key: "general.midiChannel", surface: "general", label: "MIDI Channel", instances: 1,
    kind: "select", draws: "dropdown", detents: null,
    domain: null,
  },
  {
    key: "general.midiClockOut", surface: "general", label: "MIDI Clock Out", instances: 1,
    kind: "select", draws: "dropdown", detents: null,
    domain: null,
  },
  {
    key: "general.toggle", surface: "general", label: "device boolean", instances: 12,
    kind: "switch", draws: "option-pair", detents: 2,
    domain: null,
  },
  {
    key: "general.globalBypass", surface: "general", label: "Global Bypass", instances: 8,
    kind: "button", draws: "icon-button", detents: null,
    domain: null,
  },
  {
    key: "general.sceneBypassBehavior", surface: "general", label: "Scene Bypass Behavior", instances: 1,
    kind: "select", draws: "radio-tile", detents: 3,
    domain: null,
  },
  {
    key: "tuner.referenceHz", surface: "tuner", label: "FREQ [Hz]", instances: 1,
    kind: "knob", draws: "arc-rotary", detents: null,
    domain: null,
  },
  {
    key: "tuner.inputPort", surface: "tuner", label: "INPUT 1", instances: 1,
    kind: "select", draws: "dropdown", detents: null,
    domain: null,
  },
  {
    key: "tuner.mute", surface: "tuner", label: "MUTED", instances: 1,
    kind: "button", draws: "icon-button", detents: null,
    domain: null,
  },
  {
    key: "tuner.meterEnabled", surface: "tuner", label: "LIVE TUNER", instances: 1,
    kind: "switch", draws: "option-pair", detents: 2,
    domain: null,
  },
  {
    key: "masterVolume.level", surface: "masterVolume", label: "Master Volume", instances: 1,
    kind: "knob", draws: null, detents: null,
    domain: null,
  },
  {
    key: "masterVolume.assignment", surface: "masterVolume", label: "Master Volume Knob Assignment", instances: 4,
    kind: "button", draws: "check-tile", detents: null,
    domain: null,
  }
] as const satisfies readonly DeviceSettingControl[];

export type DeviceSettingKey = (typeof DEVICE_SETTING_CONTROL_TABLE)[number]["key"];

export const DEVICE_SETTING_CONTROLS: readonly (DeviceSettingControl & { key: DeviceSettingKey })[] = DEVICE_SETTING_CONTROL_TABLE;

/** The declared row for one of the unit's settings controls. */
export function deviceSettingControl(key: DeviceSettingKey): DeviceSettingControl {
  const control = DEVICE_SETTING_CONTROLS.find((entry) => entry.key === key);
  if (!control) throw new Error(`${key} is not a declared device-setting control.`);
  return control;
}

/**
 * The five filter shapes an EQ band chooses between, in the unit's own order
 * and its own casing - which is not consistent, and is reproduced rather than
 * tidied: Peak, Hi pass, Lo pass, Hi Shelf, Lo Shelf. The closed trigger
 * upper-cases this same list; the open selector's rows do not.
 *
 * The ModelRepo does not carry them. All sixteen of its EQ TYPE parameters are
 * declared `float` 0..1 with `steps: 5` and an empty `options` array, so a
 * derivation from the declaration alone renders a knob with no labels at all,
 * which is what this used to do.
 *
 * Nothing ties a row to a wire value, so index 0..4 spread across 0..1 is the
 * arithmetic `steps: 5` implies, not a confirmed mapping.
 */
export const EQ_FILTER_TYPE_LABELS = ["Peak", "Hi pass", "Lo pass", "Hi Shelf", "Lo Shelf"] as const;

/**
 * An EQ band's filter-type chooser, which the unit draws as an icon dropdown.
 *
 * The test is exact over the whole snapshot rather than a heuristic: of 8851
 * parameters across 633 models, `steps === 5` occurs on sixteen and all sixteen
 * are Equalizer `N TYPE`. No other float carries five steps, and `steps` on a
 * float is otherwise the wire quantisation - 360 on OUTPUT VOLUME, 1001 on MIX -
 * so the count is what identifies this parameter, not the type word.
 */
export function parameterIsEqFilterType(parameter: Pick<BlockParameter, "type" | "options" | "steps">): boolean {
  return parameter.type.trim().toLowerCase() === "float" && parameter.steps === 5 && parameter.options.length === 0;
}

/**
 * The labels a chooser offers. The device supplies them for every other
 * control; the EQ filter type is the one where the repository has to.
 */
export function parameterOptionLabels(parameter: Pick<BlockParameter, "type" | "options" | "steps">): readonly string[] {
  if (parameter.options.length) return parameter.options;
  return parameterIsEqFilterType(parameter) ? EQ_FILTER_TYPE_LABELS : parameter.options;
}

export function parameterControlKind(parameter: Pick<BlockParameter, "type" | "options" | "steps">): ParameterControlKind;
export function parameterControlKind(parameter: { settingKey: DeviceSettingKey }): ParameterControlKind;
export function parameterControlKind(parameter: Pick<BlockParameter, "type" | "options" | "steps"> | { settingKey: DeviceSettingKey }): ParameterControlKind {
  // Not a BlockDetails parameter: there is no declared type to read, so the
  // hand-maintained table above answers instead of a guess from the payload.
  if ("settingKey" in parameter) return deviceSettingControl(parameter.settingKey).kind;
  // An EQ band's filter type is declared a bare float, and the unit draws it as
  // the icon dropdown global-eq-filter-type opens. It is settled before the type
  // word, which would otherwise fall through to a knob.
  if (parameterIsEqFilterType(parameter)) return "select";
  const kind = parameter.type.trim().toLowerCase();
  if (kind === "grmeter") return "meter";
  if (kind === "fader") return "fader";
  if (kind === "rotaryswitch") return "knob";
  if (kind === "togglebutton") return "button";
  if (kind === "switch") return "switch";
  if (kind === "combobox" || kind === "string" || kind === "enum") return "select";
  if (parameter.options.length === 2) return "switch";
  if (parameter.options.length > 2) return "select";
  return "knob";
}

export function parameterEditorFamily(category: string): ParameterEditorFamily {
  const key = category.trim().toLowerCase();
  if (/compress/.test(key)) return "compressor";
  if (/delay/.test(key)) return "delay";
  if (/\beq\b|equalizer/.test(key)) return "eq";
  if (/modulation/.test(key)) return "modulation";
  if (/pitch/.test(key)) return "pitch";
  if (/reverb/.test(key)) return "reverb";
  if (/synth/.test(key)) return "synth";
  if (/\bcab\b/.test(key)) return "cab";
  if (/\bir\b|irloader|impulse/.test(key)) return "ir";
  if (/looper/.test(key)) return "looper";
  return "standard";
}

export function parameterEditorIsFullScreen(category: string): boolean {
  return ["eq", "cab", "ir", "looper"].includes(parameterEditorFamily(category));
}

export function parameterEditorPageSize(category: string, parameters: ReadonlyArray<Pick<BlockParameter, "type">> = []): number {
  const family = parameterEditorFamily(category);
  // Looper X Params mode is three hardware pages of five controls.
  if (family === "looper") return 5;
  if (family === "eq" && parameters.some((parameter) => parameter.type.toLowerCase() === "fader")) return 10;
  if (family === "eq") return 5;
  return 10;
}

/** Grid category color and editor control color are not always the same on CorOS. */
export function parameterEditorAccent(name: string, fallback: string): string {
  return /adaptive\s+gate/i.test(name) ? QC_COLORS.hardware.whiteLed : fallback;
}

export function parameterEditorTabs(name: string, category: string, pageCount: number): string[] {
  if (pageCount <= 1) return [];
  const family = parameterEditorFamily(category);
  if (family === "cab") return ["CAB", "EQ"];
  if (family === "ir" || family === "looper") return Array.from({ length: pageCount }, (_, index) => String(index + 1));
  if (family === "eq") return Array.from({ length: pageCount }, (_, index) => index === pageCount - 1 ? "OUT" : `BAND ${index + 1}`);
  // The unit's synth editor footer carries four cards, not five, and the first
  // is GLOBAL & AMP rather than MAIN: `editor-overlord-synth` shows
  // GLOBAL & AMP / ARPEGGIATOR / OSCILLATOR / FILTER and draws no ENVELOPE
  // card - the envelope is a control on the first page, not a page of its own.
  // The reconstructed fixture already names them that way; this is the shared
  // list every other consumer reads.
  if (/synth|overlord/i.test(`${category} ${name}`)) {
    const labels = ["GLOBAL & AMP", "ARPEGGIATOR", "OSCILLATOR", "FILTER"];
    return Array.from({ length: pageCount }, (_, index) => labels[index] ?? `PAGE ${index + 1}`);
  }
  return Array.from({ length: pageCount }, (_, index) => index === 0 ? "MAIN" : `PAGE ${index + 1}`);
}

function clamp(value: number) { return Math.max(0, Math.min(1, value)); }

export function parameterRealValue(parameter: BlockParameter, normalized: number): number {
  const clamped = clamp(normalized);
  if (parameter.valueScale === "lookup" && parameter.scalePoints?.length) {
    for (let index = 1; index < parameter.scalePoints.length; index += 1) {
      const left = parameter.scalePoints[index - 1];
      const right = parameter.scalePoints[index];
      if (clamped <= right.normalized) {
        const span = right.normalized - left.normalized;
        const ratio = span === 0 ? 0 : (clamped - left.normalized) / span;
        return left.real + ratio * (right.real - left.real);
      }
    }
    return parameter.scalePoints.at(-1)!.real;
  }
  if (parameter.valueScale === "logarithmic" && parameter.minimum > 0 && parameter.maximum > parameter.minimum) {
    return parameter.minimum * Math.pow(parameter.maximum / parameter.minimum, clamped);
  }
  const amount = parameter.valueScale === "power"
    ? Math.pow(clamped, parameter.scaleExponent ?? 1)
    : clamped;
  return parameter.minimum + amount * (parameter.maximum - parameter.minimum);
}

export function parameterNormalizedValue(parameter: BlockParameter, real: number): number {
  if (!Number.isFinite(real)) throw new Error(`${parameter.name} must be a number.`);
  if (real < parameter.minimum || real > parameter.maximum) {
    throw new Error(`${parameter.name} must be from ${parameter.minimum} through ${parameter.maximum}${parameter.units ? ` ${parameter.units}` : ""}.`);
  }
  if (parameter.valueScale === "lookup" && parameter.scalePoints?.length) {
    const ascending = parameter.scalePoints.at(-1)!.real >= parameter.scalePoints[0].real;
    for (let index = 1; index < parameter.scalePoints.length; index += 1) {
      const left = parameter.scalePoints[index - 1];
      const right = parameter.scalePoints[index];
      if ((ascending && real <= right.real) || (!ascending && real >= right.real)) {
        const span = right.real - left.real;
        const ratio = span === 0 ? 0 : (real - left.real) / span;
        return clamp(left.normalized + ratio * (right.normalized - left.normalized));
      }
    }
    return clamp(parameter.scalePoints.at(-1)!.normalized);
  }
  const span = parameter.maximum - parameter.minimum;
  if (span <= 0) return 0;
  if (parameter.valueScale === "logarithmic") {
    if (parameter.minimum <= 0 || real <= 0) throw new Error(`${parameter.name} has an invalid logarithmic range.`);
    return clamp(Math.log(real / parameter.minimum) / Math.log(parameter.maximum / parameter.minimum));
  }
  const amount = clamp((real - parameter.minimum) / span);
  return parameter.valueScale === "power"
    ? Math.pow(amount, 1 / (parameter.scaleExponent ?? 1))
    : amount;
}

export function parameterStep(parameter: BlockParameter): number {
  const labels = parameterOptionLabels(parameter);
  if (labels.length > 1) return 1 / (labels.length - 1);
  if (parameter.steps && parameter.steps > 1) return 1 / (parameter.steps - 1);
  return .01;
}

/**
 * A left / centre / right control is written by the unit as a magnitude and a
 * side ("15 L", "C", "50 R") over a fixed span of fifty either way, whatever
 * numeric range the catalog declares for it.
 */
function labelledEndDisplay(parameter: BlockParameter, normalized: number): string | undefined {
  const { minimumLabel, midpointLabel, maximumLabel } = parameter;
  if (!minimumLabel || !midpointLabel || !maximumLabel || parameter.valueScale !== "linear") return undefined;
  // The extremes and the centre keep the bare label the unit measured there.
  if (normalized <= .000001) return minimumLabel;
  if (normalized >= .999999) return maximumLabel;
  const signed = Math.round((clamp(normalized) - .5) * 100);
  if (signed === 0) return midpointLabel;
  return signed < 0 ? `${-signed} ${minimumLabel}` : `${signed} ${maximumLabel}`;
}

export function parameterDisplay(parameter: BlockParameter, normalized: number): string {
  const labels = parameterOptionLabels(parameter);
  if (labels.length > 1) {
    return labels[Math.round(clamp(normalized) * (labels.length - 1))] ?? parameter.displayValue;
  }
  if (parameter.type.toLowerCase() === "togglebutton") return normalized >= .5 ? "ON" : "OFF";
  const labelledEnd = labelledEndDisplay(parameter, normalized);
  if (labelledEnd) return labelledEnd;
  if (normalized <= .000001 && parameter.minimumLabel) return parameter.minimumLabel;
  if (Math.abs(normalized - .5) <= .000001 && parameter.midpointLabel) return parameter.midpointLabel;
  if (normalized >= .999999 && parameter.maximumLabel) return parameter.maximumLabel;
  const real = parameterRealValue(parameter, normalized);
  // CorOS normally keeps one decimal on continuous knobs (for example 5.0).
  // Explicit device metadata remains authoritative for integer and finer values.
  const ordinaryHertz = parameter.units.trim().toLowerCase() === "hz" && parameter.maximum > 20;
  const precision = parameter.displayPrecision ?? (ordinaryHertz ? 0 : 1);
  const rendered = real.toFixed(precision);
  const value = Number(rendered) === 0 ? (0).toFixed(precision) : rendered;
  return `${value}${parameter.units ? ` ${parameter.units}` : ""}`;
}
