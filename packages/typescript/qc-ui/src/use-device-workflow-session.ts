import { useEffect, useMemo, useRef, useState } from "react";
import { SupersededPresetOperation } from "@qc-remote/core";
import type { DeviceCommandOptions } from "./workflow-options";
import { bindDeviceCommandScope, deviceCommandBusy, devicePhysicalCommandKey, registerDeviceCommandSession } from "./device-command-ownership.ts";
type DeviceWorkflowSessionOptions = Pick<DeviceCommandOptions, "lifetime" | "sessionKey" | "commandSessionKey" | "captureSessionCurrent" | "notice" | "fail"> & {
  gateway: object; connected: boolean; setPending?(pending: boolean): void;
};

/** Own device-wide reads and multi-step writes across transport replacements. */
export function useDeviceWorkflowSession<T extends DeviceWorkflowSessionOptions>(options: T) {
  const [, replaceMountedOwner] = useState(0);
  const lifetime = options.lifetime;
  const physicalSession = lifetime?.checkpoint().session;
  const origin = useMemo(() => lifetime?.checkpoint(), [lifetime, physicalSession]);
  const lifetimeKey = useMemo(() => ({ lifetime, session: physicalSession }), [lifetime, physicalSession]);
  const sessionKey = options.sessionKey ?? (lifetime ? lifetimeKey : undefined);
  const latest = useRef(options);
  latest.current = options;
  type Owner = { key: unknown; gateway: T["gateway"]; connected: boolean; current(): boolean };
  const ownerRef = useRef<Owner | null>(null);
  const epoch = useRef(0);
  const pendingOwner = useRef<Owner | null>(null);
  const pendingReset = useRef(false);
  let owner = ownerRef.current;
  const externalProbe = options.captureSessionCurrent?.() ?? (() => true);
  const probe = () => externalProbe() && (!origin || Boolean(lifetime?.currentSession(origin)));
  if (!owner || (!owner.current() && probe()) || owner.key !== sessionKey
      || owner.gateway !== options.gateway || owner.connected !== options.connected) {
    const replacement: Owner = { key: sessionKey, gateway: options.gateway, connected: options.connected, current: () => false };
    replacement.current = () => ownerRef.current === replacement && probe();
    ownerRef.current = replacement;
    owner = replacement;
  }
  const held = owner;
  const scoped = useMemo(() => {
    const current = held.current;
    const apply = (callback: () => void) => { if (current()) callback(); };
    const gateway = new Proxy(held.gateway, {
      get(target, key) {
        const method = Reflect.get(target, key);
        if (typeof method !== "function") return method;
        return async (...args: unknown[]) => {
          const started = epoch.current;
          if (!current()) throw new SupersededPresetOperation();
          try {
            const result = await method.apply(target, args);
            if (!current() || started !== epoch.current) throw new SupersededPresetOperation();
            return result;
          } catch (error) {
            if (!current() || started !== epoch.current) throw new SupersededPresetOperation();
            throw error;
          }
        };
      }
    });
    return { gateway, current, apply,
      notice: (message: string) => apply(() => latest.current.notice(message)),
      fail: (error: unknown) => { if (!(error instanceof SupersededPresetOperation)) apply(() => latest.current.fail(error)); },
      setPending: (value: boolean) => {
        if (!current()) return;
        if (value) pendingOwner.current = held;
        else if (pendingOwner.current === held) pendingOwner.current = null;
        const setter = latest.current.setPending;
        if (value || (setter && !deviceCommandBusy(scoped.setPending))) setter?.(value);
      }
    };
  }, [held]);
  const commandSessionKey = options.commandSessionKey ?? (lifetime ? devicePhysicalCommandKey(lifetime, physicalSession) : sessionKey ?? options.gateway);
  if (options.setPending) {
    bindDeviceCommandScope(scoped.setPending, options.setPending);
    pendingReset.current = registerDeviceCommandSession(scoped.setPending, commandSessionKey, true) || pendingReset.current;
  }
  useEffect(() => {
    if (pendingReset.current || (pendingOwner.current && pendingOwner.current !== held)) {
      pendingReset.current = false;
      pendingOwner.current = null;
      const setter = latest.current.setPending;
      if (setter && !deviceCommandBusy(scoped.setPending)) setter(false);
    }
  }, [commandSessionKey, held, scoped.setPending]);
  useEffect(() => {
    if (!lifetime) return;
    let checkpoint = lifetime.checkpoint();
    return lifetime.subscribe(() => {
      if (lifetime.currentSession(checkpoint)) return;
      checkpoint = lifetime.checkpoint();
      epoch.current++;
      replaceMountedOwner((version) => version + 1);
    });
  }, [lifetime]);
  useEffect(() => {
    // StrictMode replays mount effects. Replace the revoked owner explicitly;
    // restoring it would revive reads started before that cleanup.
    if (!ownerRef.current) replaceMountedOwner((version) => version + 1);
    return () => {
      epoch.current++;
      ownerRef.current = null;
      if (pendingOwner.current) {
        pendingOwner.current = null;
        const setter = latest.current.setPending;
        if (setter && !deviceCommandBusy(scoped.setPending)) setter(false);
      }
    };
  }, []);
  return { key: held, current: scoped.current, apply: scoped.apply,
    options: { ...options, gateway: scoped.gateway, setPending: scoped.setPending, notice: scoped.notice, fail: scoped.fail,
      sessionKey: held, commandSessionKey, captureSessionCurrent: () => held.current } as T };
}
