import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { connectorsNeedRefreshAfter, runConnectorUnlink } from "../src/relay-connectors.ts";

const harness = (overrides: Partial<Parameters<typeof runConnectorUnlink<string[]>>[0]> = {}) => {
  const calls: string[] = [];
  const operation = {
    confirm: async () => true,
    unlink: async () => { calls.push("unlink"); return ["remaining"]; },
    current: () => true,
    onUnlinked: (list: string[]) => { calls.push(`unlinked:${list.join(",")}`); },
    onFailed: (error: unknown) => { calls.push(`failed:${(error as Error).message}`); },
    ...overrides
  };
  return { calls, operation };
};

test("a successful unlink applies the returned list and needs no refresh (WF-4)", async () => {
  const { calls, operation } = harness();
  const outcome = await runConnectorUnlink(operation);
  assert.equal(outcome, "unlinked");
  assert.deepEqual(calls, ["unlink", "unlinked:remaining"]);
  assert.equal(connectorsNeedRefreshAfter(outcome), false);
});

test("a failed unlink is an action failure that asks for a fresh read, never a list error (WF-4)", async () => {
  const { calls, operation } = harness({ unlink: async () => { throw new Error("offline"); } });
  const outcome = await runConnectorUnlink(operation);
  assert.equal(outcome, "failed");
  assert.deepEqual(calls, ["failed:offline"]);
  assert.equal(connectorsNeedRefreshAfter(outcome), true);
});

test("a declined confirmation and a superseded mutation do nothing and need no refresh", async () => {
  const declined = harness({ confirm: async () => false });
  assert.equal(await runConnectorUnlink(declined.operation), "cancelled");
  assert.deepEqual(declined.calls, []);
  let live = true;
  const superseded = harness({ unlink: async () => { live = false; return ["x"]; }, current: () => live });
  const outcome = await runConnectorUnlink(superseded.operation);
  assert.equal(outcome, "superseded");
  assert.deepEqual(superseded.calls, []);
  assert.equal(connectorsNeedRefreshAfter(outcome), false);
});

test("App's unlink never writes its failure into the list's read-error slot", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const body = app.slice(app.indexOf("const unlinkRelayConnector"), app.indexOf("const shareRemoteConnectorAddress"));
  assert.match(body, /runConnectorUnlink\(/);
  assert.doesNotMatch(body, /setRelayConnectorsError\(\s*error/);
  assert.match(body, /onFailed: actionFailed/);
});
