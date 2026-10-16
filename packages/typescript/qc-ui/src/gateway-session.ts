import type { GatewayTransport } from "@qc-remote/client";
import { SupersededPresetOperation } from "@qc-remote/core";

type GatewayRoute = { active(): GatewayTransport; revision(): number };
type CapturedRoute = { target: GatewayTransport; current(): boolean };
const routes = new WeakMap<GatewayTransport, () => CapturedRoute>();

export function registerGatewayRoute(gateway: GatewayTransport, route: GatewayRoute): void {
  routes.set(gateway, () => {
    const target = route.active();
    const revision = route.revision();
    return { target, current: () => route.revision() === revision && route.active() === target };
  });
}

/** Keep route capture semantics when a workflow adds lifetime checks around a gateway. */
export function registerGatewayWrapper(outer: GatewayTransport, inner: GatewayTransport, wrap: (target: GatewayTransport) => GatewayTransport, valid: () => boolean): void {
  routes.set(outer, () => {
    const inherited = routes.get(inner)?.() ?? { target: inner, current: () => true };
    return { target: wrap(inherited.target), current: () => valid() && inherited.current() };
  });
}

/** Freeze one transport for the entire workflow and reject results after its session changes. */
export function captureGatewaySession(gateway: GatewayTransport, valid: () => boolean = () => true) {
  const route = routes.get(gateway)?.();
  const target = route?.target ?? gateway;
  const current = () => valid() && (!route || route.current());
  const assertCurrent = () => { if (!current()) throw new SupersededPresetOperation(); };
  const methods = new Map<PropertyKey, (...args: unknown[]) => Promise<unknown>>();
  const frozen = new Proxy({} as GatewayTransport, {
    get(_unused, method) {
      if (typeof method === "symbol" || method === "then" || method === "toJSON") return undefined;
      let call = methods.get(method);
      if (!call) {
        call = async (...args) => {
          assertCurrent();
          const implementation = (target as unknown as Record<PropertyKey, unknown>)[method];
          if (typeof implementation !== "function") throw new TypeError(`The captured gateway has no ${String(method)} method.`);
          let result;
          try { result = await implementation.apply(target, args); }
          catch (error) { assertCurrent(); throw error; }
          assertCurrent();
          return result;
        };
        methods.set(method, call);
      }
      return call;
    }
  });
  return { gateway: frozen, current, assertCurrent };
}
