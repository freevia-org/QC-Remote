import assert from "node:assert/strict";
import test from "node:test";
import { demoSnapshot } from "../../qc-core/src/device-reference/demo-snapshot.ts";
import { canAdoptNativeState } from "../../qc-core/src/state.ts";
import { createGridRepairCoordinator, isIncompleteGridRefreshFailure, observeCompleteGrid, refreshGridSnapshot, refreshGridSnapshotOnce, runGridRepairOnce, type GridSnapshotRefreshOptions } from "../src/grid-snapshot-refresh.ts";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function controllerFor(snapshot: typeof demoSnapshot & { gridLayoutComplete: boolean; stateVersion: { session: string; revision: number } }) {
  let current = snapshot;
  return {
    current: () => current,
    observe: (next: typeof current) => { current = next; },
    async readSnapshot(read: () => Promise<typeof current>) {
      const incoming = await read();
      if (canAdoptNativeState(current.stateVersion, incoming.stateVersion)) current = incoming;
      return current;
    }
  };
}

function refreshOptions(
  controller: ReturnType<typeof controllerFor>,
  sequence: { current: number },
  read: () => Promise<ReturnType<typeof controller.current>>,
  isCurrent: () => boolean,
  pending: { value: boolean },
  errors: unknown[]
): GridSnapshotRefreshOptions {
  return {
    sequence,
    readSnapshot: controller.readSnapshot,
    read,
    isCurrent,
    onPending: () => { pending.value = true; },
    onError: (error) => errors.push(error),
    onFinished: () => { pending.value = false; }
  };
}

test("a delayed complete Grid snapshot cannot replace a newer sparse invalidation", async () => {
  const controller = controllerFor({
    ...demoSnapshot,
    gridLayoutComplete: true,
    stateVersion: { session: "session-a", revision: 10 }
  });
  const pending = { value: false };
  const errors: unknown[] = [];
  const sequence = { current: 0 };
  const delayed = deferred<ReturnType<typeof controller.current>>();
  const refresh = refreshGridSnapshot(refreshOptions(controller, sequence, () => delayed.promise, () => true, pending, errors));

  controller.observe({
    ...controller.current(),
    gridLayoutComplete: false,
    stateVersion: { session: "session-a", revision: 12 }
  });
  const staleCompleteSnapshot = await Promise.resolve({
    ...demoSnapshot,
    gridLayoutComplete: true,
    stateVersion: { session: "session-a", revision: 11 }
  });
  delayed.resolve(staleCompleteSnapshot);
  await refresh;

  assert.equal(controller.current().gridLayoutComplete, false, "controller adoption rejects its older native revision");
  assert.equal(controller.current().stateVersion.revision, 12);
  assert.equal(errors.length, 1, "a stale completion is surfaced as a failed reconciliation without adoption");
  assert.equal(pending.value, false);
});

test("a current complete publication repairs an incomplete Grid through guarded adoption", async () => {
  const controller = controllerFor({
    ...demoSnapshot,
    gridLayoutComplete: false,
    stateVersion: { session: "session-a", revision: 12 }
  });
  const pending = { value: false };
  const errors: unknown[] = [];
  const current = {
    ...demoSnapshot,
    gridLayoutComplete: true,
    stateVersion: { session: "session-a", revision: 13 }
  };
  await refreshGridSnapshot(refreshOptions(controller, { current: 0 }, async () => current, () => true, pending, errors));
  assert.equal(controller.current().gridLayoutComplete, true);
  assert.equal(controller.current().stateVersion.revision, 13);
  assert.deepEqual(errors, []);
  assert.equal(pending.value, false);
});

test("a session switch blocks adoption and suppresses the stale refresh error", async () => {
  const controller = controllerFor({
    ...demoSnapshot,
    gridLayoutComplete: false,
    stateVersion: { session: "session-a", revision: 12 }
  });
  const pending = { value: false };
  const errors: unknown[] = [];
  const delayed = deferred<ReturnType<typeof controller.current>>();
  let currentAttempt = true;
  const refresh = refreshGridSnapshot(refreshOptions(controller, { current: 0 }, () => delayed.promise, () => currentAttempt, pending, errors));
  currentAttempt = false;
  delayed.resolve({ ...demoSnapshot, gridLayoutComplete: true, stateVersion: { session: "session-a", revision: 13 } });
  await refresh;
  assert.equal(controller.current().gridLayoutComplete, false);
  assert.deepEqual(errors, []);
  assert.equal(pending.value, false);
});

test("an older completion cannot clear the pending state of a newer refresh", async () => {
  const controller = controllerFor({
    ...demoSnapshot,
    gridLayoutComplete: false,
    stateVersion: { session: "session-a", revision: 12 }
  });
  const pending = { value: false };
  const errors: unknown[] = [];
  const sequence = { current: 0 };
  const older = deferred<ReturnType<typeof controller.current>>();
  const newer = deferred<ReturnType<typeof controller.current>>();
  const first = refreshGridSnapshot(refreshOptions(controller, sequence, () => older.promise, () => true, pending, errors));
  const second = refreshGridSnapshot(refreshOptions(controller, sequence, () => newer.promise, () => true, pending, errors));

  older.resolve({ ...demoSnapshot, gridLayoutComplete: true, stateVersion: { session: "session-a", revision: 13 } });
  await first;
  assert.equal(pending.value, true);
  newer.resolve({ ...demoSnapshot, gridLayoutComplete: true, stateVersion: { session: "session-a", revision: 14 } });
  await second;
  assert.equal(pending.value, false);
  assert.deepEqual(errors, []);
});

test("event and recovery paths share one Grid episode latch; only topology or producer changes rearm it", async () => {
  let reads = 0;
  const coordinator = createGridRepairCoordinator();
  const runAttempt = (stream: string | undefined, invalidationSequence?: number, error: unknown = new Error("temporary transport failure")) =>
    refreshGridSnapshotOnce(coordinator, stream, invalidationSequence, {
      sequence: {current: 0},
      readSnapshot: async (read) => read(),
      read: async () => {
        reads += 1;
        if (error) throw error;
        return {...demoSnapshot, gridLayoutComplete: true};
      },
      isCurrent: () => true,
      onPending: () => undefined,
      onError: () => undefined,
      onFinished: () => undefined
    } satisfies GridSnapshotRefreshOptions);

  await runAttempt("producer-a", 14);
  // These represent unrelated CPU/screen/tempo event tails. They do not alter
  // the active topology identity, so timer ticks issue no new READ.
  for (const tail of [15, 16, 17]) {
    void tail;
    await runAttempt("producer-a", 14);
  }
  assert.equal(reads, 1);

  // A new sparse event, producer, or recovery scope is new evidence.
  await runAttempt("producer-a", 18);
  assert.equal(reads, 2);
  await runAttempt("producer-b", 18);
  assert.equal(reads, 3);
  // An admitted Grid repair consumes its attempt even when failure is
  // ambiguous; a complete-cache recovery remains retryable on transient I/O.
  await runAttempt("producer-b", 18);
  assert.equal(reads, 3);
  for (let tick = 0; tick < 2; tick += 1) {
    await refreshGridSnapshot({
      sequence: {current: 0},
      readSnapshot: async (read) => read(),
      read: async () => { reads += 1; throw new Error("temporary transport failure"); },
      isCurrent: () => true,
      onPending: () => undefined,
      onError: () => undefined,
      onFinished: () => undefined
    });
  }
  assert.equal(reads, 5);

  observeCompleteGrid(coordinator);
  assert.equal(coordinator.attempted, false, "a complete read clears the shared episode latch");
  assert.equal(await refreshGridSnapshotOnce(coordinator, "producer-b", 18, {
    sequence: {current: 0},
    readSnapshot: async (read) => read(),
    read: async () => { reads += 1; return {...demoSnapshot, gridLayoutComplete: true}; },
    isCurrent: () => true,
    onPending() {}, onError() {}, onFinished() {}
  }), undefined);
  assert.equal(reads, 5, "replaying the completed topology event does not issue another read");
});

test("a new producer with the same sequence waits for the old repair, then remains eligible", async () => {
  const coordinator = createGridRepairCoordinator();
  let resolveOld!: (value: string) => void;
  let reads = 0;
  const old = runGridRepairOnce(coordinator, "producer-a", 2, () => new Promise<string>((resolve) => { resolveOld = resolve; }));
  await Promise.resolve();
  assert.equal(coordinator.inFlight !== undefined, true);
  assert.equal(await runGridRepairOnce(coordinator, "producer-b", 2, async () => { reads += 1; return "B"; }), undefined);
  resolveOld("A");
  assert.equal(await old, "A");
  assert.equal(coordinator.attempted, false, "the suppressed replacement episode was not consumed by A");
  assert.equal(await runGridRepairOnce(coordinator, "producer-b", 2, async () => { reads += 1; return "B"; }), "B");
  assert.equal(reads, 1);
});

test("a hashless unknown episode cannot complete a topology event that arrived while its read was pending", async () => {
  const coordinator = createGridRepairCoordinator();
  let resolveOld!: (value: typeof demoSnapshot) => void;
  let reads = 0;
  const options = (read: () => Promise<typeof demoSnapshot>) => ({
    sequence: {current: 0}, readSnapshot: async (run: () => Promise<typeof demoSnapshot>) => run(),
    read, isCurrent: () => true, onPending() {}, onError() {}, onFinished() {}
  } satisfies GridSnapshotRefreshOptions);
  const old = refreshGridSnapshotOnce(coordinator, "producer-a", undefined, options(() => new Promise((resolve) => { resolveOld = resolve; })));
  await Promise.resolve();
  await refreshGridSnapshotOnce(coordinator, "producer-a", 8, options(async () => { reads += 1; return {...demoSnapshot, gridLayoutComplete: true}; }));
  resolveOld({...demoSnapshot, gridLayoutComplete: true});
  await old;
  assert.equal(coordinator.completedInvalidationSequence, undefined, "the unknown read did not complete the later event identity");
  assert.equal(coordinator.attempted, false, "the suppressed event remains eligible after the old read exits");
  await refreshGridSnapshotOnce(coordinator, "producer-a", 8, options(async () => { reads += 1; return {...demoSnapshot, gridLayoutComplete: true}; }));
  assert.equal(reads, 1);
});

test("an incomplete current-preset refusal is identified so fallback recovery stops", () => {
  assert.equal(isIncompleteGridRefreshFailure(new Error("STATE_UNAVAILABLE: Grid layout remains incomplete")), true);
  assert.equal(isIncompleteGridRefreshFailure("The Quad Cortex did not publish a new complete Grid; layout remains incomplete."), true);
  assert.equal(isIncompleteGridRefreshFailure(new Error("Quad Cortex is disconnected")), false);
});
