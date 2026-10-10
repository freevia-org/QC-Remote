import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { demoSnapshot } from "../../qc-core/src/device-reference/demo-snapshot.ts";
import { createGridRepairCoordinator, refreshGridSnapshotOnce } from "../src/grid-snapshot-refresh.ts";
import type { NativeStateRecoveryOptions } from "../src/use-native-state-recovery.ts";

// Load the hook only after the test loader shares React with react-test-renderer.
register("./support/react-singleton-loader.mjs", import.meta.url);
const {createElement} = (await import("react")).default;
const {useNativeStateRecovery} = await import("../src/use-native-state-recovery.ts");

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function recoveryController(gridLayoutComplete = false) {
  const snapshotRef = {current: {...demoSnapshot, gridLayoutComplete, stateVersion: {session: "session-a", revision: 4}}};
  return {
    snapshotRef,
    get snapshot() { return snapshotRef.current; },
    lifetime: {checkpoint: () => 1, current: () => true},
    updateSnapshot(transform: (value: typeof snapshotRef.current) => typeof snapshotRef.current) {
      snapshotRef.current = transform(snapshotRef.current);
    },
    resetCommands() {},
    async readSnapshot(read: () => Promise<typeof snapshotRef.current>) {
      const next = await read();
      snapshotRef.current = next;
      return next;
    }
  };
}

test("the mounted recovery hook shares the event latch and scans past unrelated journal rows", async () => {
  (globalThis as typeof globalThis & {IS_REACT_ACT_ENVIRONMENT?: boolean}).IS_REACT_ACT_ENVIRONMENT = true;
  const gridRepair = {current: createGridRepairCoordinator()};
  let deviceReads = 0;
  await refreshGridSnapshotOnce(gridRepair.current, "producer-a", 2, {
    sequence: {current: 0},
    readSnapshot: async (read) => read(),
    read: async () => { deviceReads += 1; throw new Error("STATE_UNAVAILABLE: Grid layout remains incomplete"); },
    isCurrent: () => true,
    onPending() {},
    onError() {},
    onFinished() {}
  });
  assert.equal(deviceReads, 1, "the event-triggered attempt was admitted");

  const controller = recoveryController();
  let producerB = false;
  let pageCalls = 0;
  const options: NativeStateRecoveryOptions = {
    enabled: true,
    controller: controller as unknown as NativeStateRecoveryOptions["controller"],
    sequence: {current: 0},
    streamId: {current: undefined},
    recoveryRevision: {current: 0},
    gridRepair,
    intervalMs: 10,
    readEvents: async (afterSequence) => {
      pageCalls += 1;
      if (producerB) return {native: true, streamId: "producer-b", latestSequence: 1, frames: [{sequence: 1, states: [{kind: "cpuLoad", cpuTotalLoad: 0.2}]}]};
      if (afterSequence < 1) return {native: true, streamId: "producer-a", latestSequence: 2, frames: [{sequence: 1, states: [{kind: "cpuLoad", cpuTotalLoad: 0.1}]}]};
      if (afterSequence < 2) return {native: true, streamId: "producer-a", latestSequence: 2, frames: [{sequence: 2, states: [{kind: "gridTopology", gridLayoutComplete: false}]}]};
      return {native: true, streamId: "producer-a", latestSequence: 2, frames: []};
    },
    readSnapshot: async () => ({...demoSnapshot, gridLayoutComplete: false, stateVersion: {session: "session-a", revision: 4}}),
    readGridRepair: async () => {
      deviceReads += 1;
      return {...demoSnapshot, gridLayoutComplete: true, stateVersion: {session: "session-a", revision: 5}};
    }
  };
  function Recovery() {
    useNativeStateRecovery(options);
    return null;
  }

  let renderer: ReactTestRenderer | undefined;
  try {
    await act(async () => {
      renderer = create(createElement(Recovery));
    });
    await wait(45);
    assert.ok(pageCalls >= 3, "timer scan advanced from the unrelated first row to the dropped topology row");
    assert.equal(deviceReads, 1, "the dropped journal event did not duplicate the already admitted event repair");

    producerB = true;
    await wait(35);
    assert.equal(deviceReads, 2, "a replacement producer starts a new repair episode");
    assert.equal(controller.snapshotRef.current.gridLayoutComplete, true);
  } finally {
    await act(async () => { renderer?.unmount(); });
  }
});

test("a backend Grid refusal latches an evicted invalidation while ordinary transport errors remain retryable", async () => {
  (globalThis as typeof globalThis & {IS_REACT_ACT_ENVIRONMENT?: boolean}).IS_REACT_ACT_ENVIRONMENT = true;
  const controller = recoveryController(true);
  const gridRepair = {current: createGridRepairCoordinator()};
  let gridReads = 0;
  const gridOptions: NativeStateRecoveryOptions = {
    enabled: true,
    controller: controller as unknown as NativeStateRecoveryOptions["controller"],
    sequence: {current: 0}, streamId: {current: undefined}, recoveryRevision: {current: 0}, gridRepair,
    intervalMs: 8,
    readEvents: async () => ({native: true, streamId: "producer-a", latestSequence: 0, frames: []}),
    readSnapshot: async () => ({...demoSnapshot, gridLayoutComplete: false, stateVersion: {session: "session-a", revision: 5}}),
    readGridRepair: async () => { gridReads += 1; throw new Error("STATE_UNAVAILABLE: Grid layout remains incomplete"); }
  };
  function Recovery() { useNativeStateRecovery(gridOptions); return null; }
  let renderer: ReactTestRenderer | undefined;
  try {
    await act(async () => { renderer = create(createElement(Recovery)); });
    await wait(35);
    assert.equal(gridReads, 1, "the backend's typed Grid refusal consumes the unknown invalidation attempt");
    assert.equal(controller.snapshotRef.current.gridLayoutComplete, false);
  } finally {
    await act(async () => { renderer?.unmount(); });
  }

  const transientController = recoveryController(true);
  const transientRepair = {current: createGridRepairCoordinator()};
  let transientReads = 0;
  const transientOptions: NativeStateRecoveryOptions = {
    ...gridOptions,
    controller: transientController as unknown as NativeStateRecoveryOptions["controller"],
    gridRepair: transientRepair,
    readSnapshot: async () => {
      transientReads += 1;
      if (transientReads < 3) throw new Error("temporary transport failure");
      return {...demoSnapshot, gridLayoutComplete: true, stateVersion: {session: "session-a", revision: 5}};
    },
    readGridRepair: async () => { throw new Error("unexpected explicit Grid repair"); }
  };
  function TransientRecovery() { useNativeStateRecovery(transientOptions); return null; }
  renderer = undefined;
  try {
    await act(async () => { renderer = create(createElement(TransientRecovery)); });
    await wait(55);
    assert.ok(transientReads >= 3, "ordinary non-Grid recovery errors retain the retry behavior");
    assert.equal(transientController.snapshotRef.current.gridLayoutComplete, true);
  } finally {
    await act(async () => { renderer?.unmount(); });
  }
});

test("a timer tick does not reset the controller while an event repair owns the shared read", async () => {
  (globalThis as typeof globalThis & {IS_REACT_ACT_ENVIRONMENT?: boolean}).IS_REACT_ACT_ENVIRONMENT = true;
  const coordinator = createGridRepairCoordinator();
  const controller = recoveryController(true);
  let resets = 0;
  controller.resetCommands = () => { resets += 1; };
  let resolveRead!: (snapshot: typeof controller.snapshot) => void;
  const eventRepair = refreshGridSnapshotOnce(coordinator, "producer-a", 4, {
    sequence: {current: 0},
    readSnapshot: async (read) => read(),
    read: () => new Promise<typeof controller.snapshot>((resolve) => { resolveRead = resolve; }),
    isCurrent: () => true,
    onPending() {}, onError() {}, onFinished() {}
  });
  await Promise.resolve();
  assert.equal(coordinator.inFlight !== undefined, true);

  const options: NativeStateRecoveryOptions = {
    enabled: true,
    controller: controller as unknown as NativeStateRecoveryOptions["controller"],
    sequence: {current: 0}, streamId: {current: "producer-a"}, recoveryRevision: {current: 0},
    gridRepair: {current: coordinator}, intervalMs: 8,
    readEvents: async () => ({native: true, streamId: "producer-a", latestSequence: 0, frames: []}),
    readSnapshot: async () => controller.snapshot,
    readGridRepair: async () => { throw new Error("timer must not start a second read"); }
  };
  function Recovery() { useNativeStateRecovery(options); return null; }
  let renderer: ReactTestRenderer | undefined;
  try {
    await act(async () => { renderer = create(createElement(Recovery)); });
    await wait(25);
    assert.equal(resets, 0, "recovery did not revoke the in-flight event's controller lifetime");
    resolveRead({...demoSnapshot, gridLayoutComplete: true});
    await eventRepair;
    assert.equal(resets, 0);
  } finally {
    await act(async () => { renderer?.unmount(); });
  }
});

test("a rejected repair from an unmounted hook cannot mark the replacement snapshot incomplete", async () => {
  (globalThis as typeof globalThis & {IS_REACT_ACT_ENVIRONMENT?: boolean}).IS_REACT_ACT_ENVIRONMENT = true;
  const controller = recoveryController(true);
  let rejectRepair!: (error: Error) => void;
  let startedRepair!: () => void;
  const repairStarted = new Promise<void>((resolve) => { startedRepair = resolve; });
  const pendingRepair = new Promise<never>((_resolve, reject) => { rejectRepair = reject; });
  const options: NativeStateRecoveryOptions = {
    enabled: true,
    controller: controller as unknown as NativeStateRecoveryOptions["controller"],
    sequence: {current: 0}, streamId: {current: "producer-a"}, recoveryRevision: {current: 0},
    gridRepair: {current: createGridRepairCoordinator()}, intervalMs: 8,
    readEvents: async () => ({native: true, streamId: "producer-a", latestSequence: 1, frames: [{sequence: 1, states: [{kind: "gridTopology", gridLayoutComplete: false}]}]}),
    readSnapshot: async () => controller.snapshot,
    readGridRepair: () => { startedRepair(); return pendingRepair; }
  };
  function Recovery() { useNativeStateRecovery(options); return null; }
  let renderer: ReactTestRenderer | undefined;
  try {
    await act(async () => { renderer = create(createElement(Recovery)); });
    await repairStarted;
    await act(async () => { renderer?.unmount(); });
    renderer = undefined;
    controller.updateSnapshot((snapshot) => ({...snapshot, gridLayoutComplete: true}));
    rejectRepair(new Error("Grid layout remains incomplete after timeout"));
    await wait(5);
    assert.equal(controller.snapshotRef.current.gridLayoutComplete, true, "stale failure did not mutate state after unmount");
  } finally {
    if (renderer) await act(async () => { renderer?.unmount(); });
  }
});
