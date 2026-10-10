import assert from "node:assert/strict";
import test from "node:test";
import { brokerPhaseDetectsDevice, normalizeBrokerConnection, qcConnectionDuringBrokerPhase } from "../src/qc-readiness.ts";

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

// Current behaviour, pinned so that changing the private contract is a
// visible event. The raw phase listener (App.tsx onDevicePhase) reads
// `connecting` as "a Quad Cortex was found, now opening it", but the result
// of device.reconnect / device.resetSession goes through
// normalizeBrokerConnection (tauri-transport.ts reconnect() / resetSession()),
// and the generated contract still aliases broker `connecting` to
// `searching`. When the private contract source stops aliasing it
// (QC_BROKER_CONNECTION_PHASE_ALIASES), update the second half of this test and
// packages/typescript/qc-client/tests/client-behavior.test.ts together.
test("today `connecting` is `opening` on the live phase path but `searching` on the reconnect-result path", () => {
  const live = qcConnectionDuringBrokerPhase("connecting", "");
  assert.equal(live?.phase, "opening");
  assert.equal(live?.demo, true);

  const result = normalizeBrokerConnection({
    phase: "connecting",
    detail: "Quad Cortex found; opening its USB session",
    demo: false,
    source: "device"
  } as Parameters<typeof normalizeBrokerConnection>[0]);
  assert.equal(result.phase, "searching");
  assert.equal(result.demo, true);
  assert.equal(result.detail, "Quad Cortex found; opening its USB session");
});
