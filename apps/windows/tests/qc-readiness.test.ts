import assert from "node:assert/strict";
import test from "node:test";
import { brokerPhaseDetectsDevice, normalizeBrokerConnection, QC_READY_INVENTORY_REFOLLOWS, QC_REFOLLOW_RESTORE_AFTER_LIVE_MS, qcConnectionDuringBrokerPhase, qcFollowReadyInventory, qcInventoryDeviceReady } from "../src/qc-readiness.ts";

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
    userConnect() { carried = { ...carried, refollows: 0, liveSessionLost: false }; }
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
