import type { AndroidBrokerTarget } from "./android-gateway-target.ts";

export type AndroidGatewayEnvelope = {
  method: string;
  params?: Record<string, unknown>;
  expectedBrokerTarget?: AndroidBrokerTarget;
  requireAcceptedBrokerTarget: true;
};

/** The native boundary classifies reads/writes; every managed UI call requires its accepted target for writes. */
export function createAndroidGatewayContext(nativeInvoke: <T>(request: AndroidGatewayEnvelope) => Promise<T>) {
  let accepted: AndroidBrokerTarget | undefined;
  return {
    accept(target: AndroidBrokerTarget | undefined): void {
      if (target && target.source !== "physical") throw new TypeError("The native USB gateway requires a physical broker target.");
      accepted = target ? Object.freeze({ ...target }) : undefined;
    },
    invoke<T>(method: string, params?: Record<string, unknown>): Promise<T> {
      return nativeInvoke<T>({ method, params, requireAcceptedBrokerTarget: true,
        ...(accepted ? { expectedBrokerTarget: accepted } : {}) });
    }
  };
}
