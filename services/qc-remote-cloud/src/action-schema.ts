/** Check an action's arguments against the schema the contract publishes.
 *
 * The remote path used to check argument *names* and stop. Types, ranges,
 * enums, string lengths, array cardinality and the dependent-argument rules
 * that make a sparse I/O write legal were all enforced only on the local path,
 * by `services/rust-mcp`, so `set_tempo {bpm: 100000}` and
 * `select_scene {index: 99}` reached the device runtime unchecked while
 * the same calls were rejected locally. `list_qc_actions` advertises "its exact
 * input schema" for every action; this is what makes that true remotely.
 *
 * The supported keyword set is exactly what `contracts/public/qc-actions.v1.json`
 * currently generates. **Anything else is a violation, not a pass**: a schema
 * keyword this worker does not understand is a constraint it cannot enforce,
 * and silently ignoring one is how a validator comes to promise more than it
 * delivers. Adding a keyword to the contract therefore fails here until it is
 * implemented here, which is the intended way round.
 */

export interface SchemaViolation {
  /** Dotted path to the offending value, empty at the root. */
  path: string;
  message: string;
}

const SUPPORTED_KEYWORDS = new Set([
  "type", "properties", "required", "additionalProperties",
  "minimum", "maximum", "minLength", "maxLength",
  "enum", "const", "pattern",
  "items", "minItems", "maxItems", "uniqueItems",
  "anyOf", "allOf", "not",
  // Carried for humans and MCP clients; constrains nothing.
  "description", "title", "default"
]);

type Schema = Record<string, unknown>;

const at = (path: string, key: string | number): string =>
  typeof key === "number" ? `${path}[${key}]` : path ? `${path}.${key}` : key;

function matchesType(expected: string, value: unknown): boolean {
  switch (expected) {
    case "null": return value === null;
    case "boolean": return typeof value === "boolean";
    case "string": return typeof value === "string";
    case "integer": return typeof value === "number" && Number.isInteger(value);
    case "number": return typeof value === "number" && Number.isFinite(value);
    case "array": return Array.isArray(value);
    case "object": return typeof value === "object" && value !== null && !Array.isArray(value);
    default: return false;
  }
}

function sameValue(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (typeof left !== typeof right) return false;
  if (left === null || right === null) return false;
  if (typeof left !== "object") return false;
  return JSON.stringify(left) === JSON.stringify(right);
}

/** Whether every keyword anywhere in `schema` is one this validator enforces. */
function fullySupported(schema: unknown): boolean {
  if (typeof schema !== "object" || schema === null || Array.isArray(schema)) return false;
  return Object.entries(schema as Schema).every(([key, child]) => {
    if (!SUPPORTED_KEYWORDS.has(key)) return false;
    switch (key) {
      case "properties":
        return typeof child === "object" && child !== null && Object.values(child).every(fullySupported);
      case "items":
      case "not":
        return fullySupported(child);
      case "allOf":
      case "anyOf":
        return Array.isArray(child) && child.every(fullySupported);
      default:
        return true;
    }
  });
}

/** Every way `value` fails `schema`. Empty means it is acceptable. */
export function schemaViolations(schema: Schema, value: unknown, path = ""): SchemaViolation[] {
  const problems: SchemaViolation[] = [];
  const fail = (message: string) => problems.push({ path, message });

  for (const keyword of Object.keys(schema)) {
    if (!SUPPORTED_KEYWORDS.has(keyword)) {
      fail(`the contract constrains this with "${keyword}", which this relay cannot enforce`);
    }
  }
  if (problems.length > 0) return problems;

  if ("type" in schema) {
    const expected = Array.isArray(schema.type) ? schema.type as string[] : [schema.type as string];
    if (!expected.some((candidate) => matchesType(candidate, value))) {
      fail(`must be ${expected.join(" or ")}`);
      return problems;
    }
  }

  if ("const" in schema && !sameValue(value, schema.const)) {
    fail(`must be ${JSON.stringify(schema.const)}`);
  }
  if (Array.isArray(schema.enum) && !schema.enum.some((candidate) => sameValue(value, candidate))) {
    fail(`must be one of ${schema.enum.map((candidate) => JSON.stringify(candidate)).join(", ")}`);
  }

  if (typeof value === "number") {
    if (typeof schema.minimum === "number" && value < schema.minimum) fail(`must be at least ${schema.minimum}`);
    if (typeof schema.maximum === "number" && value > schema.maximum) fail(`must be at most ${schema.maximum}`);
  }

  if (typeof value === "string") {
    // Count code points, not UTF-16 units: an emoji is one character to the
    // person who typed it and the device's own limits are in characters.
    const length = [...value].length;
    if (typeof schema.minLength === "number" && length < schema.minLength) {
      fail(schema.minLength === 1 ? "must not be empty" : `must be at least ${schema.minLength} characters`);
    }
    if (typeof schema.maxLength === "number" && length > schema.maxLength) {
      fail(`must be at most ${schema.maxLength} characters`);
    }
    if (typeof schema.pattern === "string" && !new RegExp(schema.pattern, "u").test(value)) {
      fail("is not in the accepted format");
    }
  }

  if (Array.isArray(value)) {
    if (typeof schema.minItems === "number" && value.length < schema.minItems) fail(`must have at least ${schema.minItems} entries`);
    if (typeof schema.maxItems === "number" && value.length > schema.maxItems) fail(`must have at most ${schema.maxItems} entries`);
    if (schema.uniqueItems === true) {
      const seen = new Set(value.map((entry) => JSON.stringify(entry)));
      if (seen.size !== value.length) fail("must not repeat an entry");
    }
    if (schema.items && typeof schema.items === "object") {
      value.forEach((entry, index) => {
        problems.push(...schemaViolations(schema.items as Schema, entry, at(path, index)));
      });
    }
  }

  if (typeof value === "object" && value !== null && !Array.isArray(value)) {
    const object = value as Record<string, unknown>;
    const properties = (schema.properties ?? {}) as Record<string, Schema>;
    if (Array.isArray(schema.required)) {
      for (const key of schema.required as string[]) {
        if (!(key in object) || object[key] === undefined) problems.push({ path: at(path, key), message: "is required" });
      }
    }
    if (schema.additionalProperties === false) {
      for (const key of Object.keys(object)) {
        if (!(key in properties)) problems.push({ path: at(path, key), message: "is not accepted here" });
      }
    }
    // JSON Schema applies `properties` only to keys that are present, which is
    // what makes the `anyOf` branches below able to say "when the port is 4,
    // impedance must be null" without also demanding impedance.
    for (const [key, subschema] of Object.entries(properties)) {
      if (key in object && object[key] !== undefined) {
        problems.push(...schemaViolations(subschema, object[key], at(path, key)));
      }
    }
  }

  if (Array.isArray(schema.allOf)) {
    for (const subschema of schema.allOf as Schema[]) {
      problems.push(...schemaViolations(subschema, value, path));
    }
  }

  if ("not" in schema) {
    // `not` inverts its branch, so a keyword this relay cannot enforce inside
    // it would turn "cannot check" into "passes". Refuse that first. The local
    // server (services/rust-mcp/src/schema.rs) applies the same rule.
    if (!fullySupported(schema.not)) {
      fail("the contract excludes values here with a rule this relay cannot enforce");
    } else if (schemaViolations(schema.not as Schema, value, path).length === 0) {
      fail("is a value this action excludes");
    }
  }

  if (Array.isArray(schema.anyOf)) {
    const branches = schema.anyOf as Schema[];
    const satisfied = branches.some((subschema) => schemaViolations(subschema, value, path).length === 0);
    if (!satisfied) {
      // The branches are alternative shapes of one call — which ports take
      // which settings — so naming the first branch's complaint would point at
      // the wrong one. Report that no shape fits.
      fail("is not a combination this action accepts; check the action's input schema for which arguments go together");
    }
  }

  return problems;
}

/** One sentence naming every violation, or undefined when there are none. */
export function describeViolations(violations: readonly SchemaViolation[]): string | undefined {
  if (violations.length === 0) return undefined;
  return violations
    .map(({ path, message }) => (path ? `"${path}" ${message}` : message))
    .join("; ");
}
