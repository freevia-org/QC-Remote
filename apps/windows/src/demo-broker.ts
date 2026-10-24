import type { GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import { createDemoBrokerProvider, type DemoBrokerRequest } from "@qc-remote/ui";
import { callTauri } from "./tauri-invoke.ts";
import { createDemoBrokerInstanceObservation, createDemoBrokerRecovery, createDemoBrokerLifecycle, rebindDemoBrokerFromObservation, waitForDemoBrokerRegistration } from "./demo-broker-lifecycle.ts";

const BROKER_INSTANCE_EVENT = "qc-demo-broker-instance";
const BROKER_INVENTORY_EVENT = "qc-demo-broker-inventory";

/** Reuse the editor's existing local inventory observation; add no timer. */
const observeInstance = createDemoBrokerInstanceObservation(instance => {
  window.dispatchEvent(new CustomEvent(BROKER_INSTANCE_EVENT, { detail: instance }));
});
export function observeDemoBrokerInstance(instance: string | null | undefined) {
  observeInstance(instance);
  if (instance !== undefined) window.dispatchEvent(new CustomEvent(BROKER_INVENTORY_EVENT, { detail: instance }));
}

/** Expose this exact already-opened UI demo, without using its routing proxy. */
export async function exposeDesktopDemo(
  virtual: GatewayTransport,
  status: (ready: boolean, message?: string, brokerInstanceId?: string) => void,
  changed: (snapshot: PresetSnapshot) => void,
  own: (cleanup: () => Promise<void>) => void
): Promise<() => Promise<void>> {
  if (!window.__TAURI_INTERNALS__) return async () => undefined;
  const { listen } = await import("@tauri-apps/api/event");
  let live = true;
  let selected = "";
  let dispatcher: ReturnType<typeof createDemoBrokerProvider> | undefined;
  let registration: { id: string; observe(ready: boolean, message?: string, registrationAttempted?: boolean): void } | undefined;
  let userAction = true;
  const recovery = createDemoBrokerRecovery(() => lifecycle.rebind(true));
  const lifecycle = createDemoBrokerLifecycle({
    newId: () => crypto.randomUUID(),
    start: sessionId => waitForDemoBrokerRegistration({
      timeoutMs: 30_000, // Includes the explicit election/start budget, not device work.
      start: () => {
        const explicit = userAction;
        userAction = false;
        return callTauri<void>("start_demo_provider", { sessionId, userAction: explicit });
      },
      subscribe: observe => {
        const current = { id: sessionId, observe };
        registration = current;
        return () => { if (registration === current) registration = undefined; };
      }
    }),
    stop: providerId => callTauri<void>("stop_demo_provider", { providerId }),
    invalidate: () => { selected = ""; dispatcher?.dispose(); },
    failed: error => { if (live) { recovery.failed(); status(false, error instanceof Error ? error.message : String(error)); } }
  });
  const stopStatus = await listen<{ providerId: string; ready: boolean; message?: string; brokerInstanceId?: string; registrationAttempted?: boolean }>("qc-demo-provider-status", ({ payload }) => {
    if (!live || !lifecycle.isCurrent(payload.providerId)) return;
    if (registration?.id === payload.providerId) registration.observe(payload.ready, payload.message, payload.registrationAttempted);
    if (!payload.ready) { selected = ""; dispatcher?.dispose(); recovery.failed(); }
    else recovery.ready();
    status(payload.ready, payload.message, payload.ready ? payload.brokerInstanceId : undefined);
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
    if (!live) return lifecycle.dispose();
    registration?.observe(false, "The Demo selection ended");
    live = false;
    selected = "";
    dispatcher?.dispose();
    stopRequests();
    stopStatus();
    window.removeEventListener(BROKER_INSTANCE_EVENT, brokerChanged);
    window.removeEventListener(BROKER_INVENTORY_EVENT, inventoryObserved);
    return lifecycle.dispose();
  };
  const brokerChanged = (event: Event) => {
    const instance = (event as CustomEvent<string | undefined>).detail;
    if (!instance) status(false, "The broker is disconnected. Select Connect or Demo explicitly to resume.");
    // Startup already authenticates the current descriptor. Do not replace its
    // id while its registration acknowledgement is still outstanding.
    rebindDemoBrokerFromObservation(recovery, instance, Boolean(registration), available => lifecycle.rebind(available));
  };
  const inventoryObserved = (event: Event) => {
    if (live) recovery.observed((event as CustomEvent<string | null>).detail, Boolean(registration));
  };
  window.addEventListener(BROKER_INSTANCE_EVENT, brokerChanged);
  window.addEventListener(BROKER_INVENTORY_EVENT, inventoryObserved);
  // The caller owns retirement even while registration is still pending.
  // A newer Connect must not leave a late Demo registration behind it.
  own(cleanup);
  try { await lifecycle.start(); }
  catch (error) { await cleanup(); throw error; }
  return cleanup;
}
