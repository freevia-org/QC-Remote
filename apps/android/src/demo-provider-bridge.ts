import { brokerTargetKey, type BrokerTarget, type GatewayTransport } from "@qc-remote/client";
import { createDemoBrokerProvider, DemoBrokerProviderError } from "@qc-remote/ui";

export interface NativeDemoRequest {
  id: string;
  sessionId: string;
  generation: number;
  sequence: number;
  method: string;
  params: Record<string, unknown>;
  /** Shared provider admission validates this guard; it is not an extra RPC argument. */
  expectedState?: Record<string, unknown>;
  brokerTarget?: BrokerTarget;
}
export interface NativeDemoPort {
  admitDemoRequest(request: { id: string; sessionId: string; generation: number; brokerTarget?: BrokerTarget }): Promise<{ admitted: boolean }>;
  publishDemoProvider(registration: { sessionId: string | null; generation: number }): Promise<void>;
  completeDemoRequest(reply: { id: string; sessionId: string; generation: number; result?: unknown; error?: { code: string; message: string } }): Promise<void>;
}

/** Android hosting glue only. Dispatch is the shared broker provider, bound to
 * the editor's immutable virtual transport, never the switchable UI gateway.
 * Native owns authentication, deadlines, physical-first admission and teardown.
 * No timer, retry, authentication or cloud connection is started here. */
export function createNativeDemoProviderBridge(port: NativeDemoPort, onResult?: (gateway: GatewayTransport, current: () => boolean) => Promise<void>) {
  let generation = 0;
  let active: { sessionId: string; generation: number; gateway: GatewayTransport; current(): boolean; provider: ReturnType<typeof createDemoBrokerProvider> } | undefined;
  const withdraw = () => { active?.provider.dispose(); active = undefined; };
  return {
    async bind(session: { sessionId: string; gateway: GatewayTransport; current(): boolean } | undefined): Promise<void> {
      withdraw();
      const nextGeneration = ++generation;
      if (!session || !session.current()) { await port.publishDemoProvider({ sessionId: null, generation: nextGeneration }); return; }
      const current = () => active === registration && session.current();
      const provider = createDemoBrokerProvider(session.gateway, { source: "demo", sessionId: session.sessionId, generation: nextGeneration }, current);
      const registration = { ...session, generation: nextGeneration, provider, current };
      active = registration;
      try { await port.publishDemoProvider({ sessionId: session.sessionId, generation: nextGeneration }); }
      catch (error) { if (active === registration) withdraw(); throw error; }
    },
    async handle(request: NativeDemoRequest): Promise<"completed" | "discarded"> {
      if (!request || typeof request.id !== "string" || !request.id
          || typeof request.sessionId !== "string" || !request.sessionId
          || !Number.isSafeInteger(request.generation) || request.generation < 1) return "discarded";
      try { request = structuredClone(request); } catch { return "discarded"; }
      const registration = active;
      let result: unknown;
      let failure: { code: string; message: string } | undefined;
      try {
        if (!registration || !registration.current()) throw new DemoBrokerProviderError("TARGET_SUPERSEDED", "The demo provider is no longer current.");
        result = await registration.provider.invoke({
          target: { source: "demo", sessionId: request.sessionId, generation: request.generation },
          sequence: request.sequence, method: request.method, params: request.params,
          ...("expectedState" in request ? { expectedState: request.expectedState } : {})
        }, async () => {
          // The shared provider invokes this again INSIDE its actual store
          // queue. Native state can change before a JS availability event.
          if (!request.brokerTarget) throw new DemoBrokerProviderError("INVALID_PROVIDER_REQUEST", "The native demo request has no broker target identity.");
          brokerTargetKey(request.brokerTarget);
          const answer = await port.admitDemoRequest({ id: request.id, sessionId: request.sessionId, generation: request.generation, brokerTarget: request.brokerTarget });
          if (answer?.admitted !== true) throw new DemoBrokerProviderError("TARGET_SUPERSEDED", "The native demo request is no longer admitted for this target.");
        });
      } catch (error) {
        failure = error instanceof DemoBrokerProviderError
          ? { code: error.code, message: error.message }
          : { code: "DEMO_PROVIDER_FAILED", message: "The demo operation failed. It was not retried on another target." };
      }
      if (!failure && (!registration || !registration.current())) failure = { code: "TARGET_SUPERSEDED", message: "The demo target changed before this operation completed." };
      try {
        await port.completeDemoRequest({ id: request.id, sessionId: request.sessionId, generation: request.generation, ...(failure ? { error: failure } : { result }) });
      } catch { return "discarded"; } // Expired/native-superseded replies never cause a retry.
      if (!failure && registration?.current() && onResult) {
        try { await onResult(registration.gateway, registration.current); }
        catch { /* The RPC already completed; a view refresh cannot send a second reply. */ }
      }
      return failure ? "discarded" : "completed";
    },
    async dispose(): Promise<void> {
      withdraw();
      await port.publishDemoProvider({ sessionId: null, generation: ++generation });
    }
  };
}

export interface NativeDemoListenerPort extends NativeDemoPort {
  addListener(event: "qcDemoRequest", listener: (request: NativeDemoRequest) => void): Promise<{ remove(): Promise<void> }>;
}

/** Own the native listener and foreground registration as one lifetime. The
 * listener is installed before readiness is published; foreground changes
 * renew registration rather than reviving a suspended request queue. */
export function attachNativeDemoProvider(options: {
  port: NativeDemoListenerPort;
  gateway: GatewayTransport;
  current(): boolean;
  sessionId(): string;
  visible(): boolean;
  subscribeVisibility(changed: () => void): () => void;
  onResult?(gateway: GatewayTransport, current: () => boolean): Promise<void>;
  onError?(error: unknown): void;
}) {
  let alive = true, initialized = false, visible = options.visible(), revision = 0;
  const bridge = createNativeDemoProviderBridge(options.port, options.onResult);
  const renew = async () => {
    const owner = ++revision;
    await bridge.bind(alive && visible && options.current() ? {
      gateway: options.gateway, sessionId: options.sessionId(),
      current: () => alive && visible && owner === revision && options.current()
    } : undefined);
  };
  const unsubscribe = options.subscribeVisibility(() => {
    visible = options.visible();
    revision++; // Invalidate old effects synchronously, before the native call.
    if (initialized) void renew().catch(error => { if (alive) options.onError?.(error); });
  });
  let listenerRemoved = false;
  const removeListener = async (handle: { remove(): Promise<void> }) => {
    if (listenerRemoved) return;
    listenerRemoved = true;
    await handle.remove();
  };
  const listener = Promise.resolve().then(() => options.port.addListener("qcDemoRequest", request => {
    if (alive) void bridge.handle(request).catch(error => { if (alive) options.onError?.(error); });
  }));
  const ready = listener.then(async handle => {
    if (!alive) { await removeListener(handle); return; }
    initialized = true;
    await renew();
  });
  return {
    ready,
    async dispose() {
      if (!alive) return;
      alive = false; visible = false; revision++;
      unsubscribe();
      await Promise.allSettled([bridge.dispose(), listener.then(removeListener)]);
    }
  };
}
