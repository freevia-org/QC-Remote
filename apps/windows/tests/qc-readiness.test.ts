import assert from "node:assert/strict";
import test from "node:test";
import { brokerPhaseDetectsDevice, normalizeBrokerConnection, QC_BROKER_TRUTH_UNKNOWN, QC_CONNECT_STALL_MS, QC_FOLLOW_DEMO_ATTEMPTS, QC_FOLLOW_INITIAL, QC_FOLLOW_RETRY_MAX_MS, QC_FOLLOW_RETRY_MIN_MS, QC_FOLLOW_STABLE_LIVE_MS, QC_ORPHANED_DISCOVERING_MS, QC_SESSION_LEFT_OPEN_NOTICE, qcAutomaticConnectHeld, qcBackgroundDeviceLive, qcConnectionDuringBrokerPhase, qcDisconnectClosedSession, qcDisconnectOutcome, qcFollowAfterUserConnect, qcFollowBroker, qcFollowRetryDelay, qcInventoryDeviceReady, qcInventoryWhenUnreachable, qcLatchUserDisconnect, qcObserveBroker, qcPhaseListenerMayFollow, qcReconcilePhysicalDevices, qcRecordLiveSessionLoss, qcSameBrokerTruth, qcShouldFollowDevicePhase, qcStopDemoOutcome, qcUserDisconnectAfter, qcUserDisconnectHolds, qcWatchConnectStall, waitForSynchronizedQc, type QcBrokerObservation, type QcBrokerTruth, type QcFollowState, type QcUserDisconnect } from "../src/qc-readiness.ts";

test("connect waits for transient broker synchronization before falling back to demo", async () => {
  const waiting = { gatewayAvailable: true, platform: "Linux", message: "", usbDiagnostics: { phase: "ready", connected: true, synchronized: false, detail: "Preset sync pending" } };
  const ready = { ...waiting, usbDiagnostics: { ...waiting.usbDiagnostics, synchronized: true } };
  const states = [waiting, waiting, ready];
  let reads = 0;
  const result = await waitForSynchronizedQc(async () => states[Math.min(reads++, states.length - 1)]!, () => true, {
    intervalMs: 1,
    delay: async () => undefined
  });
  assert.equal(reads, 3);
  assert.equal(result, ready);
});

test("connect tolerates a transient runtime-status read failure during USB startup", async () => {
  const ready = { gatewayAvailable: true, platform: "Linux", message: "", usbDiagnostics: { phase: "ready", connected: true, synchronized: true, detail: "" } };
  let reads = 0;
  const result = await waitForSynchronizedQc(async () => {
    if (reads++ === 0) throw new Error("broker status is not ready yet");
    return ready;
  }, () => true, { attempts: 3, intervalMs: 1, delay: async () => undefined });
  assert.equal(reads, 2);
  assert.equal(result, ready);
});

test("a superseded connect stops waiting for synchronized broker state", async () => {
  const waiting = { gatewayAvailable: true, platform: "Linux", message: "", usbDiagnostics: { phase: "ready", connected: true, synchronized: false, detail: "Preset sync pending" } };
  let current = true;
  let reads = 0;
  const result = await waitForSynchronizedQc(async () => { reads += 1; current = false; return waiting; }, () => current, {
    intervalMs: 1,
    delay: async () => undefined
  });
  assert.equal(result, undefined);
  assert.equal(reads, 1);
});

test("a stalled runtime-status read is bounded by the synchronization deadline", async () => {
  await assert.rejects(waitForSynchronizedQc(() => new Promise(() => undefined), () => true, { maxWaitMs: 20 }), /Timed out waiting for synchronized Quad Cortex status/);
});

test("device inventory distinguishes a stale selected row from a same-label re-enumerated device", () => {
  const old = { id: "linux:hidraw:old", name: "Quad Cortex", connected: true, phase: "ready" };
  const replacement = { id: "linux:hidraw:new", name: "Quad Cortex", connected: true, phase: "ready" };
  assert.deepEqual(qcReconcilePhysicalDevices([replacement], old.id, [old]), [replacement, { ...old, name: "Quad Cortex (not detected)", connected: false, phase: "disconnected" }]);
  assert.deepEqual(qcReconcilePhysicalDevices([], old.id, [old]), [{ ...old, connected: false, phase: "disconnected" }], "keep a disconnected placeholder while enumeration is empty");
  assert.deepEqual(qcReconcilePhysicalDevices([old], old.id, [old]), [old], "do not duplicate a selected row still present");
});

test("pushed detection cannot undo failed Disconnect; user Connect permits the selected QC again", () => {
  for (const phase of ["connecting", "handshaking", "syncing", "ready"]) {
    assert.equal(qcShouldFollowDevicePhase({ phase, deviceId: "qc-a" }, "qc-a", true), false, phase);
    assert.equal(qcShouldFollowDevicePhase({ phase, deviceId: "qc-a" }, "qc-a", false), true, phase);
    assert.equal(qcShouldFollowDevicePhase({ phase }, "qc-a", false), true, "shared-pool event omits id");
    assert.equal(qcShouldFollowDevicePhase({ phase, deviceId: "qc-b" }, "qc-a", false), false, "another QC");
  }
  assert.equal(qcShouldFollowDevicePhase({ phase: "searching" }, undefined, false), false);
});

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

// The window mirrors the broker: broker truth comes from the 2 s inventory
// poll (qcObserveBroker), and the follow (qcFollowBroker) connects whenever
// that truth reads the device ready and the window is not live, unless the
// user's Disconnect, their Demo choice, a tray release or a Quit holds it.
type Connection = Parameters<typeof qcFollowBroker>[0]["connection"];
const demoFallback: Connection = { phase: "ready", detail: "Demo device on this computer", demo: false, source: "virtual" };
const failedConnect: Connection = { phase: "needs-attention", detail: "The Quad Cortex was released from the tray", demo: true, source: "device" };
const liveDevice: Connection = { phase: "ready", detail: "Connected", demo: false, source: "device", lastSync: "2026-10-12T00:00:00.000Z" };
const userDisconnected: Connection = { phase: "disconnected", detail: "Quad Cortex disconnected", demo: true, source: "device" };
const failedDisconnect: Connection = { phase: "needs-attention", detail: "The Quad Cortex session could not be closed", demo: true, source: "device" };
const POLL_MS = 2000;

const answered = (inventoryReady: boolean, instanceId: string | null = "broker-1"): QcBrokerObservation => ({ kind: "answered", inventoryReady, instanceId });
const unreachable: QcBrokerObservation = { kind: "unreachable" };
/** A `device.phase` detection push before any poll answered: ready, with no instance id. */
const detectionPush: QcBrokerObservation = { kind: "answered", inventoryReady: true };

/**
 * App's follow loop, observation by observation, on a fake monotonic clock
 * that advances by the real poll's 2 s. A connect it decides holds every
 * later decision for `attemptPolls` polls (the handshake), as App's
 * qcAutomaticConnectHeld does; the caller then says how it ended (`goLive`,
 * `failConnect`).
 */
function windowPolls(start: Connection = demoFallback, attemptPolls = 0) {
  let truth: QcBrokerTruth = QC_BROKER_TRUTH_UNKNOWN;
  let follow: QcFollowState = QC_FOLLOW_INITIAL;
  let latch: QcUserDisconnect | null = null;
  let connection = start;
  let now = 0;
  let heldPolls = 0;
  let demoGaveUp = false;
  const connects: number[] = [];
  const sim = {
    held: false,
    demoCanYield: true,
    get now() { return now; },
    get truth() { return truth; },
    get follow() { return follow; },
    get latch() { return latch; },
    get latched() { return qcUserDisconnectHolds(latch); },
    get demoGaveUp() { return demoGaveUp; },
    get connects() { return connects; },
    get connection() { return connection; },
    observe(observation: QcBrokerObservation) {
      now += POLL_MS;
      truth = qcObserveBroker(truth, observation);
      latch = qcUserDisconnectAfter(latch, truth);
      const connectHeld = sim.held || heldPolls > 0;
      if (heldPolls > 0) heldPolls -= 1;
      const step = qcFollowBroker({ follow, truth, now, userDisconnect: latch, connectHeld, connection, demoCanYield: sim.demoCanYield });
      follow = step.follow;
      latch = step.userDisconnect;
      demoGaveUp = step.demoGaveUp;
      if (step.connect) { connects.push(now); heldPolls = attemptPolls; }
      return step.connect;
    },
    ready(instanceId: string | null = "broker-1") { return sim.observe(answered(true, instanceId)); },
    notReady(instanceId: string | null = "broker-1") { return sim.observe(answered(false, instanceId)); },
    refused() { return sim.observe(unreachable); },
    push() { return sim.observe(detectionPush); },
    /** Polls for about `ms`, each made by `poll`; returns how many connected. */
    for(ms: number, poll: () => boolean) { let count = 0; for (let waited = 0; waited < ms; waited += POLL_MS) if (poll()) count += 1; return count; },
    /** Polls that read ready while every connect they start fails back to `fallback`. */
    failing(ms: number, fallback: Connection, poll: () => boolean = () => sim.ready()) {
      return sim.for(ms, () => { const connected = poll(); if (connected) connection = fallback; return connected; });
    },
    goLive() { connection = liveDevice; },
    failConnect(fallback: Connection = demoFallback) { connection = fallback; },
    /** beginLiveRecovery: the health check lost the session; back on the demo. */
    loseLive() { follow = qcRecordLiveSessionLoss(follow); connection = demoFallback; },
    disconnect(outcome: Connection = userDisconnected) { latch = qcLatchUserDisconnect(truth); connection = outcome; },
    userConnect() { latch = null; follow = qcFollowAfterUserConnect(follow); },
    setConnection(next: Connection) { connection = next; }
  };
  return sim;
}
const gapsOf = (times: readonly number[]) => times.slice(1).map((at, index) => at - times[index]);

test("broker truth keeps what the broker answered apart from a poll that failed", () => {
  let truth = qcObserveBroker(QC_BROKER_TRUTH_UNKNOWN, answered(true));
  assert.deepEqual(truth, { reachable: true, deviceReady: true, answeredReady: true, instanceId: "broker-1", generation: 0, readyEdges: 1 });
  truth = qcObserveBroker(truth, unreachable);
  assert.deepEqual(truth, { reachable: false, deviceReady: false, answeredReady: true, instanceId: "broker-1", generation: 0, readyEdges: 1 }, "gray now, answer kept");
  truth = qcObserveBroker(truth, answered(true));
  assert.equal(truth.readyEdges, 1, "ready again after a failed poll is no new edge");
  assert.equal(truth.generation, 0, "nor a new generation");
  truth = qcObserveBroker(truth, answered(false));
  truth = qcObserveBroker(truth, answered(true));
  assert.equal(truth.readyEdges, 2, "an answered not-ready, then ready: an edge");
  truth = qcObserveBroker(truth, answered(true, "broker-2"));
  assert.equal(truth.generation, 1, "a different instance is a new generation");
  assert.equal(truth.readyEdges, 3, "and its ready device a new session, with no gap seen");
  truth = qcObserveBroker(truth, unreachable);
  truth = qcObserveBroker(truth, detectionPush);
  assert.equal(truth.generation, 1, "an answer without an instance id never moves the generation");
  assert.equal(truth.readyEdges, 3, "nor makes an edge after a failed poll");
  assert.equal(qcSameBrokerTruth(truth, qcObserveBroker(truth, answered(true, "broker-2"))), true, "nothing to redraw");
  assert.equal(qcSameBrokerTruth(truth, qcObserveBroker(truth, unreachable)), false);
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
});

test("the selected device decides, and several without a selection decide nothing", () => {
  const devices = [{ id: "qc-1", connected: true, phase: "ready" }, { id: "qc-2", connected: false, phase: "disconnected" }];
  assert.equal(qcInventoryDeviceReady(devices, "qc-1"), true);
  assert.equal(qcInventoryDeviceReady(devices, "qc-2"), false);
  assert.equal(qcInventoryDeviceReady(devices, "qc-gone"), false);
  assert.equal(qcInventoryDeviceReady(devices), false);
  assert.equal(qcInventoryDeviceReady(devices, null), false);
});

test("the poll's failure path clears every green row, keeping ids and names", () => {
  const rows = [{ id: "qc-1", name: "Quad Cortex 1", connected: true, phase: "ready" }, { id: "qc-2", name: "Quad Cortex 2", connected: true, phase: "syncing" }];
  const cleared = qcInventoryWhenUnreachable(rows);
  assert.deepEqual(cleared, [
    { id: "qc-1", name: "Quad Cortex 1", connected: false, phase: "disconnected" },
    { id: "qc-2", name: "Quad Cortex 2", connected: false, phase: "disconnected" }
  ]);
  assert.equal(qcInventoryWhenUnreachable(cleared), cleared, "a broker that stays away changes nothing more");
  assert.deepEqual(qcInventoryWhenUnreachable([]), []);
  // The same poll's truth: the selected device reads not ready at once.
  const app = windowPolls(liveDevice);
  app.ready();
  app.refused();
  assert.equal(app.truth.deviceReady, false);
});

test("a device held behind the demo is shown live only while the demo is active and the broker says so", () => {
  const synchronized = { platform: "Windows", gatewayAvailable: true, message: "", usbDiagnostics: { phase: "ready", detail: "", connected: true, synchronized: true } };
  const ready = qcObserveBroker(QC_BROKER_TRUTH_UNKNOWN, answered(true));
  assert.equal(qcBackgroundDeviceLive(true, true, ready, synchronized), true);
  assert.equal(qcBackgroundDeviceLive(false, true, ready, synchronized), false, "nothing held");
  assert.equal(qcBackgroundDeviceLive(true, false, ready, synchronized), false, "no demo: an idle USB panel never offers Disconnect for a stale flag");
  assert.equal(qcBackgroundDeviceLive(true, true, qcObserveBroker(ready, unreachable), synchronized), false, "broker quit or released");
  assert.equal(qcBackgroundDeviceLive(true, true, qcObserveBroker(ready, answered(false)), synchronized), false, "cable out");
  assert.equal(qcBackgroundDeviceLive(true, true, ready, { ...synchronized, gatewayAvailable: false }), false, "status poll failed");
  assert.equal(qcBackgroundDeviceLive(true, true, ready, { ...synchronized, usbDiagnostics: { ...synchronized.usbDiagnostics, synchronized: false } }), false);
  assert.equal(qcBackgroundDeviceLive(true, true, QC_BROKER_TRUTH_UNKNOWN, synchronized), false, "no poll has answered yet");
});

test("Stop demo resumes only a device still live behind it, and otherwise clears the stale held flag", () => {
  assert.deepEqual(qcStopDemoOutcome(true), { resume: true, clearHeld: false });
  assert.deepEqual(qcStopDemoOutcome(false), { resume: false, clearHeld: true });
});

test("a device the broker already has ready is followed from an idle window, at once", () => {
  for (const start of [demoFallback, failedConnect]) {
    const app = windowPolls(start);
    assert.equal(app.ready(), true, start.phase);
  }
});

test("a window already live with the device does not connect again", () => {
  const app = windowPolls(liveDevice);
  assert.equal(app.for(10 * 60_000, () => app.ready()), 0);
});

test("broker quits (tray Quit or release) and restarts without a Disconnect: the window goes live again, every time", () => {
  const app = windowPolls();
  for (let cycle = 0; cycle < 6; cycle += 1) {
    const before = `broker-${cycle}`;
    const after = `broker-${cycle + 1}`;
    if (cycle === 0) { assert.equal(app.ready(before), true); app.goLive(); }
    assert.equal(app.for(10_000, () => app.ready(before)), 0, `cycle ${cycle}: live`);
    // Quit: the health check loses the session, and the host refuses every
    // poll for the released generation for as long as it is away.
    app.loseLive();
    assert.equal(app.for(5 * 60_000, () => app.refused()), 0, `cycle ${cycle}: away`);
    assert.equal(app.connection.source, "virtual", "on the demo meanwhile");
    // A new generation answers; its device takes a moment, then reads ready.
    assert.equal(app.notReady(after), false, `cycle ${cycle}: starting`);
    assert.equal(app.ready(after), true, `cycle ${cycle}: followed`);
    app.goLive();
  }
  assert.equal(app.connects.length, 7, "the first connect and one per restart");
});

test("a broker that crashes and comes back already connected is followed without any gap observed", () => {
  const app = windowPolls();
  assert.equal(app.ready("broker-1"), true);
  app.goLive();
  app.loseLive();
  assert.equal(app.ready("broker-2"), true, "a new instance is a new generation, ready or not before");
});

test("USB unplug and replug without a Disconnect: followed on every replug, however often", () => {
  const app = windowPolls();
  assert.equal(app.ready(), true);
  app.goLive();
  for (let cycle = 0; cycle < 8; cycle += 1) {
    app.loseLive();
    assert.equal(app.for(6_000, () => app.notReady()), 0, `cycle ${cycle}: unplugged`);
    assert.equal(app.for(12_000, () => app.notReady()), 0, `cycle ${cycle}: handshaking`);
    assert.equal(app.ready(), true, `cycle ${cycle}: replugged and ready`);
    app.goLive();
    assert.equal(app.for(4_000, () => app.ready()), 0);
  }
});

test("from an idle or failed window, a follow that keeps failing is retried at a growing interval, never given up", () => {
  const app = windowPolls(failedConnect);
  app.failing(30 * 60_000, failedConnect);
  const gaps = gapsOf(app.connects);
  assert.ok(app.connects.length > 25, `still converging after 30 min: ${app.connects.length}`);
  // Each wait starts at the first poll that sees the attempt over.
  assert.deepEqual(gaps.slice(0, 5), [4_000, 8_000, 16_000, 32_000, 60_000].map((delay) => delay + POLL_MS));
  for (const gap of gaps) assert.ok(gap >= QC_FOLLOW_RETRY_MIN_MS && gap <= QC_FOLLOW_RETRY_MAX_MS + POLL_MS, `gap ${gap}`);
  assert.equal(app.demoGaveUp, false, "not a demo: nothing to protect");
  assert.equal(qcFollowRetryDelay(1), QC_FOLLOW_RETRY_MIN_MS);
  assert.equal(qcFollowRetryDelay(50), QC_FOLLOW_RETRY_MAX_MS);
});

test("the retry wait is measured from the end of a long attempt, not from its start", () => {
  const handshakePolls = 10;
  const app = windowPolls(failedConnect, handshakePolls);
  app.failing(5 * 60_000, failedConnect);
  const gaps = gapsOf(app.connects);
  assert.equal(gaps[0], (handshakePolls + 1) * POLL_MS + QC_FOLLOW_RETRY_MIN_MS);
  assert.equal(gaps[1], (handshakePolls + 1) * POLL_MS + 2 * QC_FOLLOW_RETRY_MIN_MS);
});

test("failed polls never restart the backoff: one failed poll in three still settles on the ceiling", () => {
  // The probe: ten minutes of a broker that reads ready while every connect
  // fails, with one poll in three failing outright.
  const app = windowPolls(failedConnect);
  let poll = 0;
  const connects = app.failing(10 * 60_000, failedConnect, () => (poll += 1) % 3 === 0 ? app.refused() : app.ready());
  // About 4 ramp-up attempts, then one per ceiling interval: near 14, nowhere near 100.
  assert.ok(connects >= 8 && connects <= 16, `${connects} connects`);
  assert.ok(gapsOf(app.connects).slice(-4).every((gap) => gap >= QC_FOLLOW_RETRY_MAX_MS), `${gapsOf(app.connects).slice(-4)}`);
});

test("a working demo is taken over at most three times on one ready edge, then stays, and says so", () => {
  const app = windowPolls(demoFallback);
  const connects = app.failing(30 * 60_000, demoFallback);
  assert.equal(connects, QC_FOLLOW_DEMO_ATTEMPTS);
  assert.equal(app.demoGaveUp, true, "App surfaces a Connect notice for this");
  // A new answered ready edge (replug) starts over.
  app.notReady();
  assert.equal(app.ready(), true, "a new edge");
  assert.equal(app.demoGaveUp, false);
  app.failConnect(demoFallback);
  app.failing(30 * 60_000, demoFallback);
  assert.equal(app.connects.length, 2 * QC_FOLLOW_DEMO_ATTEMPTS);
  // So does a new broker generation, and so does the user's Connect.
  assert.equal(app.ready("broker-2"), true);
  app.failConnect(demoFallback);
  app.failing(30 * 60_000, demoFallback, () => app.ready("broker-2"));
  assert.equal(app.connects.length, 3 * QC_FOLLOW_DEMO_ATTEMPTS);
  app.userConnect();
  assert.equal(app.ready("broker-2"), true, "the user's Connect starts over");
});

test("short live sessions do not trip the demo cap; a session that drops soon after every connect backs off to the ceiling", () => {
  const app = windowPolls();
  app.for(20 * 60_000, () => {
    const connected = app.ready();
    if (connected) app.goLive();
    else if (app.connection.source === "device") app.loseLive();
    return connected;
  });
  assert.ok(app.connects.length > QC_FOLLOW_DEMO_ATTEMPTS, `each reached live, so none counts against the demo: ${app.connects.length}`);
  assert.equal(app.demoGaveUp, false);
  const gaps = gapsOf(app.connects);
  assert.ok(gaps.every((gap) => gap >= QC_FOLLOW_RETRY_MIN_MS), "never back to back");
  assert.ok(gaps.slice(-3).every((gap) => gap >= QC_FOLLOW_RETRY_MAX_MS), `settles on the ceiling: ${gaps.slice(-3)}`);
});

test("the answer to the user's Disconnect decides whether a latch is kept", () => {
  const latch = qcLatchUserDisconnect(QC_BROKER_TRUTH_UNKNOWN);
  const closed = { phase: "disconnected" as const, detail: "Quad Cortex disconnected", demo: true, source: "device" as const };
  assert.equal(qcDisconnectClosedSession(closed), true);
  assert.deepEqual(qcDisconnectOutcome(latch, closed), { closed: true, connection: closed, latch, notice: "Quad Cortex disconnected" });
  assert.equal(qcDisconnectClosedSession({ phase: "disconnected", detail: "Disconnected this window. The Quad Cortex session stays open while another QC Remote window or MCP client is still using it" }), false);
});

test("a Disconnect the broker answers with the session left open never bounces through a reconnect, and keeps its reason", () => {
  const app = windowPolls();
  app.ready();
  app.goLive();
  app.ready();
  // The user's Disconnect; the broker leaves the shared session open.
  app.disconnect();
  const leftOpen = { phase: "ready" as const, detail: "Another client is still attached; the shared session was left open", demo: false, source: "device" as const };
  assert.equal(qcDisconnectClosedSession(leftOpen), false);
  const outcome = qcDisconnectOutcome(app.latch, leftOpen);
  assert.equal(outcome.closed, false);
  assert.equal(outcome.connection.phase, "disconnected", "never shown ready from the raw, unsynchronized answer");
  assert.equal(outcome.notice, leftOpen.detail, "the broker's reason is what stays on screen");
  assert.equal(qcUserDisconnectHolds(outcome.latch), true, "held while App restores Live without a reconnect");
  app.setConnection(outcome.connection);
  // The broker keeps reading the device ready: no follow fires meanwhile.
  assert.equal(app.for(60_000, () => app.ready()), 0, "no automatic device.reconnect");
  // A reason-less answer still says why.
  assert.equal(qcDisconnectOutcome(app.latch, { ...leftOpen, phase: "ready", detail: "" }).notice, QC_SESSION_LEFT_OPEN_NOTICE);
  // App's restore succeeded: Live again, the latch dropped with it, and
  // nothing more to follow.
  app.userConnect();
  app.goLive();
  assert.equal(app.for(60_000, () => app.ready()), 0);
});
test("a session that stayed live starts the attempts over; a new edge or generation does too", () => {
  const app = windowPolls(failedConnect);
  app.failing(4 * 60_000, failedConnect);
  assert.ok(app.follow.attempts > 3);
  app.ready();
  app.goLive();
  app.for(QC_FOLLOW_STABLE_LIVE_MS + 2 * POLL_MS, () => app.ready());
  assert.equal(app.follow.attempts, 0, "stable live time");
  app.loseLive();
  assert.equal(app.ready(), true, "and the next loss is followed at once");
  app.failConnect(failedConnect);
  assert.equal(app.ready(), false, "inside the first backoff");
  assert.equal(app.notReady(), false);
  assert.equal(app.ready(), true, "a new answered ready edge does not wait for the backoff");
  app.failConnect(failedConnect);
  assert.equal(app.ready("broker-9"), true, "nor does a new broker generation");
});

test("a connect in flight, a gave-up reconnect still unsettled, or a command holds every follow", () => {
  const app = windowPolls();
  app.held = true;
  assert.equal(app.for(5 * 60_000, () => app.ready()), 0);
  app.held = false;
  assert.equal(app.ready(), true, "followed once it settles, with no attempt spent while held");
  for (const state of [{ awaitingDetection: true }, { commandPending: true }, { demoPending: true }, { reconnectsOutstanding: 1 }]) {
    assert.equal(qcAutomaticConnectHeld({ awaitingDetection: false, commandPending: false, demoPending: false, reconnectsOutstanding: 0, ...state }), true);
  }
  assert.equal(qcAutomaticConnectHeld({ awaitingDetection: false, commandPending: false, demoPending: false, reconnectsOutstanding: 0 }), false);
});

test("a deliberate Demo choice or a stopped demo is never overridden", () => {
  const chosen = windowPolls();
  chosen.demoCanYield = false;
  assert.equal(chosen.for(5 * 60_000, () => chosen.ready()), 0);
  const stopped = windowPolls({ phase: "disconnected", detail: "Demo device stopped. Its presets stay saved on this computer.", demo: true, source: "device" });
  assert.equal(stopped.for(5 * 60_000, () => stopped.ready()), 0);
});

test("the user's Disconnect stays disconnected across polls, a failed Disconnect included", () => {
  const app = windowPolls();
  app.ready();
  app.goLive();
  app.ready();
  app.disconnect(failedDisconnect);
  // The close failed: the broker keeps the device open and ready.
  assert.equal(app.for(10 * 60_000, () => app.ready()), 0);
  assert.equal(app.latched, true);
  // A Connect of their own lifts it.
  app.userConnect();
  assert.equal(app.ready(), true);
});

test("the user's Disconnect holds while the device stays closed, and a poll that answered before the close never lifts it", () => {
  const app = windowPolls();
  app.notReady();
  // Pressed during sync: the last poll saw the device not ready yet, and the
  // next one still reads it ready before the close lands.
  app.disconnect(failedDisconnect);
  assert.equal(app.ready(), false);
  assert.equal(app.latched, true);
  assert.equal(app.for(10 * 60_000, () => app.notReady()), 0, "closed: nothing to follow");
  assert.equal(app.latched, true);
});

test("failed polls while the same broker keeps the device open never lift the user's Disconnect", () => {
  const app = windowPolls();
  app.ready();
  app.goLive();
  app.disconnect(failedDisconnect);
  for (let blip = 0; blip < 10; blip += 1) {
    assert.equal(app.refused(), false);
    assert.equal(app.ready(), false, `blip ${blip}`);
  }
  assert.equal(app.latched, true);
});

test("the user's Disconnect is lifted by an answered not-ready -> ready device session, and then dropped once live", () => {
  const app = windowPolls();
  app.ready();
  app.goLive();
  app.disconnect();
  app.notReady();
  assert.equal(app.latched, true);
  assert.equal(app.ready(), true, "the device closed and came back ready: a session the broker opened");
  assert.equal(app.latched, false);
  assert.equal(app.latch?.lifted, true, "kept while the Disconnect's own state is left");
  app.goLive();
  app.ready();
  assert.equal(app.latch, null, "dropped once live");
});

test("a new broker generation alone never lifts the user's Disconnect (owner decision)", () => {
  const app = windowPolls();
  app.ready("broker-1");
  app.goLive();
  app.disconnect(failedDisconnect);
  assert.equal(app.for(60_000, () => app.ready("broker-1")), 0);
  // The broker restarts - or this window's own host respawned it - and the
  // new generation opens the device by itself, even through a full
  // not-ready -> ready sequence.
  assert.equal(app.for(20_000, () => app.refused()), 0);
  assert.equal(app.ready("broker-2"), false, "a new generation, ready at once");
  assert.equal(app.notReady("broker-2"), false);
  assert.equal(app.for(10 * 60_000, () => app.ready("broker-2")), 0, "and a session edge inside it");
  assert.equal(app.latched, true);
  app.userConnect();
  assert.equal(app.ready("broker-2"), true, "only the user's Connect");
});

test("only the Disconnect's own state is followed once lifted: a stopped demo or a Demo choice is not", () => {
  const lifted: QcUserDisconnect = { generation: 0, sawNotReady: true, lifted: true };
  const truth = qcObserveBroker(QC_BROKER_TRUTH_UNKNOWN, answered(true));
  const decide = (connection: Connection, userDisconnect: QcUserDisconnect | null = lifted, demoCanYield = true) =>
    qcFollowBroker({ follow: QC_FOLLOW_INITIAL, truth, now: 0, userDisconnect, connectHeld: false, connection, demoCanYield }).connect;
  assert.equal(decide(userDisconnected), true);
  assert.equal(decide({ phase: "disconnected", detail: "Demo device stopped.", demo: true, source: "device" }), true, "App drops the latch on Stop demo, so this state never meets a lifted one");
  assert.equal(decide(userDisconnected, null), false, "no latch: a disconnected window stays idle");
  assert.equal(decide(demoFallback, lifted, false), false, "the user's Demo choice");
  assert.equal(qcUserDisconnectHolds({ ...lifted, lifted: false }), true);
  assert.equal(qcUserDisconnectHolds(lifted), false);
  assert.equal(qcUserDisconnectHolds(null), false);
});

test("a tray release or a recorded Quit is never reconnected automatically", () => {
  // The host refuses broker.devices for the released generation and the
  // broker's own status with it, so every poll fails for as long as nothing
  // newer is attached - recorded losses and an expired backoff included.
  const app = windowPolls();
  app.ready();
  app.goLive();
  app.loseLive();
  assert.equal(app.for(60 * 60_000, () => app.refused()), 0);
  assert.equal(app.truth.deviceReady, false);
  // A push decides by itself only until the first poll answered, so a phase
  // event from anywhere cannot reconnect a window whose polls are refused.
  assert.equal(qcPhaseListenerMayFollow(true), false);
  assert.equal(qcPhaseListenerMayFollow(false), true);
});

test("detection pushes follow once, then wait like the polls, and never restart the backoff after a failed poll", () => {
  // Before any poll answered, App feeds each push as an answered ready with
  // no instance id.
  const app = windowPolls(failedConnect);
  assert.equal(app.push(), true, "an unspent edge is followed once");
  app.failConnect(failedConnect);
  assert.equal(app.push(), false, "a second push is inside the backoff");
  assert.equal(app.refused(), false);
  const edges = app.truth.readyEdges;
  app.push();
  assert.equal(app.truth.readyEdges, edges, "a push after a failed poll is no new edge");
  assert.equal(app.truth.generation, 0, "and no new generation");
  assert.ok(app.follow.attempts >= 1, "the attempts were not started over");
  // A burst of pushes over ten minutes is still paced by the backoff.
  const connects = app.failing(10 * 60_000, failedConnect, () => app.push());
  assert.ok(connects <= 14, `${connects} connects`);
});

test("a push never undoes the user's Disconnect", () => {
  for (const phase of ["connecting", "handshaking", "syncing", "ready"]) {
    assert.equal(qcShouldFollowDevicePhase({ phase }, undefined, qcUserDisconnectHolds({ generation: 0, sawNotReady: false, lifted: false })), false, phase);
  }
  const app = windowPolls();
  app.push();
  app.goLive();
  app.disconnect(failedDisconnect);
  assert.equal(app.for(60_000, () => app.push()), 0);
  assert.equal(app.latched, true, "a push is no answered not-ready");
});

test("a connect given up by the watchdog is followed again once its reconnect settles, under the backoff", () => {
  // The watchdog itself never connects; App records the abandoned connect as
  // a lost session, and the reconnect it left outstanding holds the follow.
  const app = windowPolls(failedConnect);
  assert.equal(app.ready(), true);
  app.failConnect(failedConnect);
  app.held = true;
  assert.equal(app.for(5 * 60_000, () => app.ready()), 0);
  app.held = false;
  assert.equal(app.ready(), false, "the wait starts when it settled");
  assert.equal(app.for(QC_FOLLOW_RETRY_MIN_MS, () => app.ready()), 1);
  // A Disconnect after an abandoned connect is still never undone.
  app.disconnect(failedDisconnect);
  assert.equal(app.for(10 * 60_000, () => app.ready()), 0);
});

test("a recorded loss restarts the live measurement; a user-action loss and the user's Connect restart the attempts", () => {
  const spent: QcFollowState = { attempts: 3, nextAt: 99_000, inFlight: false, fromDemo: true, demoFailures: 2, liveSince: 5_000, seenGeneration: 0, seenReadyEdges: 1 };
  assert.deepEqual(qcRecordLiveSessionLoss(spent), { ...spent, liveSince: null });
  assert.deepEqual(qcRecordLiveSessionLoss(spent, true), { ...spent, liveSince: null, attempts: 0, nextAt: null, demoFailures: 0 });
  assert.deepEqual(qcFollowAfterUserConnect(spent), { ...spent, attempts: 0, nextAt: null, demoFailures: 0 });
});

test("a session lost before any poll saw it live for long is not credited as stable time", () => {
  const app = windowPolls();
  app.ready();
  app.goLive();
  app.for(QC_FOLLOW_STABLE_LIVE_MS - 10_000, () => app.ready());
  app.loseLive();
  app.goLive();
  app.for(20_000, () => app.ready());
  assert.ok(app.follow.liveSince !== null && app.now - app.follow.liveSince < QC_FOLLOW_STABLE_LIVE_MS);
});

// The connect watchdog (qcWatchConnectStall): a connect() whose answer never
// comes, or a "discovering" fallback nothing owns, must not stay gray.

const discovering: Connection = { phase: "discovering", detail: "Looking for a Quad Cortex…", demo: true, source: "device" };
const returning: Connection = { phase: "discovering", detail: "The Quad Cortex was not reached; returning to the demo device…", demo: true, source: "device" };
const handshaking: Connection = { phase: "handshaking", detail: "USB open; handshaking with the Quad Cortex…", demo: true, source: "device" };
const demoStopped: Connection = { phase: "disconnected", detail: "Demo device stopped. Its presets stay saved on this computer.", demo: true, source: "device" };
type StallInput = Parameters<typeof qcWatchConnectStall>[0];
const fromDemoConnect = { attempt: 1, fromDemo: true };
const fromIdleConnect = { attempt: 1, fromDemo: false };
function watchdog(start: Omit<StallInput, "stall" | "now">) {
  let stall: StallInput["stall"] = null;
  let now = 0;
  let state = start;
  return {
    set(next: Partial<Omit<StallInput, "stall" | "now">>) { state = { ...state, ...next }; },
    tick(ms = POLL_MS) {
      now += ms;
      const step = qcWatchConnectStall({ ...state, stall, now });
      stall = step.stall;
      return step.abandon;
    }
  };
}
const ticksUntilAbandon = (dog: ReturnType<typeof watchdog>, max = 1000) => {
  for (let tick = 1; tick <= max; tick += 1) if (dog.tick()) return tick;
  return Infinity;
};
const STALL_TICKS = QC_CONNECT_STALL_MS / POLL_MS + 1;

test("a connect still waiting is given up after the stall interval, not before", () => {
  const dog = watchdog({ connection: discovering, connect: fromDemoConnect, otherWorkPending: false });
  // The first tick starts the clock; abandonment needs the full interval after it.
  assert.equal(ticksUntilAbandon(dog), STALL_TICKS);
  assert.ok(QC_CONNECT_STALL_MS > 30_000 + 11_000, "longer than a full handshake plus the boot read");
});

test("a stalled connect returns to the demo only if it started from the demo", () => {
  const fromDemo = watchdog({ connection: discovering, connect: fromDemoConnect, otherWorkPending: false });
  for (let tick = 1; tick < STALL_TICKS; tick += 1) fromDemo.tick();
  assert.equal(fromDemo.tick(), "return-to-demo");
  const fromIdle = watchdog({ connection: discovering, connect: fromIdleConnect, otherWorkPending: false });
  for (let tick = 1; tick < STALL_TICKS; tick += 1) fromIdle.tick();
  assert.equal(fromIdle.tick(), "needs-attention");
});

test("a connect the phase listener moved on to handshaking is still the same stall", () => {
  const dog = watchdog({ connection: discovering, connect: fromDemoConnect, otherWorkPending: false });
  for (let tick = 0; tick < 10; tick += 1) assert.equal(dog.tick(), null);
  dog.set({ connection: handshaking });
  assert.equal(ticksUntilAbandon(dog), STALL_TICKS - 10);
});

test("a new attempt inside an old stall (Reset Session, menu Reconnect) gets its own full interval", () => {
  const dog = watchdog({ connection: discovering, connect: fromDemoConnect, otherWorkPending: false });
  for (let tick = 0; tick < STALL_TICKS - 2; tick += 1) assert.equal(dog.tick(), null);
  dog.set({ connect: { attempt: 2, fromDemo: false } });
  assert.equal(ticksUntilAbandon(dog), STALL_TICKS);
});

test("a connect that answers in time is never abandoned and leaves no clock behind", () => {
  const dog = watchdog({ connection: discovering, connect: fromDemoConnect, otherWorkPending: false });
  for (let tick = 0; tick < STALL_TICKS - 2; tick += 1) assert.equal(dog.tick(), null);
  dog.set({ connect: null, connection: liveDevice });
  assert.equal(dog.tick(), null);
  dog.set({ connect: fromDemoConnect, connection: discovering });
  assert.equal(ticksUntilAbandon(dog), STALL_TICKS);
});

test("a connect the user superseded (Stop demo, Disconnect) is never the watchdog's to end", () => {
  // App passes `connect: null` once the waiting attempt is no longer current,
  // even though its awaitingDetection flag may still be set.
  for (const connection of [demoStopped, userDisconnected, demoFallback]) {
    const dog = watchdog({ connection, connect: null, otherWorkPending: false });
    assert.equal(ticksUntilAbandon(dog, 500), Infinity, connection.detail);
  }
});

test("an orphaned discovering state repeats its fallback once nothing owns it", () => {
  // returnToDemoDevice showed it, and startDemoDevice refused while a command ran.
  const dog = watchdog({ connection: returning, connect: null, otherWorkPending: true });
  for (let tick = 0; tick < 100; tick += 1) assert.equal(dog.tick(), null, `pending ${tick}`);
  dog.set({ otherWorkPending: false });
  assert.equal(ticksUntilAbandon(dog), QC_ORPHANED_DISCOVERING_MS / POLL_MS + 1);
  // Refused again: retried no sooner than another full interval.
  let tick = 0;
  let abandon: ReturnType<typeof dog.tick> = null;
  while (!abandon) { tick += 1; abandon = dog.tick(); }
  assert.equal(tick, QC_ORPHANED_DISCOVERING_MS / POLL_MS + 1);
  assert.equal(abandon, "retry-fallback");
});

test("a fallback after a long connect gets its own interval, not the connect's", () => {
  const dog = watchdog({ connection: discovering, connect: fromDemoConnect, otherWorkPending: false });
  for (let tick = 0; tick < STALL_TICKS - 2; tick += 1) dog.tick();
  // The connect failed from the demo: returnToDemoDevice, with the demo's open
  // not yet visible as pending in this render.
  dog.set({ connect: null, connection: returning });
  assert.equal(dog.tick(), null);
  assert.equal(ticksUntilAbandon(dog), QC_ORPHANED_DISCOVERING_MS / POLL_MS);
});

test("the watchdog leaves every other state alone, a user's Disconnect included", () => {
  for (const connection of [demoFallback, liveDevice, failedConnect, failedDisconnect, userDisconnected, demoStopped, handshaking]) {
    const dog = watchdog({ connection, connect: null, otherWorkPending: false });
    assert.equal(ticksUntilAbandon(dog, 500), Infinity, `${connection.source} ${connection.phase}`);
  }
});
