/**
 * The model catalogue a virtual Quad Cortex is built from, and the parameter
 * values a freshly placed virtual block starts with.
 *
 * The catalogue is the shape of `docs/reference/qc-live-parameter-scales.json`
 * (633 models, 8851 parameters, ranges and scales, no device identity). Only
 * the fields read here are declared; the rest of each record
 * passes through untouched. Nothing in this module fetches or bundles that
 * file: the caller that creates a virtual device hands the parsed document in,
 * so a test reads it from disk and an app decides how and when to load 7.5 MB.
 *
 * A virtual block's starting values come from one of two sources, and the two
 * must never be confused:
 *
 * - DEVICE-DERIVED. `docs/reference/qc-live-parameter-defaults.json` holds the
 *   known factory-default value of every parameter of 434 of the catalogue's
 *   models ({@link MeasuredParameterDefaults}). For those models
 *   {@link defaultParameterValues} returns the model's own values.
 * - SYNTHESIZED. Models with no factory default in the public data use
 *   deterministic starting settings - the middle of each range,
 *   the first position of each list - from {@link synthesizeParameterDefault},
 *   and must never be quoted as how a real Quad Cortex initialises that model.
 *   The same guess fills any parameter a measured model's record lacks.
 *
 * Like the catalogue, the device-derived defaults document is handed in by the caller;
 * nothing here fetches it.
 */

export interface VirtualCatalogueParameter {
  index: number;
  name: string;
  type: string;
  minimum: number;
  maximum: number;
  valueScale: string;
  scaleExponent?: number | null;
  scalePoints?: Array<{ normalized: number; real: number }>;
  displayPrecision?: number | null;
  scaleKnown?: boolean;
  units?: string;
  steps?: number | null;
  options: string[];
  minimumLabel?: string | null;
  midpointLabel?: string | null;
  maximumLabel?: string | null;
  showAsInteger?: boolean;
  expressionAssignable?: boolean;
  linkedSceneMode?: number | null;
  screenVisible?: boolean;
  displayPosition?: number;
}

export interface VirtualCatalogueModel {
  id: number;
  name: string;
  category: string;
  hidden: boolean;
  parameters: VirtualCatalogueParameter[];
}

export interface VirtualModelCatalogue {
  models: VirtualCatalogueModel[];
}

/**
 * Parameter types whose positions are discrete states rather than a range, so
 * "the middle" means nothing: a two-position switch has no midpoint, and a
 * `string` parameter (IR and capture names) addresses a list this catalogue
 * does not carry. They start at their first position, like an options list.
 */
const FIRST_POSITION_TYPES = new Set(["switch", "toggleButton", "string", "empty"]);

/** Map a real value inside [minimum, maximum] onto the normalized 0..1 wire value. */
function realToNormalized(parameter: VirtualCatalogueParameter, real: number): number {
  const span = parameter.maximum - parameter.minimum;
  if (!Number.isFinite(span) || span <= 0) return 0;
  const linear = (real - parameter.minimum) / span;
  // Power scale: real = minimum + span * normalized ** exponent.
  if (parameter.valueScale === "power" && typeof parameter.scaleExponent === "number" && parameter.scaleExponent > 0) {
    return Math.pow(linear, 1 / parameter.scaleExponent);
  }
  // Linear, and the fallback for `lookup`/`logarithmic`, whose mapping no
  // local source records in closed form.
  return linear;
}

/** The inverse of {@link realToNormalized}, used to render a display value. */
export function normalizedToReal(parameter: VirtualCatalogueParameter, normalized: number): number {
  const span = parameter.maximum - parameter.minimum;
  if (parameter.valueScale === "power" && typeof parameter.scaleExponent === "number" && parameter.scaleExponent > 0) {
    return parameter.minimum + span * Math.pow(normalized, parameter.scaleExponent);
  }
  return parameter.minimum + span * normalized;
}

/**
 * A SYNTHESIZED starting value for one parameter, as the normalized 0..1 value
 * every gateway parameter call carries (never the displayed number).
 *
 * - An options list, or a switch-like type, starts at its first position: 0.
 * - A numeric range starts at the real midpoint of `minimum`..`maximum`,
 *   converted through the parameter's scale, so a power-scaled control sits at
 *   the middle of its displayed range rather than at normalized 0.5.
 * - A stepped range (`steps` of 2 or more) snaps that midpoint to the nearest
 *   of its `steps` positions.
 *
 * This is not the device's factory default; see the module comment.
 */
export function synthesizeParameterDefault(parameter: VirtualCatalogueParameter): number {
  if (parameter.options.length > 0 || FIRST_POSITION_TYPES.has(parameter.type)) return 0;
  const midpoint = (parameter.minimum + parameter.maximum) / 2;
  const normalized = Math.min(1, Math.max(0, realToNormalized(parameter, midpoint)));
  const steps = parameter.steps ?? 0;
  if (steps >= 2) return Math.round(normalized * (steps - 1)) / (steps - 1);
  return normalized;
}

/**
 * SYNTHESIZED starting values for every parameter a model declares, keyed by
 * parameter index. An approximation, not the device's factory state.
 */
export function synthesizeParameterValues(model: VirtualCatalogueModel | undefined): Record<number, number> {
  const values: Record<number, number> = {};
  for (const parameter of model?.parameters ?? []) values[parameter.index] = synthesizeParameterDefault(parameter);
  return values;
}

/** One parameter's known factory default. */
export interface MeasuredParameterDefault {
  index: number;
  name: string;
  /** The normalized 0..1 wire value, the same unit as every `parameterValues` map. */
  normalizedValue: number;
}

export interface MeasuredModelDefaults {
  name: string;
  category: string;
  parameters: MeasuredParameterDefault[];
}

/**
 * The shape of `docs/reference/qc-live-parameter-defaults.json`: known factory
 * defaults, keyed by model id as a string. Models with no known default are
 * simply absent.
 */
export interface MeasuredParameterDefaults {
  schema: number;
  source: "device-derived compatibility data";
  models: Record<string, MeasuredModelDefaults>;
}

/**
 * The starting values for every parameter a model declares, keyed by
 * parameter index: the unit's measured factory default wherever
 * `measured` has one, and the synthesized approximation otherwise.
 *
 * Only indices the catalogue declares for the model are returned. A measured
 * record also lists one slot the catalogue does not describe (a bypass or an
 * unnamed placeholder); the virtual device has no parameter to put it in, so
 * it is left out rather than invented.
 */
export function defaultParameterValues(model: VirtualCatalogueModel | undefined, measured?: MeasuredParameterDefaults): Record<number, number> {
  const values = synthesizeParameterValues(model);
  if (!model || !measured) return values;
  for (const parameter of measured.models[String(model.id)]?.parameters ?? []) {
    const value = parameter.normalizedValue;
    if (values[parameter.index] !== undefined && Number.isFinite(value) && value >= 0 && value <= 1) values[parameter.index] = value;
  }
  return values;
}
