import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create } from "react-test-renderer";
import { observedGlobalBypassUpdate } from "../src/global-bypass-update.ts";

register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const { useDeviceSettingsWorkflow } = await import("../src/use-device-settings-workflow.ts");
test("Device settings reject prior-session masks and late read/write completions on a stable gateway", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  let value, renderer;
  let resolveRead!: (value: unknown) => void;
  let resolveWrite!: (value: unknown) => void;
  const first = { globalBypassCab: { row1: false, row2: true, row3: false, row4: false } };
  const replacement = { globalBypassCab: { row1: false, row2: false, row3: false, row4: false } };
  let read = async () => first;
  const writes: unknown[] = [], notices: string[] = [], pending: boolean[] = [];
  const gateway = {
    generalSettings: () => read(),
    setGlobalBypass: (cab, ir) => { writes.push({ cab, ir }); return new Promise((resolve) => { resolveWrite = resolve; }); }
  };
  let options = { gateway, sessionKey: "usb-A", connected: true, pending: false,
    setPending: (busy: boolean) => pending.push(busy), notice: (message: string) => notices.push(message), fail: (error) => { throw error; } };
  function Harness() { value = useDeviceSettingsWorkflow(options); return null; }
  await act(async () => { renderer = create(React.createElement(Harness)); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  await act(async () => { await value.refresh(); });
  assert.deepEqual(value.settings, first);
  read = () => new Promise((resolve) => { resolveRead = resolve; });
  let staleRead;
  await act(async () => { staleRead = value.refresh(); });
  let staleWrite;
  await act(async () => { staleWrite = value.setGlobalBypass("cab", 1, true); });
  assert.deepEqual(writes, [{ cab: [true, true, false, false], ir: null }]);
  pending.length = 0;
  await act(async () => { options = { ...options, sessionKey: "usb-B" }; renderer.update(React.createElement(Harness)); });
  assert.equal(value.settings, undefined);
  assert.deepEqual(pending, [false], "The session boundary releases the old operation's pending ownership");
  notices.length = 0; pending.length = 0;
  await act(async () => { resolveRead(first); await staleRead; resolveWrite({ detail: "old completion" }); await staleWrite; });
  assert.equal(value.settings, undefined);
  assert.deepEqual(notices, []);
  assert.deepEqual(pending, []);
  await act(async () => { await value.setGlobalBypass("cab", 1, true); });
  assert.equal(writes.length, 1, "The replacement session has no observed mask yet");
  read = async () => replacement;
  await act(async () => { await value.refresh(); });
  let newWrite;
  await act(async () => { newWrite = value.setGlobalBypass("cab", 1, true); });
  assert.deepEqual(writes[1], { cab: [true, false, false, false], ir: null });
  await act(async () => { resolveWrite({ detail: "current completion" }); await newWrite; });
  assert.equal(notices.at(-1), "current completion");
  read = () => new Promise((resolve) => { resolveRead = resolve; });
  let afterUnmount;
  await act(async () => { afterUnmount = value.refresh(); });
  await act(async () => renderer.unmount());
  notices.length = 0; pending.length = 0;
  await act(async () => { resolveRead(first); await afterUnmount; });
  assert.deepEqual(notices, []);
  assert.deepEqual(pending, []);
});

test("Global Bypass edits preserve observed rows and never replace unknown rows with false", () => {
  assert.equal(observedGlobalBypassUpdate(undefined, "cab", 1, true), undefined);
  assert.equal(observedGlobalBypassUpdate({ globalBypassCab: { row1: false, row2: true, row4: true } }, "cab", 1, true), undefined);
  const settings = { globalBypassCab: { row1: false, row2: true, row3: false, row4: true } };
  assert.deepEqual(observedGlobalBypassUpdate(settings, "cab", 3, true), { cab: [false, true, true, true], ir: null });
  assert.deepEqual(settings.globalBypassCab, { row1: false, row2: true, row3: false, row4: true });
  assert.deepEqual(observedGlobalBypassUpdate({ globalBypassIr: { row1: true, row2: false, row3: true, row4: false } }, "ir", 1, false), { cab: null, ir: [false, false, true, false] });
});
