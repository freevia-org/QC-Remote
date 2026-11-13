import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import type { GatewayTransport } from "@qc-remote/client";
import type { QcDeviceTransport } from "@qc-remote/core";
import { demoSnapshot } from "../../qc-core/src/device-reference/demo-snapshot.ts";
import { createGridRepairCoordinator } from "../src/grid-snapshot-refresh.ts";
import type { UsePresetWorkflowOptions } from "../src/preset-workflow-types.ts";

register("./support/react-singleton-loader.mjs", import.meta.url);
register("./support/tsx-css-loader.mjs", import.meta.url);
const { createElement } = (await import("react")).default;
const { DeviceObservationProvider } = await import("../src/device-observation-policy.ts");
const { useNativeStateRecovery } = await import("../src/use-native-state-recovery.ts");
const { useAdaptiveQcScreenPolling } = await import("../src/use-adaptive-screen-polling.ts");
const { usePresetWorkflow } = await import("../src/use-preset-workflow.ts");
const { useQcController } = await import("../src/use-qc-controller.ts");
const { QcUnitScreenMirror } = await import("../src/qc-unit-screen-mirror.tsx");

test("actual recovery, follow, Directory and screen hooks make zero idle reads in on-demand mode", async t => {
  (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval", "Date"] });
  let calls = 0;
  const read = async () => { calls++; return { ...demoSnapshot }; };
  const gateway = {
    currentSnapshot: read, currentPreset: read,
    async graphicsTree() { calls++; return { tree: "zenUI::Grid" }; },
    async captureScreen() { calls++; return { pngBase64: "" }; },
    async listPresets() { calls++; return { setlistKey: "1_p", presets: [], folders: [] }; },
    async listPresetFolders() { calls++; return { folders: [] }; },
  } as unknown as GatewayTransport;
  let preset!: ReturnType<typeof usePresetWorkflow>;
  let controller!: ReturnType<typeof useQcController>;
  const sequence = { current: 0 }, streamId = { current: undefined as string | undefined };
  const gridRepair = { current: createGridRepairCoordinator() };
  function Probe() {
    controller = useQcController(demoSnapshot);
    useNativeStateRecovery({ enabled: true, controller, sequence, streamId, gridRepair,
      readEvents: async () => { calls++; return { native: true, frames: [] }; }, readSnapshot: read, readGridRepair: read });
    useAdaptiveQcScreenPolling({ enabled: true, connected: true, gateway, onObservation() {} });
    preset = usePresetWorkflow({ gateway, lifetime: controller.lifetime, connected: true, pending: false,
      snapshotRef: controller.snapshotRef, setSnapshot: controller.setSnapshot,
      setPending() {}, prompts: {} as UsePresetWorkflowOptions["prompts"], notice() {}, fail(error) { throw error; } });
    return createElement(QcUnitScreenMirror, { gateway });
  }
  let renderer!: ReactTestRenderer;
  await act(async () => { renderer = create(createElement(DeviceObservationProvider, { value: { mode: "on-demand" } }, createElement(Probe))); });
  try {
    await act(async () => { t.mock.timers.tick(12 * 60 * 60 * 1_000); });
    assert.equal(calls, 0, "foreground idle must not start any gateway reader");
    await act(async () => { t.mock.timers.tick(12 * 60 * 60 * 1_000); });
    assert.equal(calls, 0, "background idle must not create replacement timers");
    await act(async () => { await preset.openDirectory(); });
    assert.equal(preset.directoryOpen, true);
    const afterOpen = calls;
    assert.ok(afterOpen > 0, "explicit Directory open still reads the real gateway");
    await act(async () => { t.mock.timers.tick(24 * 60 * 60 * 1_000); });
    assert.equal(calls, afterOpen, "an open Directory must not revalidate on a timer");
    await act(async () => { await controller.readSnapshot(read); });
    assert.equal(calls, afterOpen + 1, "explicit refresh is a fresh gateway read");
    const button = renderer.root.findAllByType("button").find(button => button.children.includes("Refresh screen"))!;
    await act(async () => { button.props.onClick(); });
    assert.equal(calls, afterOpen + 2, "screen capture requires an explicit refresh");
    await act(async () => { t.mock.timers.tick(24 * 60 * 60 * 1_000); });
    assert.equal(calls, afterOpen + 2, "screen refresh must not arm another capture");
    // Changing a previously live route to on-demand cancels the real hooks'
    // timers too, not just timers created under the new policy.
    await act(async () => { renderer.update(createElement(DeviceObservationProvider, { value: { mode: "live" } }, createElement(Probe))); });
    await act(async () => { await Promise.resolve(); });
    await act(async () => { renderer.update(createElement(DeviceObservationProvider, { value: { mode: "on-demand" } }, createElement(Probe))); });
    const afterPolicyChange = calls;
    await act(async () => { t.mock.timers.tick(86_400_000); });
    assert.equal(calls, afterPolicyChange, "switching modes must cancel previously live readers");
  } finally { await act(async () => renderer.unmount()); }
  const afterUnmount = calls;
  await act(async () => { t.mock.timers.tick(24 * 60 * 60 * 1_000); });
  assert.equal(calls, afterUnmount);
});

test("on-demand controls use fresh planning and immediate readback without recurring retries", async t => {
  (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval", "Date"] });
  let actual = { ...demoSnapshot, tempo: 111 };
  const calls: string[] = [];
  let controller!: ReturnType<typeof useQcController>;
  function Probe() { controller = useQcController(demoSnapshot); return null; }
  const transport = {
    async currentSnapshot() { calls.push("read"); return { ...actual }; },
    async setTempo(tempo: number, expected: typeof actual) {
      calls.push("write");
      assert.equal(expected.tempo, 111, "the guard must use the fresh device observation");
      actual = { ...actual, tempo };
      return {};
    }
  } as unknown as QcDeviceTransport;
  let renderer!: ReactTestRenderer;
  await act(async () => { renderer = create(createElement(DeviceObservationProvider, { value: { mode: "on-demand" } }, createElement(Probe))); });
  try {
    await act(async () => { await controller.runTempo(transport, 123); });
    assert.deepEqual(calls, ["read", "write", "read"]);
    assert.equal(controller.snapshotRef.current.tempo, 123);
    await act(async () => { t.mock.timers.tick(86_400_000); });
    assert.deepEqual(calls, ["read", "write", "read"]);
  } finally { await act(async () => renderer.unmount()); }
});

test("a failed on-demand control gets one direct readback and no idle retry loop", async t => {
  (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval", "Date"] });
  const calls: string[] = [];
  let controller!: ReturnType<typeof useQcController>;
  function Probe() { controller = useQcController(demoSnapshot); return null; }
  const transport = {
    async currentSnapshot() {
      calls.push("read");
      if (calls.length > 1) throw new Error("readback offline");
      return { ...demoSnapshot };
    },
    async setTempo() { calls.push("write"); throw new Error("write outcome unknown"); }
  } as unknown as QcDeviceTransport;
  let renderer!: ReactTestRenderer;
  await act(async () => { renderer = create(createElement(DeviceObservationProvider, { value: { mode: "on-demand" } }, createElement(Probe))); });
  try {
    await act(async () => { await assert.rejects(controller.runTempo(transport, 123), /write outcome unknown/); });
    assert.deepEqual(calls, ["read", "write", "read"]);
    await act(async () => { t.mock.timers.tick(86_400_000); });
    assert.deepEqual(calls, ["read", "write", "read"]);
  } finally { await act(async () => renderer.unmount()); }
});

test("a forced reconciliation read that fails once is retried (TU-33)", async t => {
  (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval", "Date"] });
  let controller!: ReturnType<typeof useQcController>;
  function Probe() { controller = useQcController(demoSnapshot); return null; }
  let reads = 0;
  const releases: Array<() => void> = [];
  const transport = {
    async currentSnapshot() { reads++; if (reads === 1) throw new Error("transient"); return { ...demoSnapshot }; },
    setTempo(tempo: number) { return new Promise((resolve) => { releases.push(() => resolve({})); }); }
  } as unknown as QcDeviceTransport;
  let renderer!: ReactTestRenderer;
  await act(async () => { renderer = create(createElement(Probe)); });
  try {
    let first!: Promise<unknown>, second!: Promise<unknown>;
    await act(async () => { first = controller.runTempo(transport, 120); second = controller.runTempo(transport, 130); });
    await act(async () => { releases[1](); await second; });
    await act(async () => { releases[0](); await first; });
    await act(async () => { t.mock.timers.tick(3_100); });
    assert.equal(reads, 1, "the forced read ran once and failed");
    await act(async () => { t.mock.timers.tick(1_100); });
    assert.ok(reads >= 2, `the superseded late success schedules a forced read and its failure is retried (reads: ${reads})`);
  } finally { await act(async () => renderer.unmount()); }
});
