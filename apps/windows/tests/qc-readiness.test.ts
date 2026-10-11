import assert from "node:assert/strict";
import test from "node:test";
import { brokerPhaseDetectsDevice, normalizeBrokerConnection, qcConnectionDuringBrokerPhase, qcFollowReadyInventory, qcInventoryDeviceReady } from "../src/qc-readiness.ts";

// The broker's startup phases as 2badc68 ("Report responsive USB startup
// progress") defines them: "searching" is enumeration finding nothing,
// "connecting" is published only after enumeration found a matching QC and
// the broker is opening HID, "handshaking" follows once that handle is open.

test("a broker phase proves a Quad Cortex is there from `connecting` on, and not before", () => {
  for (const phase of ["connecting", "handshaking", "syncing", "ready"]) {
    assert.equal(brokerPhaseDetectsDevice(phase), true, phase);
  }
  for (const phase of ["searching", "disconnected", "error", "future-phase", ""]) {
    assert.equal(brokerPhaseDetectsDevice(phase), false, phase);
  }
});

test("each detecting phase maps to the connection state shown during one connect", () => {
  assert.deepEqual(qcConnectionDuringBrokerPhase("connecting", "Quad Cortex found; opening its USB session"), {
    phase: "opening",
    detail: "Quad Cortex found; opening its USB connection…",
    demo: true,
    source: "device"
  });
  assert.deepEqual(qcConnectionDuringBrokerPhase("handshaking", "Waiting for Quad Cortex ResetCommsBuffers reply (attempt 2)"), {
    phase: "handshaking",
    detail: "Waiting for Quad Cortex ResetCommsBuffers reply (attempt 2)",
    demo: true,
    source: "device"
  });
  for (const phase of ["syncing", "ready"]) {
    assert.deepEqual(qcConnectionDuringBrokerPhase(phase, "ignored broker detail"), {
      phase: "syncing",
      detail: "Quad Cortex connected; synchronizing its live state…",
      demo: false,
      source: "device"
    }, phase);
  }
});

test("a handshake without broker detail falls back to the host's own wording", () => {
  const connection = qcConnectionDuringBrokerPhase("handshaking", "");
  assert.equal(connection?.phase, "handshaking");
  assert.equal(connection?.detail, "USB open; handshaking with the Quad Cortex…");
});

test("phases that do not prove a Quad Cortex map to no connection state", () => {
  for (const phase of ["searching", "disconnected", "error", "future-phase", ""]) {
    assert.equal(qcConnectionDuringBrokerPhase(phase, "some detail"), undefined, phase);
  }
});

test("the detection predicate and the phase mapping accept exactly the same phases", () => {
  // App.tsx's device-phase listener checks the predicate first and then reads
  // the mapping; a phase accepted by one and refused by the other would be
  // dropped silently or reach the UI as `undefined`.
  for (const phase of ["searching", "connecting", "handshaking", "syncing", "ready", "disconnected", "error", "future-phase", ""]) {
    assert.equal(brokerPhaseDetectsDevice(phase), qcConnectionDuringBrokerPhase(phase, "") !== undefined, phase);
  }
});

// The broker publishes `connecting` only after enumeration found a matching QC
// and it is being opened. Both paths read it as `opening`: the raw phase
// listener (App.tsx onDevicePhase) through qcConnectionDuringBrokerPhase, and
// the result of device.reconnect / device.resetSession through
// normalizeBrokerConnection (QC_BROKER_CONNECTION_PHASE_ALIASES).
test("`connecting` is `opening` on the live phase path and on the reconnect-result path", () => {
  const live = qcConnectionDuringBrokerPhase("connecting", "");
  assert.equal(live?.phase, "opening");
  assert.equal(live?.demo, true);

  const result = normalizeBrokerConnection({
    phase: "connecting",
    detail: "Quad Cortex found; opening its USB session",
    demo: false,
    source: "device"
  } as Parameters<typeof normalizeBrokerConnection>[0]);
  assert.equal(result.phase, "opening");
  assert.equal(result.demo, true);
  assert.equal(result.detail, "Quad Cortex found; opening its USB session");
});

// The inventory poll's safety net: a tray resident that never pushes
// `device.phase` left the window on the demo, beside a tray showing the QC
// connected, until someone clicked Reconnect.
type Connection = Parameters<typeof qcFollowReadyInventory>[0]["connection"];
const demoFallback: Connection = { phase: "ready", detail: "Demo device on this computer", demo: false, source: "virtual" };
const failedConnect: Connection = { phase: "needs-attention", detail: "The Quad Cortex was released from the tray", demo: true, source: "device" };
const liveDevice: Connection = { phase: "ready", detail: "Connected", demo: false, source: "device", lastSync: "2026-10-12T00:00:00.000Z" };
const userDisconnected: Connection = { phase: "disconnected", detail: "Quad Cortex disconnected", demo: true, source: "device" };
const failedDisconnect: Connection = { phase: "needs-attention", detail: "The Quad Cortex session could not be closed", demo: true, source: "device" };
const idleWindow = { inventoryReady: true, followed: false, userDisconnected: false, connectInFlight: false, connection: demoFallback, demoCanYield: true };

test("a device the broker already has ready is followed from an idle window", () => {
  assert.deepEqual(qcFollowReadyInventory(idleWindow), { connect: true, followed: true });
  assert.deepEqual(qcFollowReadyInventory({ ...idleWindow, connection: failedConnect }), { connect: true, followed: true });
});

test("a window already live with the device does not connect again, and spends the edge", () => {
  assert.deepEqual(qcFollowReadyInventory({ ...idleWindow, connection: liveDevice }), { connect: false, followed: true });
});

test("a connect in flight neither fires nor spends the edge, so its miss is followed once it ends", () => {
  const during = qcFollowReadyInventory({ ...idleWindow, connectInFlight: true });
  assert.deepEqual(during, { connect: false, followed: false });
  assert.deepEqual(qcFollowReadyInventory({ ...idleWindow, followed: during.followed }), { connect: true, followed: true });
});

test("a connect in flight after the edge was used stays quiet and keeps it used", () => {
  assert.deepEqual(qcFollowReadyInventory({ ...idleWindow, followed: true, connectInFlight: true }), { connect: false, followed: true });
});

test("a Disconnect that failed is not undone while the device stays ready", () => {
  // Pressed during sync, so no poll ever saw the window live and the edge is
  // unspent; the close failed, so the inventory still reads ready, and
  // "needs-attention" alone would be followed.
  const afterFailedDisconnect = { ...idleWindow, connection: failedDisconnect, userDisconnected: true };
  assert.deepEqual(qcFollowReadyInventory(afterFailedDisconnect), { connect: false, followed: false });
  // The latch alone holds it: the same state without it would connect.
  assert.equal(qcFollowReadyInventory({ ...afterFailedDisconnect, userDisconnected: false }).connect, true);
});

test("a deliberate Disconnect or Demo choice is never overridden", () => {
  assert.equal(qcFollowReadyInventory({ ...idleWindow, connection: userDisconnected }).connect, false);
  assert.equal(qcFollowReadyInventory({ ...idleWindow, demoCanYield: false }).connect, false);
});

test("only a connected device in the ready phase counts as ready", () => {
  assert.equal(qcInventoryDeviceReady([{ id: "qc-1", connected: true, phase: "ready" }]), true);
  assert.equal(qcInventoryDeviceReady([{ id: "qc-1", connected: false, phase: "ready" }]), false);
  assert.equal(qcInventoryDeviceReady([{ id: "qc-1", connected: false, phase: "disconnected" }]), false);
  for (const phase of ["connecting", "handshaking", "syncing", "searching", "error"]) {
    assert.equal(qcInventoryDeviceReady([{ id: "qc-1", connected: true, phase }]), false, phase);
  }
  assert.equal(qcInventoryDeviceReady([{ id: "qc-1" }]), false);
  assert.equal(qcInventoryDeviceReady([]), false);
  assert.deepEqual(qcFollowReadyInventory({ ...idleWindow, inventoryReady: false }), { connect: false, followed: false });
});

test("the selected device decides, and several without a selection decide nothing", () => {
  const devices = [{ id: "qc-1", connected: true, phase: "ready" }, { id: "qc-2", connected: false, phase: "disconnected" }];
  assert.equal(qcInventoryDeviceReady(devices, "qc-1"), true);
  assert.equal(qcInventoryDeviceReady(devices, "qc-2"), false);
  assert.equal(qcInventoryDeviceReady(devices, "qc-gone"), false);
  assert.equal(qcInventoryDeviceReady(devices), false);
  assert.equal(qcInventoryDeviceReady(devices, null), false);
});

test("at most one connect per transition into ready, a refused one included", () => {
  // Poll by poll: ready -> connect; the attempt is refused and the window is
  // back on the demo while the device stays ready -> nothing more; the device
  // leaves ready -> re-armed; ready again -> exactly one more connect.
  let followed = false;
  const poll = (inventoryReady: boolean) => {
    const step = qcFollowReadyInventory({ ...idleWindow, inventoryReady, followed });
    followed = step.followed;
    return step.connect;
  };
  const connects = [true, true, true, false, true, true].map(poll);
  assert.deepEqual(connects, [true, false, false, false, true, false]);
});
