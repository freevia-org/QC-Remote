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
  const value = frame.brokerTarget;
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
  return {
    ready: typeof frame.targetReady === "boolean" ? frame.targetReady : frame.usbConnected === true,
    usbConnected: frame.usbConnected === true,
    ...(brokerTarget ? { brokerTarget } : {})
  };
}

export function sameBrokerTarget(left: BrokerTarget | undefined, right: BrokerTarget | undefined): boolean {
  return left?.source === right?.source && left?.sessionId === right?.sessionId && left?.generation === right?.generation;
}

export function assertBrokerTarget(expected: BrokerTarget | undefined, current: BrokerTarget | undefined): void {
  if (!sameBrokerTarget(expected, current)) throw new Error("The selected broker target changed before dispatch; read its current state before starting another operation.");
}
