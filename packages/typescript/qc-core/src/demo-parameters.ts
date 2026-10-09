import type { BlockDetails, BlockParameter, GridBlock } from "@qc-remote/client";
import { modelSchemas, schemas } from "./device-reference/demo-parameter-seeds.ts";

export type ParameterSeed = [name: string, value: number, minimum?: number, maximum?: number, units?: string, options?: string[], type?: BlockParameter["type"], displayPrecision?: number];

function parameter(seed: ParameterSeed, index: number): BlockParameter {
  const [name, normalizedValue, minimum = 0, maximum = 1, units = "", options = [], type, displayPrecision] = seed;
  return {
    index, displayPosition: index, name, normalizedValue, displayValue: "", units,
    type: type ?? (options.length ? "enum" : "float"), minimum, maximum,
    valueScale: options.length ? "options" : "unknown", scalePoints: [], scaleKnown: options.length > 0, displayPrecision,
    showAsInteger: false, hasOffPosition: false, floorDisplay: null, floorNormalized: null,
    steps: options.length ? options.length : null, sceneMode: index < 5,
    options, writable: true, enabled: true, expressionAssignable: true,
    wireValueKind: options.length ? "text" : "numeric",
    expression: null, expressionMinimum: null, expressionMaximum: null
  };
}

export function demoBlockDetails(block: GridBlock, scene: number): BlockDetails {
  const category = (block.category ?? block.kind).toLowerCase();
  const seeds = modelSchemas[block.name.toLowerCase()] ?? schemas[category] ?? schemas.utility;
  const parameters = seeds.map(parameter).map((candidate) => {
    if (/graphic-9/i.test(block.name) && /^(?:N)?(?:65|125|250|500|1K|2K|4K|8K|16K)\s*HZ$/i.test(candidate.name.replace(/\s/g, ""))) return { ...candidate, type: "fader" };
    if (/crying wah/i.test(block.name) && candidate.index === 0) return { ...candidate, expression: 1, expressionMinimum: 0, expressionMaximum: 1 };
    if (/dual octaver/i.test(block.name) && candidate.index === 0) return { ...candidate, expression: 2, expressionMinimum: .25, expressionMaximum: .8 };
    return candidate;
  });
  return {
    row: block.row,
    column: block.column,
    modelId: block.modelId ?? 0,
    name: block.name,
    category: block.category ?? "Utility",
    scene,
    parameters
  };
}
