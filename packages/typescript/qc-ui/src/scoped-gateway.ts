import { useMemo } from "react";
import type { GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import { SupersededPresetOperation, type PresetLifetime, type PresetOperation } from "@qc-remote/core";
import { captureGatewaySession, registerGatewayWrapper } from "./gateway-session.ts";

const sessionMethods = new Set(["listModels", "listPresets", "listPresetFolders", "listPresetSlots", "setMasterVolume"]);

/** Bind deferred UI callbacks and their results to the context that created them. */
export function scopedGateway(gateway: GatewayTransport, lifetime: PresetLifetime, origin: PresetOperation, snapshot: Pick<PresetSnapshot, "setlistKey">): GatewayTransport {
  const setlistKey = snapshot.setlistKey;
  const captured = captureGatewaySession(gateway, () => lifetime.currentSession(origin));
  const scoped = new Proxy(captured.gateway, {
    get(target, property, receiver) {
      const method = Reflect.get(target, property, receiver);
      if (typeof method !== "function") return method;
      return async (...args: unknown[]) => {
        captured.assertCurrent();
        const sessionOnly = sessionMethods.has(String(property));
        const valid = (operation: PresetOperation) => sessionOnly ? lifetime.currentSession(operation) : lifetime.current(operation);
        if (!valid(origin)) throw new SupersededPresetOperation();
        const destination = property === "recallPreset"
          ? {setlistKey: args[0] as string, presetPosition: args[1] as number}
          : property === "navigateBank"
            ? {setlistKey, presetPosition: (args[2] as number) + (args[0] as number) * 8}
            : undefined;
        const operation = lifetime.begin(destination);
        try {
          const result = await Reflect.apply(method, target, args);
          captured.assertCurrent();
          if (!valid(operation)) throw new SupersededPresetOperation();
          return result;
        } catch (error) {
          captured.assertCurrent();
          if (!valid(operation)) throw new SupersededPresetOperation();
          throw error;
        } finally {
          lifetime.end(operation);
        }
      };
    }
  });
  registerGatewayWrapper(scoped, gateway, (target) => scopedGateway(target, lifetime, origin, snapshot), captured.current);
  return scoped;
}

/**
 * The lifetime's current checkpoint, as one object for as long as the session
 * and revision it names are unchanged. `checkpoint()` returns a new object on
 * every call, so a hook that captured it directly re-created every callback,
 * effect and timer that depended on it on every render.
 */
export function useLifetimeCheckpoint(lifetime: PresetLifetime): PresetOperation {
  const { session, revision } = lifetime.checkpoint();
  return useMemo(() => ({ session, revision }), [lifetime, session, revision]);
}

/**
 * `scopedGateway` for the current render's lifetime checkpoint, re-created
 * only when the gateway, the checkpoint or the active setlist changes. A
 * per-render proxy made every `useCallback([gateway])` in the workflow hooks
 * new on every render: the open Directory re-read its setlist and restarted
 * its timer on each one, and every native frame renders.
 */
export function useScopedGateway(gateway: GatewayTransport, lifetime: PresetLifetime, setlistKey: string): { gateway: GatewayTransport; origin: PresetOperation } {
  const origin = useLifetimeCheckpoint(lifetime);
  const scoped = useMemo(() => scopedGateway(gateway, lifetime, origin, { setlistKey }), [gateway, lifetime, origin, setlistKey]);
  return { gateway: scoped, origin };
}
