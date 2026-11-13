import assert from "node:assert/strict";
import test from "node:test";
import { listenScoped } from "../src/scoped-listener.ts";

const deferredListen = () => {
  let attach!: (stop: () => void) => void;
  let fail!: (error: unknown) => void;
  const handlers: Array<(event: { payload: unknown }) => void> = [];
  const listen = ((_event: string, handler: (event: { payload: unknown }) => void) => {
    handlers.push(handler);
    return new Promise<() => void>((resolve, reject) => { attach = resolve; fail = reject; });
  }) as Parameters<typeof listenScoped>[0];
  return { listen, handlers, attach: (stop: () => void) => attach(stop), fail: (error: unknown) => fail(error) };
};

test("a dispose before the listener attaches stops it the moment it arrives (WF-15)", async () => {
  const { listen, attach } = deferredListen();
  let stops = 0;
  const dispose = listenScoped(listen, "ev", () => undefined);
  dispose();
  attach(() => { stops += 1; });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(stops, 1);
});

test("a dispose after attachment stops it once, and later events are ignored", async () => {
  const { listen, handlers, attach } = deferredListen();
  let stops = 0;
  const seen: unknown[] = [];
  const dispose = listenScoped(listen, "ev", ({ payload }) => seen.push(payload));
  attach(() => { stops += 1; });
  await new Promise((resolve) => setTimeout(resolve, 0));
  handlers[0]!({ payload: 1 });
  dispose();
  dispose();
  handlers[0]!({ payload: 2 });
  assert.deepEqual(seen, [1]);
  assert.equal(stops, 1);
});

test("an attachment failure reaches onError unless the owner is already gone", async () => {
  const first = deferredListen();
  const errors: unknown[] = [];
  listenScoped(first.listen, "ev", () => undefined, (error) => errors.push(error));
  first.fail(new Error("refused"));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(errors.length, 1);
  const second = deferredListen();
  const dispose = listenScoped(second.listen, "ev", () => undefined, (error) => errors.push(error));
  dispose();
  second.fail(new Error("late"));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(errors.length, 1);
});
