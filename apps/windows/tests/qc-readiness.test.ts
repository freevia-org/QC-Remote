import assert from "node:assert/strict";
import test from "node:test";
import { brokerPhaseDetectsDevice, normalizeBrokerConnection, QC_CONNECT_STALL_MS, QC_ORPHANED_DISCOVERING_MS, QC_READY_INVENTORY_REFOLLOWS, QC_REFOLLOW_RESTORE_AFTER_LIVE_MS, qcAutomaticConnectHeld, qcConnectionDuringBrokerPhase, qcFollowReadyInventory, qcInventoryDeviceReady, qcPhaseListenerMayFollow, qcRecordLiveSessionLoss, qcWatchConnectStall } from "../src/qc-readiness.ts";

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
const idleWindow = { inventoryReady: true, followed: false, refollows: 0, liveSessionLost: false, liveSince: null, now: 0, userDisconnected: false, connectInFlight: false, connection: demoFallback, demoCanYield: true };

test("a device the broker already has ready is followed from an idle window", () => {
  assert.deepEqual(qcFollowReadyInventory(idleWindow), { connect: true, followed: true, refollows: 0, liveSessionLost: false, liveSince: null });
  assert.deepEqual(qcFollowReadyInventory({ ...idleWindow, connection: failedConnect }), { connect: true, followed: true, refollows: 0, liveSessionLost: false, liveSince: null });
});

test("a window already live with the device does not connect again, and spends the edge", () => {
  assert.deepEqual(qcFollowReadyInventory({ ...idleWindow, connection: liveDevice }), { connect: false, followed: true, refollows: 0, liveSessionLost: false, liveSince: 0 });
});

test("a connect in flight neither fires nor spends the edge, so its miss is followed once it ends", () => {
  const during = qcFollowReadyInventory({ ...idleWindow, connectInFlight: true });
  assert.deepEqual(during, { connect: false, followed: false, refollows: 0, liveSessionLost: false, liveSince: null });
  assert.deepEqual(qcFollowReadyInventory({ ...idleWindow, followed: during.followed }), { connect: true, followed: true, refollows: 0, liveSessionLost: false, liveSince: null });
});

test("a connect in flight after the edge was used stays quiet and keeps it used", () => {
  assert.deepEqual(qcFollowReadyInventory({ ...idleWindow, followed: true, connectInFlight: true }), { connect: false, followed: true, refollows: 0, liveSessionLost: false, liveSince: null });
});

test("a Disconnect that failed is not undone while the device stays ready", () => {
  // Pressed during sync, so no poll ever saw the window live and the edge is
  // unspent; the close failed, so the inventory still reads ready, and
  // "needs-attention" alone would be followed.
  const afterFailedDisconnect = { ...idleWindow, connection: failedDisconnect, userDisconnected: true };
  assert.deepEqual(qcFollowReadyInventory(afterFailedDisconnect), { connect: false, followed: false, refollows: 0, liveSessionLost: false, liveSince: null });
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
  assert.deepEqual(qcFollowReadyInventory({ ...idleWindow, inventoryReady: false }), { connect: false, followed: false, refollows: 0, liveSessionLost: false, liveSince: null });
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

// A window that was live and lost its session to its own live-health check
// (beginLiveRecovery) while the broker stayed ready: no new edge, no push.
// Each poll advances a fake monotonic clock by the real poll's 2 s.
const POLL_MS = 2000;
type Carried = Pick<Parameters<typeof qcFollowReadyInventory>[0], "followed" | "refollows" | "liveSessionLost" | "liveSince">;
function windowPolls() {
  let carried: Carried = { followed: false, refollows: 0, liveSessionLost: false, liveSince: null };
  let now = 0;
  const app = {
    get now() { return now; },
    poll(connection: Connection, inventoryReady = true) {
      now += POLL_MS;
      const step = qcFollowReadyInventory({ ...idleWindow, ...carried, inventoryReady, connection, now });
      carried = { followed: step.followed, refollows: step.refollows, liveSessionLost: step.liveSessionLost, liveSince: step.liveSince };
      return step.connect;
    },
    // Polls that all find the window live for about `ms`.
    stayLive(ms: number) { for (let waited = 0; waited < ms; waited += POLL_MS) app.poll(liveDevice); },
    // Time passing with no decision at all: every poll threw.
    pollsRefused(ms: number) { now += ms; },
    // What beginLiveRecovery records before it returns to the demo.
    loseLiveSession() { carried = { ...carried, liveSessionLost: true, liveSince: null }; },
    // What the user's own Connect clears in App's connect().
    userConnect() { carried = { ...carried, refollows: 0, liveSessionLost: false }; },
    // A `device.phase` push before any poll answered (qcPhaseListenerMayFollow):
    // the same decision as a poll that finds the device ready, at the same
    // moment, with the user's Disconnect latch as App holds it.
    push(connection: Connection, latched = false) {
      const step = qcFollowReadyInventory({ ...idleWindow, ...carried, inventoryReady: true, connection, now, userDisconnected: latched });
      carried = { followed: step.followed, refollows: step.refollows, liveSessionLost: step.liveSessionLost, liveSince: step.liveSince };
      return step.connect;
    },
    // What App records for a lost session or a pending follow: beginLiveRecovery,
    // a connect the watchdog gave up on, the demo fallback's retry (userAction
    // false), a failed USB-tab resume (true).
    recordLoss(userAction = false) { carried = qcRecordLiveSessionLoss(carried, userAction); },
    get carried() { return carried; }
  };
  return app;
}

test("a live session lost while the broker stays ready is followed once more on the same edge", () => {
  const app = windowPolls();
  assert.equal(app.poll(liveDevice), false);
  app.loseLiveSession();
  assert.equal(app.poll(demoFallback), true);
});

test("a re-follow that fails before going live is not retried", () => {
  const app = windowPolls();
  app.stayLive(QC_REFOLLOW_RESTORE_AFTER_LIVE_MS * 3);
  app.loseLiveSession();
  assert.equal(app.poll(demoFallback), true);
  // Refused or incomplete: back on the demo, or needs-attention, and never
  // live, so no new loss and no give-back, however long it sits there.
  for (let poll = 0; poll < 100; poll += 1) {
    assert.equal(app.poll(poll % 2 ? demoFallback : failedConnect), false, `poll ${poll}`);
  }
});

test("a second loss on the same edge is followed once the session stayed live long enough", () => {
  const app = windowPolls();
  app.poll(liveDevice);
  app.loseLiveSession();
  assert.equal(app.poll(demoFallback), true);
  app.stayLive(QC_REFOLLOW_RESTORE_AFTER_LIVE_MS + POLL_MS);
  app.loseLiveSession();
  assert.equal(app.poll(demoFallback), true);
});

test("a second loss after a short live period (a flapping health check) is not followed", () => {
  const app = windowPolls();
  app.poll(liveDevice);
  app.loseLiveSession();
  assert.equal(app.poll(demoFallback), true);
  app.stayLive(QC_REFOLLOW_RESTORE_AFTER_LIVE_MS - 2 * POLL_MS);
  app.loseLiveSession();
  assert.equal(app.poll(demoFallback), false);
  // Live long enough only in total, across a drop: the drop restarts it.
  app.stayLive(QC_REFOLLOW_RESTORE_AFTER_LIVE_MS / 2);
  app.loseLiveSession();
  assert.equal(app.poll(demoFallback), false);
});

test("a health check that keeps failing gets at most two connects per edge, then one per restore interval", () => {
  assert.equal(QC_READY_INVENTORY_REFOLLOWS, 1);
  // One cycle: the session is lost, the window polls on the demo beside a
  // ready broker, and any connect reaches live for `liveMs` before the next loss.
  const run = (app: ReturnType<typeof windowPolls>, liveMs: number, cycles: number) => {
    const connectedAt: number[] = [];
    for (let cycle = 0; cycle < cycles; cycle += 1) {
      app.loseLiveSession();
      for (let poll = 0; poll < 3; poll += 1) {
        if (!app.poll(demoFallback)) continue;
        connectedAt.push(app.now);
        app.stayLive(liveMs);
        break;
      }
    }
    return connectedAt;
  };
  for (const liveMs of [0, POLL_MS, 10_000, QC_REFOLLOW_RESTORE_AFTER_LIVE_MS - POLL_MS]) {
    // Failing sooner than the restore interval: never more than the budget.
    const spent = windowPolls();
    spent.poll(liveDevice);
    assert.equal(run(spent, liveMs, 50).length, QC_READY_INVENTORY_REFOLLOWS, `spent edge, live ${liveMs} ms`);
    assert.equal(run(windowPolls(), liveMs, 50).length, 1 + QC_READY_INVENTORY_REFOLLOWS, `unspent edge, live ${liveMs} ms`);
  }
  for (const liveMs of [QC_REFOLLOW_RESTORE_AFTER_LIVE_MS + POLL_MS, QC_REFOLLOW_RESTORE_AFTER_LIVE_MS * 2]) {
    // Failing later than that: followed every time, but never two re-follows
    // closer together than the restore interval.
    const spent = windowPolls();
    spent.poll(liveDevice);
    const connectedAt = run(spent, liveMs, 50);
    assert.equal(connectedAt.length, 50, `live ${liveMs} ms`);
    for (let index = 1; index < connectedAt.length; index += 1) {
      assert.ok(connectedAt[index] - connectedAt[index - 1] >= QC_REFOLLOW_RESTORE_AFTER_LIVE_MS, `live ${liveMs} ms, connect ${index}`);
    }
  }
});

test("only a new edge, the user's Connect or a long live session gives the re-follow back", () => {
  const app = windowPolls();
  const loseAndPoll = () => { app.poll(liveDevice); app.loseLiveSession(); return app.poll(demoFallback); };
  assert.equal(loseAndPoll(), true);
  assert.equal(loseAndPoll(), false);
  // The device leaves ready and returns: a new edge, followed, and a fresh
  // re-follow with it.
  assert.equal(app.poll(demoFallback, false), false);
  assert.equal(app.poll(demoFallback), true);
  assert.equal(loseAndPoll(), true);
  assert.equal(loseAndPoll(), false);
  // The user's Connect hands back exactly one more.
  app.userConnect();
  assert.equal(loseAndPoll(), true);
  assert.equal(loseAndPoll(), false);
});

test("a recorded loss restarts the live measurement, even when another path heals it unseen", () => {
  // The function only sees `liveSince`; App clears it with every recorded
  // loss (beginLiveRecovery). Time live before that loss earns nothing.
  const app = windowPolls();
  app.poll(liveDevice);
  app.loseLiveSession();
  assert.equal(app.poll(demoFallback), true);
  app.stayLive(QC_REFOLLOW_RESTORE_AFTER_LIVE_MS - 10_000);
  // Lost and healed (by the phase listener, say) between two polls.
  app.loseLiveSession();
  app.stayLive(20_000);
  app.loseLiveSession();
  assert.equal(app.poll(demoFallback), false);
});

test("a gap with live polls on both sides and no recorded loss counts as live time", () => {
  // Documents what continuity means here: only polls and recorded losses can
  // break it, so refused polls between two live ones do not.
  const app = windowPolls();
  app.poll(liveDevice);
  app.loseLiveSession();
  assert.equal(app.poll(demoFallback), true);
  app.poll(liveDevice);
  app.pollsRefused(QC_REFOLLOW_RESTORE_AFTER_LIVE_MS);
  app.poll(liveDevice);
  app.loseLiveSession();
  assert.equal(app.poll(demoFallback), true);
});

test("the inventory leaving ready while the window is live drops its live measurement", () => {
  const step = qcFollowReadyInventory({ ...idleWindow, inventoryReady: false, connection: liveDevice, followed: true, refollows: 1, liveSince: 0, now: 10_000 });
  assert.deepEqual(step, { connect: false, followed: false, refollows: 0, liveSessionLost: false, liveSince: null });
});

test("a clock that goes backwards only delays the give-back", () => {
  const spent = { ...idleWindow, followed: true, refollows: 1, connection: liveDevice, liveSince: 50_000 };
  assert.equal(qcFollowReadyInventory({ ...spent, now: 1_000 }).refollows, 1);
  assert.equal(qcFollowReadyInventory({ ...spent, now: 50_000 + QC_REFOLLOW_RESTORE_AFTER_LIVE_MS - 1 }).refollows, 1);
  assert.equal(qcFollowReadyInventory({ ...spent, now: 50_000 + QC_REFOLLOW_RESTORE_AFTER_LIVE_MS }).refollows, 0);
});

test("a loss recorded before a tray release is followed once the polls resume", () => {
  // The release itself is not visible here: `broker.devices` is refused, the
  // poll throws, and this function is never called. Modelled as time passing
  // with no decision; the explicit reconnect that ends the release (from the
  // paired relay, say) leaves the broker ready and the window on the demo.
  const app = windowPolls();
  app.poll(liveDevice);
  app.loseLiveSession();
  app.pollsRefused(5 * 60_000);
  assert.equal(app.poll(demoFallback), true);
  assert.equal(app.poll(demoFallback), false);
});

test("a lost live session still yields to a Disconnect, a Demo choice and a connect in flight", () => {
  const lost = { ...idleWindow, followed: true, liveSessionLost: true };
  const held = { connect: false, followed: true, refollows: 0, liveSessionLost: true, liveSince: null };
  assert.deepEqual(qcFollowReadyInventory({ ...lost, userDisconnected: true }), held);
  assert.deepEqual(qcFollowReadyInventory({ ...lost, demoCanYield: false }), held);
  assert.deepEqual(qcFollowReadyInventory({ ...lost, connection: userDisconnected }), held);
  // A connect in flight (the phase listener's, say) keeps the loss pending
  // and leaves the re-follow unspent, so a miss is followed once it ends; the
  // window is not live meanwhile, so a live measurement starts over.
  assert.deepEqual(qcFollowReadyInventory({ ...lost, connectInFlight: true, liveSince: -QC_REFOLLOW_RESTORE_AFTER_LIVE_MS }), held);
  assert.deepEqual(qcFollowReadyInventory(lost), { connect: true, followed: true, refollows: 1, liveSessionLost: false, liveSince: null });
});

test("a lost live session that something else brought back live is answered, not followed", () => {
  // The phase listener's connect reached live before a poll saw the demo.
  assert.deepEqual(
    qcFollowReadyInventory({ ...idleWindow, followed: true, refollows: 1, liveSessionLost: true, connection: liveDevice, now: 5000 }),
    { connect: false, followed: true, refollows: 1, liveSessionLost: false, liveSince: 5000 }
  );
});

// Who decides an automatic follow. The poll, from what `broker.devices`
// answered; a `device.phase` push only while no poll has ever answered
// (qcPhaseListenerMayFollow), through the same bounded, latch-aware decision;
// the demo fallback's retry never, it only records a pending follow.

test("a push decides by itself only until the first inventory poll answers", () => {
  assert.equal(qcPhaseListenerMayFollow(false), true);
  assert.equal(qcPhaseListenerMayFollow(true), false);
});

test("a detection push (before any poll answered) follows an unspent edge exactly once", () => {
  const app = windowPolls();
  assert.equal(app.push(demoFallback), true);
  for (let push = 0; push < 20; push += 1) assert.equal(app.push(demoFallback), false, `push ${push}`);
  assert.equal(app.poll(demoFallback), false);
});

test("a detection push never undoes the user's Disconnect, a failed one included", () => {
  for (const connection of [failedDisconnect, userDisconnected, demoFallback]) {
    const app = windowPolls();
    for (let push = 0; push < 10; push += 1) assert.equal(app.push(connection, true), false, `${connection.phase} push ${push}`);
    // The latch holds it without spending anything.
    assert.equal(app.carried.followed, false);
  }
  const lost = windowPolls();
  lost.poll(liveDevice);
  lost.recordLoss();
  assert.equal(lost.push(demoFallback, true), false);
  assert.equal(lost.carried.liveSessionLost, true);
});

test("pushes spend the re-follow budget the poll spends", () => {
  const app = windowPolls();
  app.poll(liveDevice);
  app.recordLoss();
  assert.equal(app.push(demoFallback), true);
  app.poll(liveDevice);
  app.recordLoss();
  assert.equal(app.push(demoFallback), false);
  assert.equal(app.poll(demoFallback), false);
  // A push followed the loss, so the poll does not follow it again.
  const first = windowPolls();
  first.poll(liveDevice);
  first.recordLoss();
  assert.equal(first.push(demoFallback), true);
  assert.equal(first.poll(demoFallback), false);
});

test("a push on a spent edge is followed by a poll only after one saw the device leave ready", () => {
  const app = windowPolls();
  assert.equal(app.poll(demoFallback), true);
  // Spent and no loss pending: neither the push nor the poll follows.
  assert.equal(app.push(demoFallback), false);
  assert.equal(app.poll(demoFallback), false);
  // A poll that reads the device not ready re-arms; the next ready one follows.
  assert.equal(app.poll(demoFallback, false), false);
  assert.equal(app.poll(demoFallback), true);
});

test("the demo retry is a pending follow: the next poll decides it, within the budget", () => {
  // The edge follow failed after a push arrived during it.
  const app = windowPolls();
  assert.equal(app.poll(demoFallback), true);
  app.recordLoss();
  assert.equal(app.poll(demoFallback), true);
  // The same again: the budget is spent, so the retry is not followed.
  app.recordLoss();
  assert.equal(app.poll(demoFallback), false);
  // With the user's Disconnect latched nothing follows it either.
  const latched = windowPolls();
  latched.poll(liveDevice);
  latched.recordLoss();
  assert.equal(qcFollowReadyInventory({ ...idleWindow, ...latched.carried, userDisconnected: true }).connect, false);
});

test("a retry recorded while the polls are refused (tray release, Quit) connects nothing until a poll reads the device ready", () => {
  const app = windowPolls();
  app.poll(liveDevice);
  app.recordLoss();
  // Every poll throws: no decision is made at all, however long.
  app.pollsRefused(10 * 60_000);
  assert.equal(app.carried.liveSessionLost, true);
  // Only a poll that the broker answers with the device ready may follow it.
  assert.equal(app.poll(demoFallback, false), false);
  assert.equal(app.carried.liveSessionLost, false);
  assert.equal(app.poll(demoFallback), true);
});

test("a gave-up reconnect still unsettled holds every automatic follow, as a connect in flight does", () => {
  const idle = { awaitingDetection: false, commandPending: false, demoPending: false, reconnectsOutstanding: 0 };
  assert.equal(qcAutomaticConnectHeld(idle), false);
  assert.equal(qcAutomaticConnectHeld({ ...idle, awaitingDetection: true }), true);
  assert.equal(qcAutomaticConnectHeld({ ...idle, commandPending: true }), true);
  assert.equal(qcAutomaticConnectHeld({ ...idle, demoPending: true }), true);
  assert.equal(qcAutomaticConnectHeld({ ...idle, reconnectsOutstanding: 1 }), true);
  // The loss the watchdog records stays pending through the overlap, for a
  // push and a poll alike, and is followed once the request settles.
  const app = windowPolls();
  assert.equal(app.poll(demoFallback), true);
  app.recordLoss();
  const overlap = { ...idleWindow, ...app.carried, connectInFlight: qcAutomaticConnectHeld({ ...idle, reconnectsOutstanding: 1 }) };
  for (const connection of [demoFallback, failedConnect]) {
    const step = qcFollowReadyInventory({ ...overlap, connection });
    assert.equal(step.connect, false, connection.phase);
    assert.equal(step.liveSessionLost, true, connection.phase);
  }
  assert.equal(app.poll(failedConnect), true);
});

// A failed USB-tab resume (resumeDeviceSession) returns to the demo beside a
// broker that may still hold the device ready, on an edge already spent.

test("a failed USB-tab resume is followed once, even with the budget spent", () => {
  const app = windowPolls();
  app.poll(liveDevice);
  app.recordLoss();
  assert.equal(app.poll(demoFallback), true);
  // Back live, then the user stepped to the demo with the device held.
  app.poll(liveDevice);
  app.poll(demoFallback);
  assert.equal(app.poll(demoFallback), false);
  // Without a record, nothing would follow it.
  assert.equal(app.carried.liveSessionLost, false);
  app.recordLoss(true);
  assert.deepEqual(app.carried, { followed: true, refollows: 0, liveSessionLost: true, liveSince: null });
  assert.equal(app.poll(demoFallback), true);
  assert.equal(app.poll(demoFallback), false);
});

test("a failed resume never lifts the user's Disconnect latch", () => {
  const app = windowPolls();
  app.poll(liveDevice);
  app.recordLoss(true);
  assert.equal(app.push(demoFallback, true), false);
  assert.equal(qcFollowReadyInventory({ ...idleWindow, ...app.carried, userDisconnected: true }).connect, false);
});

test("an automatic loss keeps the budget it had; only the user's choice gives it back", () => {
  const spent = { followed: true, refollows: 1, liveSessionLost: false, liveSince: 5_000 };
  assert.deepEqual(qcRecordLiveSessionLoss(spent), { followed: true, refollows: 1, liveSessionLost: true, liveSince: null });
  assert.deepEqual(qcRecordLiveSessionLoss(spent, true), { followed: true, refollows: 0, liveSessionLost: true, liveSince: null });
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

test("a connect given up by the watchdog is followed once more within the budget, then held", () => {
  // The watchdog itself never connects; App records the abandoned connect as
  // a lost session and the bounded follow decides once the request settled.
  const app = windowPolls();
  assert.equal(app.poll(demoFallback), true);
  app.recordLoss();
  assert.equal(app.poll(failedConnect), true);
  for (let cycle = 0; cycle < 20; cycle += 1) {
    app.recordLoss();
    assert.equal(app.poll(cycle % 2 ? demoFallback : failedConnect), false, `cycle ${cycle}`);
    assert.equal(app.push(demoFallback), false, `cycle ${cycle} push`);
  }
  // A Disconnect after an abandoned connect is still never undone.
  app.userConnect();
  app.recordLoss();
  assert.equal(app.push(demoFallback, true), false);
});
