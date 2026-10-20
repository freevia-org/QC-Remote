import type { ConnectionState, PresetSnapshot, RuntimeStatus } from "@qc-remote/client";

export type AndroidBrokerTarget = { source: "physical" | "demo"; sessionId: string; generation: number };
export type AndroidGatewayRuntimeStatus = RuntimeStatus & { connected: boolean; synchronized: boolean; brokerTarget?: AndroidBrokerTarget };
export type AndroidGatewayTargetObservation = {
  connection: ConnectionState & { brokerTarget?: AndroidBrokerTarget };
  snapshot?: PresetSnapshot;
  accept?(): void;
};

interface TargetReaderPort {
  status(): Promise<AndroidGatewayRuntimeStatus>;
  snapshot(target: AndroidBrokerTarget): Promise<PresetSnapshot>;
  accept(target: AndroidBrokerTarget): void;
}

const same = (left: AndroidBrokerTarget, right: AndroidBrokerTarget | undefined) =>
  right?.source === left.source && right.sessionId === left.sessionId && right.generation === left.generation;

/** Discovery is separate from adoption. Never change the UI's target while reading status. */
export async function readAndroidPhysicalTarget(port: TargetReaderPort): Promise<AndroidGatewayTargetObservation> {
  const before = await port.status();
  const offered = before.brokerTarget;
  if (!before.connected || !before.synchronized || offered?.source !== "physical") {
    return { connection: { phase: "disconnected", demo: false, source: "device", detail: "No synchronized physical Quad Cortex is available." } };
  }
  const captured = Object.freeze({ ...offered });
  const snapshot = await port.snapshot(captured);
  const after = await port.status();
  if (!after.connected || !after.synchronized || !same(captured, after.brokerTarget)) {
    throw Object.assign(new Error("The physical Quad Cortex changed while its state was being read."), { code: "TARGET_SUPERSEDED" });
  }
  return {
    connection: { phase: "ready", demo: false, source: "device", detail: "Quad Cortex connected.", brokerTarget: captured },
    snapshot,
    accept: () => port.accept(captured)
  };
}
