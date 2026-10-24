import assert from "node:assert/strict";
import test from "node:test";
import { beginBrowserPairingOnPanelOpen, browserPairingPollFailureMessage, createBrowserPairingSession, startBrowserPairingLifecycle } from "../src/browser-pairing-session.ts";

const settle = async () => { for (let i = 0; i < 12; i += 1) await Promise.resolve(); };

function fakeClock() {
  let time = 1_000;
  let sequence = 0;
  const timers = new Map<number, { at: number; run: () => void }>();
  return {
    now: () => time,
    schedule: (run: () => void, delay: number) => { const id = ++sequence; timers.set(id, { at: time + delay, run }); return id; },
    cancel: (id: unknown) => { timers.delete(id as number); },
    pending: () => timers.size,
    async advance(delay: number) {
      const end = time + delay;
      let iterations = 0;
      while (true) {
        const next = [...timers].filter(([, timer]) => timer.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
        if (!next) break;
        assert.ok(++iterations < 100, "a terminal flow must not keep scheduling polls");
        time = next[1].at;
        timers.delete(next[0]);
        next[1].run();
        await settle();
      }
      time = end;
      await settle();
    }
  };
}

test("a paired or unknown status-panel open never begins or polls a new session", async () => {
  const clock = fakeClock();
  let begins = 0;
  let polls = 0;
  const begin = () => {
    begins += 1;
    startBrowserPairingLifecycle({ expiresAt: clock.now() + 10_000,
      poll: async () => { polls += 1; return { state: "waiting" }; },
      onPaired() {}, onExpired() {}, onFailure() {}
    }, clock);
  };
  beginBrowserPairingOnPanelOpen(true, false, false, begin);
  beginBrowserPairingOnPanelOpen(undefined, false, false, begin);
  await clock.advance(60 * 60 * 1_000);
  assert.equal(begins, 0);
  assert.equal(polls, 0);
  assert.equal(clock.pending(), 0);
});

test("first-time REMOTE open starts at most one offer; existing/in-flight offers do not restart", () => {
  let begins = 0;
  const begin = () => { begins += 1; };
  beginBrowserPairingOnPanelOpen(false, false, false, begin);
  beginBrowserPairingOnPanelOpen(false, true, false, begin);
  beginBrowserPairingOnPanelOpen(false, false, true, begin);
  assert.equal(begins, 1);
});

test("successful pairing cancels expiry and polling even if the panel stays open", async () => {
  const clock = fakeClock();
  let begins = 0;
  let polls = 0;
  let paired = 0;
  let expired = 0;
  const session = createBrowserPairingSession({ async begin() { begins += 1; return "offer"; }, async cancel() {} });
  await session.begin();
  startBrowserPairingLifecycle({ expiresAt: clock.now() + 10_000,
    async poll() { return { state: ++polls === 2 ? "paired" : "waiting" }; },
    onPaired() { paired += 1; }, onExpired() { expired += 1; }, onFailure(error) { throw error; }
  }, clock);
  await settle();
  await clock.advance(2_000);
  assert.equal(paired, 1);
  assert.equal(polls, 2);
  assert.equal(clock.pending(), 0);
  beginBrowserPairingOnPanelOpen(true, false, false, () => { void session.begin(); });
  await clock.advance(60 * 60 * 1_000);
  assert.equal(begins, 1);
  assert.equal(polls, 2);
  assert.equal(expired, 0);
});

test("expiry cancels the offer without renewing or polling until an explicit new action", async () => {
  const clock = fakeClock();
  let begins = 0;
  let cancels = 0;
  let polls = 0;
  let expired = 0;
  const session = createBrowserPairingSession({ async begin() { begins += 1; return "offer"; }, async cancel() { cancels += 1; } });
  await session.begin();
  startBrowserPairingLifecycle({ expiresAt: clock.now() + 5_000,
    async poll() { polls += 1; return { state: "waiting" }; },
    onPaired() { assert.fail("unapproved offer"); },
    onExpired() { expired += 1; void session.cancel(); }, onFailure(error) { throw error; }
  }, clock);
  await settle();
  await clock.advance(5_000);
  assert.equal(expired, 1);
  assert.equal(cancels, 1);
  assert.equal(polls, 3);
  assert.equal(clock.pending(), 0);
  await clock.advance(60 * 60 * 1_000);
  assert.equal(polls, 3);
  assert.equal(begins, 1);
  await session.begin(); // Explicit Retry/Pair action, not a timer.
  assert.equal(begins, 2);
});

test("an already-expired offer never sends even an initial poll", async () => {
  const clock = fakeClock();
  let expired = 0;
  let polls = 0;
  startBrowserPairingLifecycle({ expiresAt: clock.now(),
    async poll() { polls += 1; return { state: "waiting" }; },
    onPaired() { assert.fail(); }, onExpired() { expired += 1; }, onFailure(error) { throw error; }
  }, clock);
  await settle();
  await clock.advance(10_000);
  assert.equal(polls, 0);
  assert.equal(expired, 1);
  assert.equal(clock.pending(), 0);
});

for (const end of ["close", "expiry"] as const) {
  test(`${end} rejects a late successful poll and never schedules another one`, async () => {
    const clock = fakeClock();
    let finish!: (result: { state: "paired" }) => void;
    let paired = 0;
    let expired = 0;
    let polls = 0;
    const stop = startBrowserPairingLifecycle({ expiresAt: clock.now() + 5_000,
      poll() { polls += 1; return new Promise((resolve) => { finish = resolve; }); },
      onPaired() { paired += 1; }, onExpired() { expired += 1; }, onFailure(error) { throw error; }
    }, clock);
    await clock.advance(4_000);
    assert.equal(polls, 1, "slow polls cannot overlap");
    if (end === "close") stop(); else await clock.advance(1_000);
    finish({ state: "paired" });
    await settle();
    await clock.advance(60 * 60 * 1_000);
    assert.equal(paired, 0);
    assert.equal(polls, 1);
    assert.equal(expired, end === "expiry" ? 1 : 0);
    assert.equal(clock.pending(), 0);
  });
}

test("closing during a late begin cancels its native offer without starting any poll", async () => {
  const clock = fakeClock();
  let finish!: (offer: string) => void;
  let nativeOffer: string | undefined;
  let polls = 0;
  const session = createBrowserPairingSession({
    async begin() { nativeOffer = await new Promise<string>((resolve) => { finish = resolve; }); return nativeOffer; },
    async cancel() { nativeOffer = undefined; }
  });
  const opening = session.begin();
  await settle();
  const closing = session.cancel();
  finish("late-offer");
  const operation = await opening;
  if (operation.current()) {
    startBrowserPairingLifecycle({ expiresAt: clock.now() + 5_000,
      async poll() { polls += 1; return { state: "waiting" }; },
      onPaired() {}, onExpired() {}, onFailure(error) { throw error; }
    }, clock);
  }
  await closing;
  await clock.advance(60_000);
  assert.equal(operation.current(), false);
  assert.equal(nativeOffer, undefined);
  assert.equal(polls, 0);
  assert.equal(clock.pending(), 0);
});

test("a failed poll is terminal and its safe UI error contains no backend secrets", async () => {
  const clock = fakeClock();
  let polls = 0;
  const errors: string[] = [];
  startBrowserPairingLifecycle({ expiresAt: clock.now() + 10_000,
    async poll() { polls += 1; throw new Error("https://example.invalid/pair?token=do-not-render"); },
    onPaired() { assert.fail(); }, onExpired() { assert.fail(); },
    onFailure(error) { errors.push(browserPairingPollFailureMessage(error)); }
  }, clock);
  await settle();
  await clock.advance(60_000);
  assert.equal(polls, 1);
  assert.deepEqual(errors, ["Could not check the pairing session. Retry to create a new code."]);
  assert.equal(clock.pending(), 0);
});

test("native expiry keeps its actionable message and arbitrary errors stay private", () => {
  const expiry = "The browser pairing session expired. Start it again from QC Remote.";
  assert.equal(browserPairingPollFailureMessage(new Error(expiry)), expiry);
  assert.equal(browserPairingPollFailureMessage("unexpected"), "Could not check the pairing session. Retry to create a new code.");
});

for (const pending of [false, true]) {
  test(`a synchronous close/refresh fence stops ${pending ? "an in-flight" : "a scheduled"} poll before React cleanup`, async () => {
    const clock = fakeClock();
    let current = true;
    let polls = 0;
    let applied = 0;
    let finish!: (result: { state: "paired" }) => void;
    startBrowserPairingLifecycle({ expiresAt: clock.now() + 5_000, isCurrent: () => current,
      async poll() {
        polls += 1;
        if (pending) return await new Promise<{ state: "paired" }>((resolve) => { finish = resolve; });
        return { state: "waiting" };
      },
      onPaired() { applied += 1; }, onExpired() { applied += 1; }, onFailure() { applied += 1; }
    }, clock);
    await settle();
    current = false; // Panel ref/generation/in-flight flag changes synchronously.
    if (pending) finish({ state: "paired" });
    await settle();
    await clock.advance(60_000);
    assert.equal(polls, 1);
    assert.equal(applied, 0);
    assert.equal(clock.pending(), 0);
  });
}

for (const outcome of ["paired", "error"] as const) {
  test(`completed refresh permanently fences an old ${outcome} poll before React cleanup`, async () => {
    const clock = fakeClock();
    let begins = 0;
    const session = createBrowserPairingSession({ async begin() { return `offer-${++begins}`; }, async cancel() {} });
    const firstPending = session.begin();
    const firstCurrent = session.checkpoint();
    const first = await firstPending;
    let inFlight = false;
    let visibleOffer: string | undefined = first.offer;
    let notice = "waiting";
    let resolveOld!: (result: { state: "paired" }) => void;
    let rejectOld!: (error: Error) => void;
    const stopOld = startBrowserPairingLifecycle({ expiresAt: clock.now() + 10_000,
      isCurrent: () => firstCurrent() && !inFlight,
      poll: () => new Promise((resolve, reject) => { resolveOld = resolve; rejectOld = reject; }),
      onPaired() { visibleOffer = undefined; notice = "old paired"; },
      onExpired() { visibleOffer = undefined; notice = "old expired"; },
      onFailure() { visibleOffer = undefined; notice = "old error"; }
    }, clock);
    // Follow the App's actual begin/checkpoint/await order, including the
    // in-flight flag returning to false before old React effect cleanup.
    inFlight = true;
    const refreshPending = session.begin();
    const refreshCurrent = session.checkpoint();
    assert.equal(firstCurrent(), false, "refresh invalidates old poll before native begin completes");
    const refreshed = await refreshPending;
    await settle(); // QR preparation may complete before the old poll does.
    visibleOffer = refreshed.offer;
    notice = "new offer ready";
    inFlight = false;
    assert.equal(refreshed.current(), true);
    assert.equal(refreshCurrent(), true);
    assert.equal(first.current(), false);
    if (outcome === "paired") resolveOld({ state: "paired" }); else rejectOld(new Error("late old error"));
    await settle();
    assert.equal(visibleOffer, "offer-2");
    assert.equal(notice, "new offer ready");
    assert.equal(clock.pending(), 0);
    stopOld();
  });
}

test("begin failure remains current for the App catch checkpoint captured after begin", async () => {
  const session = createBrowserPairingSession({ async begin(): Promise<string> { throw new Error("offline"); }, async cancel() {} });
  const previous = session.checkpoint();
  const pending = session.begin();
  const current = session.checkpoint();
  assert.equal(previous(), false);
  await assert.rejects(pending, /offline/);
  assert.equal(current(), true, "the current begin error must still reach its UI catch");
  await session.cancel();
  assert.equal(current(), false, "close still suppresses a stale catch");
});
