import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { appendConnectionEvent, DIAGNOSTICS_EVENT_COUNT, MAX_CONNECTION_EVENTS } from "../src/connection-events.ts";

test("appending past the cap drops the oldest entries and keeps order (WF-3)", () => {
  let events: number[] = [];
  for (let i = 0; i < MAX_CONNECTION_EVENTS + 25; i += 1) events = appendConnectionEvent(events, i);
  assert.equal(events.length, MAX_CONNECTION_EVENTS);
  assert.equal(events[0], 25);
  assert.equal(events.at(-1), MAX_CONNECTION_EVENTS + 24);
  assert.deepEqual(appendConnectionEvent([1, 2, 3], 4, 3), [2, 3, 4]);
  assert.deepEqual(appendConnectionEvent([1], 2), [1, 2]);
});

test("the cap leaves the diagnostics window of the newest entries intact", () => {
  assert.ok(DIAGNOSTICS_EVENT_COUNT <= MAX_CONNECTION_EVENTS);
});

test("App appends connection events only through the capped helper", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  assert.equal((app.match(/setConnectionEvents\(/g) ?? []).length, 2, "the capped helper and the clear button");
  assert.doesNotMatch(app, /setConnectionEvents\(\(?\w+\)? => \[\.\.\.\w+, /);
});
