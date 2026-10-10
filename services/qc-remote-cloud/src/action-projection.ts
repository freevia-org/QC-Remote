/** Turn an MCP action call into the gateway request a paired host will accept.
 *
 * Kept out of `index.ts` so it can be exercised without the Workers runtime:
 * the maintainers' contract tests (not yet published in this repository) pin
 * what this produces against the argument lists every generated gateway admits.
 *
 * The worker is deployed once and serves every paired host, but each host runs
 * whatever build is installed on it. A phone that has not been updated since
 * gateway API 23 rejects `expectedModelId` outright ("Unexpected argument for
 * device.setParameter"), while this worker's catalog, generated from API 24,
 * requires `expected_model_id` for the same action. Forwarding the current
 * projection unchanged therefore locked an older host out of every parameter
 * and bypass write. `adaptForHost` projects a request down to the API version
 * the host itself reports, and keeps the guard the caller supplied by checking
 * it here instead of dropping it.
 */
import { SHARED_QC_ACTIONS } from "../../../packages/typescript/qc-core/src/generated-actions.ts";
import gatewayContract from "../../../contracts/public/gateway-methods.v1.json" with { type: "json" };
import { describeViolations, schemaViolations } from "./action-schema.ts";

export type Json = Record<string, unknown>;
export type QcAction = (typeof SHARED_QC_ACTIONS)[number];
export type GatewayArgument = string | { name: string; default?: unknown };
export type GatewayMethod = { rpc: string; args?: GatewayArgument[] };

/** The gateway API this worker's catalog was generated from. */
export const CURRENT_GATEWAY_API_VERSION: number = gatewayContract.apiVersion;

export const qcActionsByName = new Map<string, QcAction>(SHARED_QC_ACTIONS.map((action) => [action.name, action]));
export const qcActionsByRpc = new Map<string, QcAction>(SHARED_QC_ACTIONS.map((action) => [action.rpc, action]));
export const gatewayMethodsByRpc = new Map<string, GatewayMethod>(
  (gatewayContract.methods as GatewayMethod[]).map((method) => [method.rpc, method])
);

export function snakeCase(value: string): string {
  return value.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
}

/** The action input a gateway argument is read from. */
export function actionSourceFor(actionName: string, target: string): string {
  return actionName === "rename_current_preset" && target === "name" ? "new_name" : snakeCase(target);
}

/** Which gateway arguments `projectAction` can send for `action`, and which it
 * always sends. The contract tests hold these against every generated gateway's
 * accepted and required argument lists. */
export function projectedArguments(action: QcAction): { possible: string[]; always: string[] } {
  const gatewayMethod = gatewayMethodsByRpc.get(action.rpc);
  if (!gatewayMethod) return { possible: [], always: [] };
  const properties = new Set(Object.keys(action.inputSchema.properties));
  const required = new Set<string>(action.required);
  const possible: string[] = [];
  const always: string[] = [];
  for (const argument of gatewayMethod.args ?? []) {
    const target = typeof argument === "string" ? argument : argument.name;
    const source = actionSourceFor(action.name, target);
    const defaulted = target.startsWith("confirm") || (typeof argument !== "string" && "default" in argument);
    if (properties.has(source) || defaulted) possible.push(target);
    if (required.has(source) || defaulted) always.push(target);
  }
  return { possible, always };
}

function actionInputError(message: string): never {
  throw new Error(`Invalid QC action input: ${message}`);
}

/** Some MCP clients cannot emit a JSON `null` for a union-typed parameter and
 * send the four-character string `"null"` instead (`expected_model_id: "null"`
 * to `add_block`, seen from several MCP clients). The local server restores
 * the null the caller meant; this is the same rule, so a call accepted locally
 * is not refused here. Only where it is unambiguous: the property must accept
 * `null` and reject the text `"null"`, so a nullable string (`label`, `query`)
 * keeps the literal text, and every value is still validated afterwards. */
export function restoreStringifiedNulls(action: QcAction, rawParams: Record<string, unknown>): Record<string, unknown> {
  const properties = action.inputSchema.properties as Record<string, Record<string, unknown>>;
  let restored: Record<string, unknown> | undefined;
  for (const [key, value] of Object.entries(rawParams)) {
    const property = properties[key];
    if (value !== "null" || !property) continue;
    if (schemaViolations(property, null).length === 0 && schemaViolations(property, "null").length > 0) {
      restored ??= { ...rawParams };
      restored[key] = null;
    }
  }
  return restored ?? rawParams;
}

export function projectAction(actionName: string, suppliedParams: Record<string, unknown>): { action: QcAction; method: string; params: Json } {
  const action = qcActionsByName.get(actionName);
  if (!action) actionInputError(`unknown action \"${actionName}\"`);
  const rawParams = restoreStringifiedNulls(action, suppliedParams);
  const allowed = new Set(Object.keys(action.inputSchema.properties));
  for (const key of Object.keys(rawParams)) {
    if (!allowed.has(key)) actionInputError(`\"${key}\" is not accepted by ${action.name}`);
  }
  for (const key of action.required) {
    if (!(key in rawParams) || rawParams[key] === undefined) actionInputError(`\"${key}\" is required for ${action.name}`);
  }
  // Names were once the whole check here, so a tempo of 100000 and a scene
  // index of 99 both reached the device. The contract publishes the ranges,
  // enums, lengths and the dependent-argument rules that decide which
  // arguments go together; enforce them before anything leaves this worker.
  const violations = describeViolations(schemaViolations(action.inputSchema as Record<string, unknown>, rawParams));
  if (violations) actionInputError(`${action.name} was called with ${violations}`);
  if (action.classification === "risky-write" && rawParams.confirm_risky_operation !== true) {
    actionInputError(`${action.name} requires confirm_risky_operation: true`);
  }
  if (action.classification === "persistent-write" && rawParams.confirm_persistent_write !== true) {
    actionInputError(`${action.name} requires confirm_persistent_write: true`);
  }
  const gatewayMethod = gatewayMethodsByRpc.get(action.rpc);
  if (!gatewayMethod) actionInputError(`${action.name} has no generated gateway method`);
  const params: Json = {};
  for (const argument of gatewayMethod.args ?? []) {
    const target = typeof argument === "string" ? argument : argument.name;
    const source = actionSourceFor(action.name, target);
    if (source in rawParams) {
      params[target] = rawParams[source];
    } else if (target.startsWith("confirm")) {
      // Confirmation arguments which are not action inputs (currently
      // confirmRename) are enabled only after the action-level confirmation
      // above has been validated.
      params[target] = true;
    } else if (typeof argument !== "string" && "default" in argument) {
      params[target] = argument.default;
    }
  }
  return { action, method: action.rpc, params };
}

/** Private admin callers use the same action inputs and confirmations as MCP. */
export function projectAdminCommand(payload: Json): { action: string; method: string; params: Json } {
  const declared = typeof payload.method === "string" ? qcActionsByRpc.get(payload.method) : undefined;
  if (!declared) actionInputError("a declared contract method is required");
  const supplied = payload.params ?? {};
  if (typeof supplied !== "object" || supplied === null || Array.isArray(supplied)) {
    actionInputError("params must be an object");
  }
  const projected = projectAction(declared.name, supplied as Json);
  return { action: projected.action.name, method: projected.method, params: projected.params };
}

/** How the worker keeps a guard an older host cannot receive.
 *
 * `blockModel`: read `device.blockDetails` for the same row/column under the
 * same preset guards and compare its `modelId`, which is exactly where a
 * caller obtains `expected_model_id` in the first place.
 *
 * `refuse`: there is no way to keep it. The call is refused for a host too old
 * to receive the argument, rather than sent without it. */
export type LegacyGuardCheck = "blockModel" | "refuse";

export type GatewayArgumentHistoryEntry = {
  argument: string;
  introducedIn: number;
  methods: readonly string[];
  check: LegacyGuardCheck;
};

/** Gateway arguments added after hosts were already in the field.
 *
 * Every entry names the API version that first accepted the argument on the
 * listed methods. A host reporting an older `gatewayApiVersion` is sent the
 * request without it and the worker performs `check` itself; an entry whose
 * guard cannot be kept says `check: "refuse"`, and `adaptForHost` then throws
 * rather than forwarding the call without it - as it does for any check it
 * does not know how to perform.
 *
 * This table is the only record of when an argument arrived. An argument not
 * listed here is assumed to be accepted by every host in the field, which is
 * why `tests/remote-gateway-compat.test.ts` pins the gateway argument lists to
 * the API version: none can change without this table being revisited. */
export const GATEWAY_ARGUMENT_HISTORY: readonly GatewayArgumentHistoryEntry[] = [
  {
    // Binds parameter and bypass writes to the caller-observed model.
    argument: "expectedModelId",
    introducedIn: 24,
    methods: [
      "device.toggleBypass",
      "device.setExpressionBypass",
      "device.previewParameter",
      "device.resetBlockParameters",
      "device.setParameter",
      "device.setParameterSceneMode",
      "device.setParameterExpression"
    ],
    check: "blockModel"
  },
  {
    argument: "expectedModelId",
    introducedIn: 27,
    methods: ["device.replaceBlock"],
    check: "refuse"
  },
  {
    // These methods did not exist before API 27. A required argument is used
    // as the version sentinel so the worker refuses the whole method instead
    // of sending an unknown RPC to an older paired host.
    argument: "row",
    introducedIn: 27,
    methods: ["device.createModelPreset"],
    check: "refuse"
  },
  {
    argument: "assignments",
    introducedIn: 27,
    methods: ["device.setLooperStompAssignments"],
    check: "refuse"
  }
];

/** Arguments of `method` that a host at `hostApiVersion` does not accept. */
export function argumentsNewerThanHost(method: string, hostApiVersion: number): string[] {
  return GATEWAY_ARGUMENT_HISTORY
    .filter((entry) => entry.introducedIn > hostApiVersion && entry.methods.includes(method))
    .map((entry) => entry.argument);
}

/** True when `method` carries an argument some supported host version lacks. */
export function isVersionSensitive(method: string): boolean {
  return GATEWAY_ARGUMENT_HISTORY.some((entry) => entry.methods.includes(method));
}

export type LegacyModelGuard = {
  row: unknown;
  column: unknown;
  expectedModelId: unknown;
  /** The preset guards forwarded with the details read, so it reads the same preset the write targets. */
  detailsParams: Json;
};

export type HostAdaptation = { params: Json; modelGuard?: LegacyModelGuard };

/** Project an already-validated gateway request onto the host's API version.
 *
 * A host reporting the current version (or newer) receives the request as is.
 * An older host receives it without the arguments it would reject, and the
 * guard those arguments carried is returned for the worker to check first. */
export function adaptForHost(
  method: string,
  params: Json,
  hostApiVersion: number,
  history: readonly GatewayArgumentHistoryEntry[] = GATEWAY_ARGUMENT_HISTORY
): HostAdaptation {
  if (hostApiVersion >= CURRENT_GATEWAY_API_VERSION) return { params };
  const adapted: Json = { ...params };
  let modelGuard: LegacyModelGuard | undefined;
  for (const entry of history) {
    if (entry.introducedIn <= hostApiVersion || !entry.methods.includes(method)) continue;
    if (!(entry.argument in adapted)) continue;
    const value = adapted[entry.argument];
    delete adapted[entry.argument];
    switch (entry.check) {
      case "blockModel": {
        const detailsParams: Json = { row: params.row, column: params.column };
        if ("expectedPresetName" in params) detailsParams.expectedPresetName = params.expectedPresetName;
        if ("expectedPresetIdentity" in params) detailsParams.expectedPresetIdentity = params.expectedPresetIdentity;
        modelGuard = { row: params.row, column: params.column, expectedModelId: value, detailsParams };
        break;
      }
      default:
        // "refuse", and any check this function does not implement: the guard
        // cannot be kept, so the call must not go out without it.
        throw new Error(tooOldForArgument(method, entry.argument, entry.introducedIn, hostApiVersion));
    }
  }
  return modelGuard ? { params: adapted, modelGuard } : { params: adapted };
}

/** Why a call was refused for a host too old to receive one of its arguments. */
export function tooOldForArgument(method: string, argument: string, introducedIn: number, hostApiVersion: number): string {
  return `${method} with ${argument} needs QC Remote gateway API ${introducedIn} or newer, and the paired host reports API ${hostApiVersion}. Update QC Remote on the paired computer or phone, then retry.`;
}

/** The `modelId` a `device.blockDetails` reply reports, wherever the host put it. */
export function blockDetailsModelId(result: Json): number | undefined {
  for (const candidate of [result, result.details, result.block, result.result]) {
    if (typeof candidate !== "object" || candidate === null || Array.isArray(candidate)) continue;
    const modelId = (candidate as Json).modelId;
    if (typeof modelId === "number" && Number.isInteger(modelId)) return modelId;
  }
  return undefined;
}

/** Why a legacy host's write must not proceed, or undefined when the guard holds.
 *
 * The wording follows the native planner's own mismatch message so a caller
 * sees the same failure whichever side enforced the guard. */
export function legacyModelGuardFailure(guard: LegacyModelGuard, details: Json): string | undefined {
  const actual = blockDetailsModelId(details);
  if (actual === undefined) {
    return `The block at row ${String(guard.row)}, column ${String(guard.column)} did not report a model, so expected model ${String(guard.expectedModelId)} cannot be confirmed. Refresh and retry.`;
  }
  if (actual !== guard.expectedModelId) {
    return `The block at row ${String(guard.row)}, column ${String(guard.column)} changed: expected model ${String(guard.expectedModelId)}, found ${actual}. Refresh and retry.`;
  }
  return undefined;
}

/** A `system.status` reply's API version; hosts that predate the field are version 0. */
export function reportedGatewayApiVersion(status: Json): number {
  const value = status.gatewayApiVersion;
  return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : 0;
}
