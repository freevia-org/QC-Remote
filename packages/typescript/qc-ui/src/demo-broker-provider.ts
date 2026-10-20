import { GATEWAY_CLIENT_METHODS, type GatewayTransport } from "@qc-remote/client";
import { withVirtualGatewayAdmission } from "./virtual-gateway.ts";

/** Internal reverse-provider contract; not a new public MCP method/envelope. */
export interface DemoBrokerTarget {
  source: "demo";
  sessionId: string;
  generation: number;
}

export interface DemoBrokerRequest {
  target: DemoBrokerTarget;
  /** Strictly increasing within this registration. Never reuse after timeout. */
  sequence: number;
  method: string;
  params: Record<string, unknown>;
  expectedState?: Record<string, unknown>;
}

export class DemoBrokerProviderError extends Error {
  readonly retryable = false;
  readonly code: "TARGET_SUPERSEDED" | "INVALID_PROVIDER_REQUEST" | "METHOD_NOT_SUPPORTED" | "PROVIDER_BUSY" | "STATE_CONFLICT";
  constructor(code: DemoBrokerProviderError["code"], message: string) {
    super(message);
    this.code = code;
    this.name = "DemoBrokerProviderError";
  }
}

/**
 * Reverse-dispatch into the exact demo transport the editor already opened.
 * Pass `demoDeviceSession.virtual`, NEVER its switchable `gateway`. Native
 * adapters own authentication, registration lifetime, effect admission and
 * deadlines. They must withdraw this provider on window death and never retry
 * its failed/uncertain operation against physical hardware.
 */
export function createDemoBrokerProvider(
  transport: GatewayTransport,
  registration: DemoBrokerTarget,
  isCurrent: () => boolean,
  beforeEffect?: (target: DemoBrokerTarget, sequence: number) => Promise<void>
) {
  if (registration.source !== "demo" || !registration.sessionId.trim()
      || !Number.isSafeInteger(registration.generation) || registration.generation < 1) {
    throw new DemoBrokerProviderError("INVALID_PROVIDER_REQUEST", "Invalid demo provider registration.");
  }
  const target = Object.freeze({ ...registration });
  const methods = new Map<string, { client: string; descriptor: { params: readonly string[]; defaults: Record<string, unknown> } }>(
    Object.entries(GATEWAY_CLIENT_METHODS).map(([client, descriptor]) => [descriptor.rpc, { client, descriptor }])
  );
  let disposed = false;
  let sequence = 0;
  let pending = 0;
  let queue: Promise<void> = Promise.resolve();
  const assertCurrent = () => {
    if (disposed || !isCurrent()) throw new DemoBrokerProviderError("TARGET_SUPERSEDED", "The broker target changed. Refresh its identity before starting another operation.");
  };
  return {
    target,
    dispose() { disposed = true; },
    async invoke(request: DemoBrokerRequest, admitEffect?: () => Promise<void>): Promise<unknown> {
      assertCurrent();
      if (request.target?.source !== target.source || request.target?.sessionId !== target.sessionId
          || request.target?.generation !== target.generation) {
        throw new DemoBrokerProviderError("TARGET_SUPERSEDED", "This request belongs to another broker target.");
      }
      if (!Number.isSafeInteger(request.sequence) || request.sequence <= sequence
          || !request.params || typeof request.params !== "object" || Array.isArray(request.params)) {
        throw new DemoBrokerProviderError("INVALID_PROVIDER_REQUEST", "Invalid or repeated demo provider request.");
      }
      const method = methods.get(request.method);
      const implementation = method && (transport as unknown as Record<string, unknown>)[method.client];
      if (!method || typeof implementation !== "function") {
        throw new DemoBrokerProviderError("METHOD_NOT_SUPPORTED", "The demo provider does not implement this gateway method.");
      }
      if (Object.keys(request.params).some(key => !(method.descriptor.params as readonly string[]).includes(key))) {
        throw new DemoBrokerProviderError("INVALID_PROVIDER_REQUEST", "The demo request contains unsupported arguments.");
      }
      if (pending >= 8) throw new DemoBrokerProviderError("PROVIDER_BUSY", "The demo provider request queue is full.");
      if (request.expectedState !== undefined && (!request.expectedState || typeof request.expectedState !== "object" || Array.isArray(request.expectedState))) {
        throw new DemoBrokerProviderError("INVALID_PROVIDER_REQUEST", "Expected state must be an object.");
      }
      const expected = structuredClone(request.expectedState ?? {});
      const stateFields: Readonly<Record<string, string>> = {
        presetName: "presetName", position: "presetPosition", presetPosition: "presetPosition",
        activeScene: "activeScene", scene: "activeScene", tempo: "tempo", presetIdentity: "presetIdentity"
      };
      if (!expected || typeof expected !== "object" || Array.isArray(expected)
          || Object.keys(expected).some(key => !Object.hasOwn(stateFields, key))) {
        throw new DemoBrokerProviderError("INVALID_PROVIDER_REQUEST", "The demo provider cannot verify these expected-state constraints.");
      }
      // Capture arguments now. Neither queueing nor a later source change can
      // replace the method/transport or alter the admitted request document.
      const params = structuredClone({ ...method.descriptor.defaults, ...request.params }) as Record<string, unknown>;
      const args = method.descriptor.params.map(name => params[name]);
      sequence = request.sequence;
      const admittedSequence = request.sequence;
      pending += 1;
      const run = queue.then(async () => {
        assertCurrent();
        try {
          const admit = async () => {
            assertCurrent();
            if (Object.keys(expected).length) {
            const snapshot = await transport.currentSnapshot() as unknown as Record<string, unknown>;
            assertCurrent();
            for (const [field, value] of Object.entries(expected)) {
              if (snapshot[stateFields[field]!] !== value) {
                throw new DemoBrokerProviderError("STATE_CONFLICT", "The demo state changed before this operation. Refresh it before editing.");
              }
            }
            }
            if (admitEffect) await admitEffect();
            else if (beforeEffect) await beforeEffect(target, admittedSequence);
            assertCurrent();
          };
          await admit();
          // The view shares the original store, runtime and mutation queue.
          // Admission is checked again INSIDE that queue before mutation,
          // including after an earlier UI storage write finished.
          const view = withVirtualGatewayAdmission(transport, admit);
          const effect = (view as unknown as Record<string, (...args: unknown[]) => unknown>)[method.client]!;
          const result = await effect.apply(view, args);
          assertCurrent();
          return result;
        } catch (error) {
          assertCurrent();
          throw error;
        }
      });
      queue = run.then(() => undefined, () => undefined);
      try { return await run; }
      finally { pending -= 1; }
    }
  };
}
