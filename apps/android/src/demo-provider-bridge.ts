import type { GatewayTransport } from "@qc-remote/client";
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
}
export interface NativeDemoPort {
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
      const registration = active;
      let result: unknown;
      let failure: { code: string; message: string } | undefined;
      try {
        if (!registration || !registration.current()) throw new DemoBrokerProviderError("TARGET_SUPERSEDED", "The demo provider is no longer current.");
        result = await registration.provider.invoke({
          target: { source: "demo", sessionId: request.sessionId, generation: request.generation },
          sequence: request.sequence, method: request.method, params: request.params,
          ...(request.expectedState ? { expectedState: request.expectedState } : {})
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
