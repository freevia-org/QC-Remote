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

test("the CorOS version the unit's Device Information screen showed is kept for the session, reconciled with the host's, and dropped with the session", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  let value, renderer;
  let identity: { serial: string; corOsVersion?: string } = { serial: "SYN-0001" };
  const gateway = { generalSettings: async () => ({}), identity: async () => identity };
  let options = { gateway, sessionKey: "usb-A", connected: true, pending: false,
    setPending: () => {}, notice: () => {}, fail: (error) => { throw error; } };
  function Harness() { value = useDeviceSettingsWorkflow(options); return null; }
  await act(async () => { renderer = create(React.createElement(Harness)); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  assert.equal(value.screenCorOsVersion, undefined);

  // Anything that is not a dotted version number is not kept, whatever the screen showed.
  await act(async () => { for (const text of ["XXXXXXXX", "d14e", "", "4", "1.2.3.4.5", "XX:XX:XX:XX:XX:XX"]) value.noteCorOsVersionFromScreen(text); });
  assert.equal(value.screenCorOsVersion, undefined);

  await act(async () => { value.noteCorOsVersionFromScreen("4.1.0"); });
  assert.equal(value.screenCorOsVersion.value, "4.1.0");
  const first = value.screenCorOsVersion;
  // Seen again at once: nothing re-renders. The record only holds a version number and a time.
  await act(async () => { value.noteCorOsVersionFromScreen("4.1.0"); });
  assert.equal(value.screenCorOsVersion, first);
  assert.deepEqual(Object.keys(value.screenCorOsVersion).sort(), ["at", "value"]);

  // The screen closing on the unit changes nothing: the value is still there (no read, no further note).
  await act(async () => { await value.refresh(); });
  assert.equal(value.screenCorOsVersion.value, "4.1.0");

  // The identity read reports the host's value: both are stated; the screen was seen before the read, so the host's wins.
  identity = { serial: "SYN-0001", corOsVersion: "4.1.0" };
  await act(async () => { value.openSettings(); });
  assert.equal(typeof value.identityReadAt, "number");
  assert.ok(value.identityReadAt >= value.screenCorOsVersion.at);

  // A different session never inherits it.
  await act(async () => { options = { ...options, sessionKey: "usb-B" }; renderer.update(React.createElement(Harness)); });
  assert.equal(value.screenCorOsVersion, undefined);
  assert.equal(value.identity, undefined);
  await act(async () => { value.noteCorOsVersionFromScreen("4.2.0"); });
  assert.equal(value.screenCorOsVersion.value, "4.2.0");

  // A reading that arrives for a session that has ended is dropped.
  const stale = value.noteCorOsVersionFromScreen;
  await act(async () => { options = { ...options, sessionKey: "usb-C" }; renderer.update(React.createElement(Harness)); });
  await act(async () => { stale("9.9.9"); });
  assert.equal(value.screenCorOsVersion, undefined);
});

test("following the unit's Settings pane selects the app's section and row, writes nothing, and ignores a session that has ended", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  let value, renderer;
  const calls: string[] = [];
  const gateway = new Proxy({ generalSettings: async () => ({}), identity: async () => ({ serial: "SYN-0001" }) }, {
    get(target, key) { if (key in target) return target[key]; return (...args) => { calls.push(String(key)); return Promise.resolve({ detail: "x" }); }; }
  });
  let options = { gateway, sessionKey: "usb-A", connected: true, pending: false,
    setPending: () => {}, notice: () => {}, fail: (error) => { throw error; } };
  function Harness() { value = useDeviceSettingsWorkflow(options); return null; }
  await act(async () => { renderer = create(React.createElement(Harness)); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  assert.equal(value.section, "system", "the app's own default until the unit says otherwise");
  await act(async () => { value.followUnitSelection({ section: "support", row: "info" }); });
  assert.equal(value.section, "support");
  assert.equal(value.supportRow, "info");
  await act(async () => { value.followUnitSelection({ section: "system", row: "updates" }); });
  assert.deepEqual([value.section, value.systemRow, value.supportRow], ["system", "updates", "info"]);
  // The app's own navigation still works after following.
  await act(async () => { value.setSection("device"); value.setDeviceRow("latency"); });
  assert.deepEqual([value.section, value.deviceRow], ["device", "latency"]);
  assert.deepEqual(calls, [], "following reads and writes nothing on the unit");
  const stale = value.followUnitSelection;
  await act(async () => { options = { ...options, sessionKey: "usb-B" }; renderer.update(React.createElement(Harness)); });
  await act(async () => { stale({ section: "account", row: "backup" }); });
  assert.notEqual(value.accountRow, "backup", "a pane named for the previous session is not applied");
});
