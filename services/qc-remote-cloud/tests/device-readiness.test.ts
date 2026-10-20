import assert from "node:assert/strict";
import test from "node:test";
import { assertBrokerTarget, deviceReadiness, parseBrokerTarget, sameBrokerTarget } from "../src/device-readiness.ts";
import { selectDeviceHost } from "../src/device-selection.ts";

const demo = { source: "demo" as const, sessionId: "broker-1:demo-1", generation: 1 };
const physical = { source: "physical" as const, sessionId: "broker-1:qc-1", generation: 2 };
const host = (deviceId: string, frame: Record<string, unknown>) => ({
  readyState: 1,
  deserializeAttachment: () => ({ deviceId, ...deviceReadiness(frame) })
});

test("legacy readiness stays compatible and new demo readiness preserves USB truth", () => {
  assert.deepEqual(deviceReadiness({ usbConnected: true }), { ready: true, usbConnected: true });
  assert.deepEqual(deviceReadiness({ usbConnected: false }), { ready: false, usbConnected: false });
  assert.deepEqual(deviceReadiness({ usbConnected: false, targetReady: true, brokerTarget: demo }),
    { ready: true, usbConnected: false, brokerTarget: demo });
  assert.equal(deviceReadiness({ usbConnected: true, targetReady: false }).ready, false);
});

test("target switch metadata survives even when both targets stay ready", () => {
  const first = deviceReadiness({ usbConnected: false, targetReady: true, brokerTarget: demo });
  const next = deviceReadiness({ usbConnected: true, targetReady: true, brokerTarget: physical });
  const resumed = deviceReadiness({ usbConnected: false, targetReady: true, brokerTarget: { ...demo, generation: 3 } });
  assert.equal(sameBrokerTarget(first.brokerTarget, next.brokerTarget), false);
  assert.equal(sameBrokerTarget(first.brokerTarget, resumed.brokerTarget), false);
  assert.equal(sameBrokerTarget(resumed.brokerTarget, { ...demo, generation: 3 }), true);
  assert.equal(sameBrokerTarget(first.brokerTarget, undefined), false);
  assert.equal(sameBrokerTarget(undefined, undefined), true);
});

test("queued commands retain their admitted target across switches and ABA returns", () => {
  assert.deepEqual(parseBrokerTarget({ ...demo }), demo);
  assert.equal(parseBrokerTarget(undefined), undefined);
  assert.throws(() => parseBrokerTarget(null));
  assert.doesNotThrow(() => assertBrokerTarget(undefined, undefined));
  assert.doesNotThrow(() => assertBrokerTarget(demo, { ...demo }));
  assert.throws(() => assertBrokerTarget(demo, physical), /target changed/);
  assert.throws(() => assertBrokerTarget(demo, { ...demo, generation: 3 }), /target changed/);
  assert.throws(() => assertBrokerTarget(demo, undefined), /target changed/);
  assert.throws(() => assertBrokerTarget(undefined, demo), /target changed/);
});

test("malformed target metadata cannot become a ready broker target", () => {
  for (const brokerTarget of [null, [], {}, { ...demo, source: "unknown" }, { ...demo, sessionId: " " },
    { ...demo, generation: 0 }, { ...demo, generation: 1.1 }, { ...demo, generation: Number.MAX_SAFE_INTEGER + 1 }]) {
    assert.throws(() => deviceReadiness({ targetReady: true, brokerTarget }));
  }
  assert.throws(() => deviceReadiness({ usbConnected: true, targetReady: "yes" }));
});

test("physical hosts always outrank demo hosts, regardless of socket order", () => {
  const virtual = host("demo", { usbConnected: false, targetReady: true, brokerTarget: demo });
  const qc = host("qc", { usbConnected: true, targetReady: true, brokerTarget: physical });
  const legacy = host("old-qc", { usbConnected: true });
  for (const method of ["device.snapshot", "device.setTempo", "system.status"]) {
    assert.equal(selectDeviceHost([virtual, qc], method), qc);
    assert.equal(selectDeviceHost([qc, virtual], method), qc);
    assert.equal(selectDeviceHost([virtual, legacy], method), legacy);
    assert.equal(selectDeviceHost([virtual], method), virtual);
  }
});

test("multiple physical or demo hosts stay ambiguous instead of choosing arbitrarily", () => {
  const one = host("one", { usbConnected: true });
  const two = host("two", { usbConnected: true });
  assert.throws(() => selectDeviceHost([one, two], "device.snapshot"), /ambiguous/);
  const a = host("a", { targetReady: true, brokerTarget: demo });
  const b = host("b", { targetReady: true, brokerTarget: { ...demo, sessionId: "other" } });
  assert.throws(() => selectDeviceHost([a, b], "device.snapshot"), /ambiguous/);
});
