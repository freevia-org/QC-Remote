/** A direct MCP tool's input shape, built from the shared action contract.
 *
 * The cloud registers `set_tempo` as a tool of its own as well as through
 * `execute_qc_action`. Its advertised schema used to be written by hand here
 * and drifted from the contract (no minimum length on `expected_preset_name`,
 * a different description). Building it from `SHARED_QC_ACTIONS` keeps the two
 * identical. Only the keywords these schemas use are converted; anything else
 * throws at startup rather than being advertised loosely. */
import { z } from "zod";
import { SHARED_QC_ACTIONS } from "../../../packages/typescript/qc-core/src/generated-actions.ts";

const CONVERTED_KEYWORDS = new Set(["type", "minimum", "maximum", "minLength", "maxLength", "description"]);

export function sharedTool(name: string): { description: string; shape: Record<string, z.ZodType> } {
  const action = SHARED_QC_ACTIONS.find((candidate) => candidate.name === name);
  if (!action) throw new Error(`The shared action catalog has no ${name}`);
  const properties = action.inputSchema.properties as Record<string, Record<string, unknown>>;
  const shape = Object.fromEntries(Object.entries(properties).map(([field, schema]) => [field, zodForProperty(`${name}.${field}`, schema)]));
  return { description: action.description, shape };
}

function zodForProperty(path: string, schema: Record<string, unknown>): z.ZodType {
  const unknown = Object.keys(schema).filter((key) => !CONVERTED_KEYWORDS.has(key));
  if (unknown.length > 0) throw new Error(`${path} uses unsupported schema keywords: ${unknown.join(", ")}`);
  let field: z.ZodType;
  if (schema.type === "integer") {
    let integer = z.number().int();
    if (typeof schema.minimum === "number") integer = integer.min(schema.minimum);
    if (typeof schema.maximum === "number") integer = integer.max(schema.maximum);
    field = integer;
  } else if (schema.type === "string") {
    let text = z.string();
    if (typeof schema.minLength === "number") text = text.min(schema.minLength);
    if (typeof schema.maxLength === "number") text = text.max(schema.maxLength);
    field = text;
  } else {
    throw new Error(`${path} has unsupported type ${String(schema.type)}`);
  }
  return typeof schema.description === "string" ? field.describe(schema.description) : field;
}
