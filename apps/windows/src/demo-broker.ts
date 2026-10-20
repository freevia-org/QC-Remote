import type { GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import { createDemoBrokerProvider, type DemoBrokerRequest } from "@qc-remote/ui";
import { callTauri } from "./tauri-invoke.ts";
import { createDemoBrokerLifecycle } from "./demo-broker-lifecycle.ts";

const BROKER_INSTANCE_EVENT = "qc-demo-broker-instance";
let observedBrokerInstance: string | undefined;

/** Reuse the editor's existing local inventory observation; add no timer. */
export function observeDemoBrokerInstance(instance: string | null | undefined): void {
  const next = instance || undefined;
  if (next === observedBrokerInstance) return;
  observedBrokerInstance = next;
  window.dispatchEvent(new CustomEvent(BROKER_INSTANCE_EVENT, { detail: next }));
}

/** Expose this exact already-opened UI demo, without using its routing proxy. */
export async function exposeDesktopDemo(
  virtual: GatewayTransport,
  status: (ready: boolean, message?: string) => void,
  changed: (snapshot: PresetSnapshot) => void
): Promise<() => void> {
  if (!window.__TAURI_INTERNALS__) return () => undefined;
  const { listen } = await import("@tauri-apps/api/event");
  let live = true;
  let selected = "";
  let dispatcher: ReturnType<typeof createDemoBrokerProvider> | undefined;
  const lifecycle = createDemoBrokerLifecycle({
    newId: () => crypto.randomUUID(),
    start: sessionId => callTauri<void>("start_demo_provider", { sessionId }),
    stop: providerId => callTauri<void>("stop_demo_provider", { providerId }),
    invalidate: () => { selected = ""; dispatcher?.dispose(); },
    failed: error => { if (live) status(false, error instanceof Error ? error.message : String(error)); }
  });
  const stopStatus = await listen<{ providerId: string; ready: boolean; message?: string }>("qc-demo-provider-status", ({ payload }) => {
    if (!live || !lifecycle.isCurrent(payload.providerId)) return;
    if (!payload.ready) { selected = ""; dispatcher?.dispose(); }
    status(payload.ready, payload.message);
  });
  const stopRequests = await listen<{ providerId: string; request: DemoBrokerRequest }>("qc-demo-provider-request", ({ payload }) => {
    if (!live || !lifecycle.isCurrent(payload.providerId)) return;
    const providerId = payload.providerId;
    void (async () => {
      const request = payload.request;
      const key = JSON.stringify(request.target);
      // A demo -> physical -> same demo return carries a fresh generation.
      // Recreate only the dispatcher lease, retaining the exact same store.
      if (key !== selected) {
        selected = key;
        dispatcher?.dispose();
        dispatcher = createDemoBrokerProvider(virtual, request.target, () => live && lifecycle.isCurrent(providerId) && selected === key,
          (target, sequence) => callTauri("admit_demo_provider_request", { providerId, target, sequence }).then(() => undefined));
      }
      let response: Record<string, unknown>;
      try {
        const captured = dispatcher;
        if (!captured) throw new Error("The demo provider has no current target lease.");
        const result = await captured.invoke(request);
        const snapshot = await virtual.currentSnapshot();
        if (live && lifecycle.isCurrent(providerId) && selected === key) changed(snapshot);
        response = { result };
      } catch (error) {
        const failure = error as { code?: string; message?: string };
        response = { error: { code: -32002, message: failure.message ?? "The demo provider request failed.", data: { code: failure.code ?? "DEMO_PROVIDER_ERROR", retryable: false } } };
      }
      if (!live || !lifecycle.isCurrent(providerId)) return;
      await callTauri("complete_demo_provider_request", { providerId, target: request.target, sequence: request.sequence, response });
    })().catch(error => { if (live && lifecycle.isCurrent(providerId)) status(false, error instanceof Error ? error.message : String(error)); });
  });
  const cleanup = () => {
    if (!live) return;
    live = false;
    selected = "";
    dispatcher?.dispose();
    stopRequests();
    stopStatus();
    window.removeEventListener(BROKER_INSTANCE_EVENT, brokerChanged);
    lifecycle.dispose();
  };
  const brokerChanged = (event: Event) => {
    const instance = (event as CustomEvent<string | undefined>).detail;
    lifecycle.rebind(Boolean(instance));
  };
  window.addEventListener(BROKER_INSTANCE_EVENT, brokerChanged);
  try { await lifecycle.start(); }
  catch (error) { cleanup(); throw error; }
  return cleanup;
}
