import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import type { QcStateUpdate } from "@qc-remote/core";
import type { QcCpuTelemetry } from "../src/use-qc-cpu-telemetry.ts";

// Load the hook only after the test loader shares React with react-test-renderer.
register("./support/react-singleton-loader.mjs", import.meta.url);
const { createElement } = (await import("react")).default;
const { useQcCpuTelemetry } = await import("../src/use-qc-cpu-telemetry.ts");

function mount() {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const seen: { current?: QcCpuTelemetry } = {};
  function Probe() { seen.current = useQcCpuTelemetry(); return null; }
  let renderer!: ReactTestRenderer;
  act(() => { renderer = create(createElement(Probe)); });
  return { get value() { return seen.current!; }, renderer };
}
const cpu = (update: Partial<QcStateUpdate>) => ({ kind: "cpuLoad", ...update }) as QcStateUpdate;

test("one telemetry owner tracks the aggregate load and the per-block columns", () => {
  const host = mount();
  assert.equal(host.value.cpuLoad, 0);
  assert.deepEqual(host.value.cpuColumns, []);
  const columns = [{ column: 2, load: 64 }] as unknown as NonNullable<QcStateUpdate["cpuColumns"]>;
  act(() => host.value.consume([cpu({ cpuTotalLoad: 41, cpuColumns: columns })]));
  assert.equal(host.value.cpuLoad, 41);
  assert.deepEqual(host.value.cpuColumns, columns);
  host.renderer.unmount();
});

test("a partial CPU update keeps the other reading, and unrelated states are ignored", () => {
  const host = mount();
  const columns = [{ column: 0, load: 10 }] as unknown as NonNullable<QcStateUpdate["cpuColumns"]>;
  act(() => host.value.consume([cpu({ cpuTotalLoad: 30, cpuColumns: columns })]));
  act(() => host.value.consume([cpu({ cpuTotalLoad: 55 })]));
  assert.equal(host.value.cpuLoad, 55);
  assert.deepEqual(host.value.cpuColumns, columns, "a load-only update must not clear the columns");
  act(() => host.value.consume([{ kind: "gigView", gigViewShown: true } as QcStateUpdate, cpu({ cpuColumns: [] })]));
  assert.equal(host.value.cpuLoad, 55, "a columns-only update must not reset the load");
  assert.deepEqual(host.value.cpuColumns, []);
  host.renderer.unmount();
});

test("the monitor overlay carries the tracked readings only while it is open", () => {
  const host = mount();
  const columns = [{ column: 3, load: 80 }] as unknown as NonNullable<QcStateUpdate["cpuColumns"]>;
  act(() => host.value.consume([cpu({ cpuTotalLoad: 12, cpuColumns: columns })]));
  const close = () => {};
  assert.equal(host.value.monitor(false, undefined, close), undefined);
  assert.deepEqual(host.value.monitor(true, "grid-menu", close), { cpuLoad: 12, cpuColumns: columns, returnFocusTarget: "grid-menu", onClose: close });
  host.renderer.unmount();
});
