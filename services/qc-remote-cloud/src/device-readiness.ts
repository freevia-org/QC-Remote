/** The selected broker target is independent of whether physical USB is open. */
export interface BrokerTarget {
  source: "physical" | "demo";
  sessionId: string;
  generation: number;
}

export interface DeviceReadiness {
  ready: boolean;
  usbConnected: boolean;
  brokerTarget?: BrokerTarget;
}

/** Legacy hosts only publish USB readiness. New hosts publish both facts. */
export function deviceReadiness(frame: Record<string, unknown>): DeviceReadiness {
  if (frame.targetReady !== undefined && typeof frame.targetReady !== "boolean") {
    throw new Error("Invalid target readiness");
  }
  const brokerTarget = parseBrokerTarget(frame.brokerTarget);
  return {
    ready: typeof frame.targetReady === "boolean" ? frame.targetReady : frame.usbConnected === true,
    usbConnected: frame.usbConnected === true,
    ...(brokerTarget ? { brokerTarget } : {})
  };
}

/** Shared validation for host status and the client's explicitly accepted target. */
export function parseBrokerTarget(value: unknown): BrokerTarget | undefined {
  let brokerTarget: BrokerTarget | undefined;
  if (value !== undefined) {
    if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid broker target");
    const target = value as Record<string, unknown>;
    if ((target.source !== "physical" && target.source !== "demo")
        || typeof target.sessionId !== "string" || !target.sessionId.trim() || target.sessionId.length > 256
        || !Number.isSafeInteger(target.generation) || (target.generation as number) < 1) {
      throw new Error("Invalid broker target");
    }
    brokerTarget = { source: target.source, sessionId: target.sessionId, generation: target.generation as number };
  }
  return brokerTarget;
}

export function sameBrokerTarget(left: BrokerTarget | undefined, right: BrokerTarget | undefined): boolean {
  return left?.source === right?.source && left?.sessionId === right?.sessionId && left?.generation === right?.generation;
}

export function assertBrokerTarget(expected: BrokerTarget | undefined, current: BrokerTarget | undefined): void {
  if (!sameBrokerTarget(expected, current)) throw new Error("The selected broker target changed before dispatch; read its current state before starting another operation.");
}

/** A trusted native host alone issues this after verifying its private device
 * binding. Other writes may never use a lifecycle acknowledgement as a bypass. */
export function verifiedTargetTransition(method: string, expected: BrokerTarget, current: BrokerTarget | undefined, result: unknown): boolean {
  if (expected.source !== "physical" || !result || typeof result !== "object") return false;
  const ack = (result as Record<string, unknown>).brokerTargetTransition;
  if (!ack || typeof ack !== "object" || Array.isArray(ack)) return false;
  const value = ack as Record<string, unknown>;
  if (value.method !== method || !Object.hasOwn(value, "from") || !Object.hasOwn(value, "to") || value.to === undefined) return false;
  try {
    const from = parseBrokerTarget(value.from);
    const to = value.to === null ? undefined : parseBrokerTarget(value.to);
    if (!sameBrokerTarget(from, expected) || !sameBrokerTarget(to, current)) return false;
    if (method === "device.resetSession" || method === "device.reconnect") {
      return value.verification === "same_native_controller" && to?.source === "physical";
    }
    return method === "device.disconnect" && value.verification === "closed_native_controller"
      && (to === undefined || to.source === "demo");
  } catch { return false; }
}
