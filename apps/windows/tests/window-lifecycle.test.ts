import assert from "node:assert/strict";
import test from "node:test";
import { createWindowPromptSession, type WindowPromptRequest } from "../src/window-prompt-session.ts";
import { createBrowserPairingSession, startSequentialPairingPoll } from "../src/browser-pairing-session.ts";
import { saveOpenedWorkspaceCopy } from "../src/workspace-copy.ts";
import type { WorkspaceDocument } from "@qc-remote/client";

test("replaced prompts cancel their original caller and stale answers cannot accept the replacement", async () => {
  const session = createWindowPromptSession();
  let active: WindowPromptRequest | undefined;
  session.attach((request) => { active = request; });
  const first = session.confirm("Overwrite preset A?");
  const firstId = active!.id;
  const second = session.confirm("Overwrite preset B?");
  assert.equal(await first, false);
  session.answer(firstId, true);
  assert.equal(active!.message, "Overwrite preset B?");
  session.answer(active!.id, false);
  assert.equal(await second, false);
});

test("switching prompt kinds cancels text entry and disposal settles the active confirmation", async () => {
  const session = createWindowPromptSession();
  const dispose = session.attach(() => undefined);
  const text = session.prompt("Backup name", "Backup");
  const confirm = session.confirm("Unpair?");
  assert.equal(await text, null);
  dispose();
  assert.equal(await confirm, false);
  assert.equal(await session.prompt("After close"), null);
});

test("effect cleanup and remount can attach the same prompt session without accepting a stale answer", async () => {
  const session = createWindowPromptSession();
  let active: WindowPromptRequest | undefined;
  const detach = session.attach((request) => { active = request; });
  const old = session.prompt("Old", "old");
  const oldId = active!.id;
  detach();
  session.attach((request) => { active = request; });
  const fresh = session.prompt("New", "new");
  session.answer(oldId, "wrong");
  assert.equal(active!.kind, "text");
  session.answer(active!.id, "new");
  assert.equal(await old, null);
  assert.equal(await fresh, "new");
});

test("closing REMOTE during a slow begin cancels its late native offer before reopening", async () => {
  let finish!: (value: string) => void;
  let nativeOffer: string | undefined;
  let begins = 0;
  const calls: string[] = [];
  const session = createBrowserPairingSession({
    async begin() {
      calls.push("begin");
      const offer = ++begins === 1 ? await new Promise<string>((resolve) => { finish = resolve; }) : "new";
      nativeOffer = offer;
      return offer;
    },
    async cancel() { calls.push("cancel"); nativeOffer = undefined; }
  });
  const old = session.begin();
  await Promise.resolve();
  const close = session.cancel();
  const fresh = session.begin();
  finish("old");
  const discarded = await old;
  assert.equal(discarded.current(), false);
  await close;
  const current = await fresh;
  assert.equal(current.current(), true);
  assert.equal(current.offer, "new");
  assert.equal(nativeOffer, "new");
  assert.deepEqual(calls, ["begin", "cancel", "begin"]);
});

test("a failed begin still permits cancellation and a new pairing", async () => {
  let begins = 0;
  let cancelled = false;
  const session = createBrowserPairingSession({
    async begin() { if (++begins === 1) throw new Error("offline"); return "new"; },
    async cancel() { cancelled = true; }
  });
  await assert.rejects(session.begin(), /offline/);
  await session.cancel();
  assert.equal(cancelled, true);
  assert.equal((await session.begin()).offer, "new");
});

test("slow pairing polls never overlap and cleanup rejects their late result", async () => {
  let finish!: () => void;
  let polls = 0;
  let applied = 0;
  const scheduled: Array<() => void> = [];
  const stop = startSequentialPairingPoll(async (current) => {
    polls += 1;
    await new Promise<void>((resolve) => { finish = resolve; });
    if (current()) applied += 1;
  }, (run) => { scheduled.push(run); return run; }, () => undefined);
  assert.equal(polls, 1);
  assert.equal(scheduled.length, 0);
  finish();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(applied, 1);
  assert.equal(scheduled.length, 1);
  scheduled[0]();
  assert.equal(polls, 2);
  stop();
  finish();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(applied, 1);
  assert.equal(scheduled.length, 1);
});

test("Save Copy preserves the opened workspace instead of exporting the live preset", async () => {
  const opened = { version: 1, savedAt: "2026-01-01T00:00:00Z", source: { presetLocation: "2B", presetName: "Opened A" }, snapshot: { presetName: "Opened A" }, ui: {} } as WorkspaceDocument;
  let written: WorkspaceDocument | undefined;
  let suggested: string | undefined;
  const writer = { async saveAs(document: WorkspaceDocument, name: string) { written = document; suggested = name; return { cancelled: false, path: "copy.qcw" }; } };
  await saveOpenedWorkspaceCopy(writer, opened);
  assert.equal(written, opened);
  assert.equal(written!.snapshot.presetName, "Opened A");
  assert.equal(written!.savedAt, opened.savedAt);
  assert.equal(suggested, "2B Opened A");
  await assert.rejects(saveOpenedWorkspaceCopy(writer, undefined), /No workspace is open/);
});
