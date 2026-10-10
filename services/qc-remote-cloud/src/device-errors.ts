import { ProtocolError, ProtocolErrorCode } from "@modelcontextprotocol/server";

/**
 * A failure returned by the paired device host, retaining its stable code,
 * whether it is retryable, and - for the two unsaved-state refusals only -
 * whether the user has to act on the device.
 *
 * `userActionRequired` is derived here from the exact device code and nothing
 * else. An incoming frame's own userActionRequired, and its message, can never
 * create, remove or change it: any other code leaves it absent whatever the
 * frame says. For the derived codes the derived retryable also overrides a
 * conflicting incoming one, as the hosts derive both from the same code. Every
 * other code keeps the frame's boolean retryable, false when it sent none (the
 * Rust frame's default).
 *
 * The frame's `error.details` contributes ONLY the allowlisted dispatch
 * and quarantine facts below, each strictly typed and absent when missing or
 * malformed (absent is not false). No other detail, and nothing from the top
 * level besides code, message and retryable, ever reaches a projection.
 */
export type DeviceInvocationError = Error & DeviceDispatchFacts & {
  deviceErrorCode: string;
  retryable: boolean;
  userActionRequired?: boolean;
};

export type DeviceQuarantine = { scope: "presetGrid" | "device"; sinceUnixMs: number; sessionEpoch: number };

/** Verified-safe dispatch and quarantine facts a host may put in error.details. */
export type DeviceDispatchFacts = {
  requestSent?: boolean;
  writesQuarantined?: boolean;
  quarantine?: DeviceQuarantine;
};

/** The one typed device-error projection every MCP surface emits. */
export type DeviceErrorProjection = DeviceDispatchFacts & {
  deviceErrorCode: string;
  retryable: boolean;
  userActionRequired?: boolean;
};

const DEVICE_ERROR_CODE = /^[A-Z][A-Z0-9_]{0,63}$/;

/** Codes whose retry and action policy the cloud derives from the code alone. */
const DERIVED_POLICY: Readonly<Record<string, { retryable: boolean; userActionRequired?: boolean }>> = {
  UNSAVED_STATE_UNCONFIRMED: { retryable: false, userActionRequired: true },
  UNSAVED_STATE_NOT_YET_OBSERVED: { retryable: true, userActionRequired: false },
  // An admission refusal: retrying cannot succeed until the quarantine clears.
  WRITE_QUARANTINED: { retryable: false },
};

const QUARANTINE_SCOPES: ReadonlySet<string> = new Set(["presetGrid", "device"]);

/** The legacy getDeviceInvocationError shape, unchanged (`code`, not `deviceErrorCode`). */
export type DeviceInvocationFailure = {
  code: string;
  message: string;
  retryable: boolean;
  userActionRequired?: boolean;
};

/** Facts the device error frame may carry besides code and message. */
export type DeviceInvocationFlags = { retryable?: unknown; details?: unknown };

function policyFor(code: string, incomingRetryable: unknown): { retryable: boolean; userActionRequired?: boolean } {
  const derived = Object.hasOwn(DERIVED_POLICY, code) ? DERIVED_POLICY[code] : undefined;
  if (derived) {
    return derived.userActionRequired === undefined
      ? { retryable: derived.retryable }
      : { retryable: derived.retryable, userActionRequired: derived.userActionRequired };
  }
  return { retryable: typeof incomingRetryable === "boolean" ? incomingRetryable : false };
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isSafeNonNegativeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

/**
 * {scope, sinceUnixMs, sessionEpoch}, all three present and valid, or nothing:
 * an invalid object (a missing field, a wrong type, a bad scope, an unsafe
 * integer) is omitted whole, never partially kept. Extra inner keys are
 * stripped: the result is built from the three fields alone.
 */
function quarantineFrom(value: unknown): DeviceQuarantine | undefined {
  if (!isPlainObject(value)) return undefined;
  const { scope, sinceUnixMs, sessionEpoch } = value;
  if (typeof scope !== "string" || !QUARANTINE_SCOPES.has(scope)) return undefined;
  if (!isSafeNonNegativeInteger(sinceUnixMs) || !isSafeNonNegativeInteger(sessionEpoch)) return undefined;
  return { scope: scope as DeviceQuarantine["scope"], sinceUnixMs, sessionEpoch };
}

/**
 * Only the allowlisted keys, each strictly typed; everything else is dropped.
 * The code never creates a fact: one appears only when the host supplied a
 * strictly valid value, and a supplied one is preserved for every code,
 * including the ambiguous outcomes (WRITE_OUTCOME_AMBIGUOUS can honestly mean
 * "sent, effect unknown"), unrelated codes and DEVICE_ERROR.
 *
 * The one exception is exact WRITE_QUARANTINED (the host's name for proven
 * broker -32012, an admission refusal before dispatch): its pre-dispatch
 * invariant takes precedence over a contradicting detail, so requestSent is kept
 * only when false and writesQuarantined only when true, and a contradicting
 * value is dropped rather than flipped. Nested codes are never read at all.
 */
function dispatchFactsFor(code: string, source: unknown): DeviceDispatchFacts {
  if (!isPlainObject(source)) return {};
  const facts: DeviceDispatchFacts = {};
  const quarantineRefusal = code === "WRITE_QUARANTINED";
  if (typeof source.requestSent === "boolean" && !(quarantineRefusal && source.requestSent)) facts.requestSent = source.requestSent;
  if (typeof source.writesQuarantined === "boolean" && !(quarantineRefusal && !source.writesQuarantined)) {
    facts.writesQuarantined = source.writesQuarantined;
  }
  const quarantine = quarantineFrom(source.quarantine);
  if (quarantine) facts.quarantine = quarantine;
  return facts;
}

/** Legacy helper, shape unchanged; MCP egress uses deviceErrorProjection. */
export function getDeviceInvocationError(error: unknown): DeviceInvocationFailure | undefined {
  const projection = deviceErrorProjection(error);
  if (!projection) return undefined;
  const failure: DeviceInvocationFailure = { code: projection.deviceErrorCode, message: (error as Error).message, retryable: projection.retryable };
  if (projection.userActionRequired !== undefined) failure.userActionRequired = projection.userActionRequired;
  return failure;
}

/**
 * Build the projection from an error that may have crossed the Durable Object
 * RPC boundary: nothing there guarantees the property types, so the code alone
 * decides the policy again and every fact is re-validated.
 */
export function deviceErrorProjection(error: unknown): DeviceErrorProjection | undefined {
  if (!(error instanceof Error)) return undefined;
  const typed = error as Partial<DeviceInvocationError>;
  const code = typed.deviceErrorCode;
  if (typeof code !== "string" || !DEVICE_ERROR_CODE.test(code)) return undefined;
  return { deviceErrorCode: code, ...policyFor(code, typed.retryable), ...dispatchFactsFor(code, typed) };
}

export function deviceInvocationError(code: unknown, message: unknown, flags?: DeviceInvocationFlags): DeviceInvocationError {
  const safeCode = typeof code === "string" && DEVICE_ERROR_CODE.test(code)
    ? code
    : "DEVICE_ERROR";
  const safeMessage = typeof message === "string" && message.length > 0
    ? message
    : "The device rejected the request.";
  return Object.assign(new Error(safeMessage), {
    deviceErrorCode: safeCode,
    ...policyFor(safeCode, flags?.retryable),
    ...dispatchFactsFor(safeCode, flags?.details),
  });
}

/** Preserve typed device failures in the established MCP tool-error result. */
export function deviceInvocationToolError(error: unknown) {
  const projection = deviceErrorProjection(error);
  if (!projection) return undefined;
  return {
    isError: true as const,
    content: [{ type: "text" as const, text: (error as Error).message }],
    structuredContent: projection
  };
}

/**
 * The MCP resource-read error for a typed device failure: a JSON-RPC error with
 * code -32603 (InternalError) and the projection as `data`, which is where the
 * local rust-mcp server puts its resource error facts too.
 */
export function deviceInvocationResourceError(error: unknown): ProtocolError | undefined {
  const projection = deviceErrorProjection(error);
  if (!projection) return undefined;
  return new ProtocolError(ProtocolErrorCode.InternalError, (error as Error).message, projection);
}

/** Run a tool handler; a typed device failure becomes the tool-error result. */
export async function deviceToolCall<T>(run: () => Promise<T>): Promise<T | NonNullable<ReturnType<typeof deviceInvocationToolError>>> {
  try {
    return await run();
  } catch (reason) {
    const toolError = deviceInvocationToolError(reason);
    if (toolError) return toolError;
    throw reason;
  }
}

/** Run a resource read; a typed device failure becomes the resource error. */
export async function deviceResourceRead<T>(run: () => Promise<T>): Promise<T> {
  try {
    return await run();
  } catch (reason) {
    throw deviceInvocationResourceError(reason) ?? reason;
  }
}
