import type { GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import { createDemoBrokerProvider, type DemoBrokerRequest } from "@qc-remote/ui";
import { callTauri } from "./tauri-invoke.ts";

/** Expose this exact already-opened UI demo, without using its routing proxy. */
export async function exposeDesktopDemo(
  virtual: GatewayTransport,
  status: (ready: boolean, message?: string) => void,
  changed: (snapshot: PresetSnapshot) => void
): Promise<() => void> {
  if (!window.__TAURI_INTERNALS__) return () => undefined;
  const { listen } = await import("@tauri-apps/api/event");
  const providerId = crypto.randomUUID();
  let live = true;
  let selected = "";
  let dispatcher: ReturnType<typeof createDemoBrokerProvider> | undefined;
  const stopStatus = await listen<{ providerId: string; ready: boolean; message?: string }>("qc-demo-provider-status", ({ payload }) => {
    if (!live || payload.providerId !== providerId) return;
    if (!payload.ready) { selected = ""; dispatcher?.dispose(); }
    status(payload.ready, payload.message);
  });
  const stopRequests = await listen<{ providerId: string; request: DemoBrokerRequest }>("qc-demo-provider-request", ({ payload }) => {
    if (!live || payload.providerId !== providerId) return;
    void (async () => {
      const request = payload.request;
      const key = JSON.stringify(request.target);
      // A demo -> physical -> same demo return carries a fresh generation.
      // Recreate only the dispatcher lease, retaining the exact same store.
      if (key !== selected) {
        selected = key;
        dispatcher?.dispose();
        dispatcher = createDemoBrokerProvider(virtual, request.target, () => live && selected === key);
      }
      let response: Record<string, unknown>;
      try {
        const captured = dispatcher;
        if (!captured) throw new Error("The demo provider has no current target lease.");
        await callTauri("admit_demo_provider_request", { providerId, target: request.target, sequence: request.sequence });
        const result = await captured.invoke(request);
        const snapshot = await virtual.currentSnapshot();
        if (live && selected === key) changed(snapshot);
        response = { result };
      } catch (error) {
        const failure = error as { code?: string; message?: string };
        response = { error: { code: -32002, message: failure.message ?? "The demo provider request failed.", data: { code: failure.code ?? "DEMO_PROVIDER_ERROR", retryable: false } } };
      }
      if (!live) return;
      await callTauri("complete_demo_provider_request", { providerId, target: request.target, sequence: request.sequence, response });
    })().catch(error => { if (live) status(false, error instanceof Error ? error.message : String(error)); });
  });
  const cleanup = () => {
    if (!live) return;
    live = false;
    selected = "";
    dispatcher?.dispose();
    stopRequests();
    stopStatus();
    void callTauri("stop_demo_provider", { providerId }).catch(() => undefined);
  };
  try { await callTauri("start_demo_provider", { sessionId: providerId }); }
  catch (error) { cleanup(); throw error; }
  return cleanup;
}
