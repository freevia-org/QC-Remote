import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { browserPairingPollFailureMessage } from "../src/browser-pairing-session.ts";

test("a failed pairing poll replaces the stale creating state with an actionable error", () => {
  assert.equal(browserPairingPollFailureMessage(new Error("network unavailable")), "Could not check the pairing session. Retry to create a new code.");
  assert.equal(browserPairingPollFailureMessage(new Error("The browser pairing session expired. Start it again from QC Remote.")), "The browser pairing session expired. Start it again from QC Remote.");
});

test("pairing poll failures never copy backend or secret-bearing details into the UI", () => {
  assert.equal(browserPairingPollFailureMessage(new Error("https://example.invalid/pair?token=do-not-render")), "Could not check the pairing session. Retry to create a new code.");
  assert.equal(browserPairingPollFailureMessage("unexpected"), "Could not check the pairing session. Retry to create a new code.");
});

test("the App poll-failure branch clears the stale offer, shows the safe error, and routes Retry through the pairing refresh handler", () => {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const app = readFileSync(path.join(root, "src/App.tsx"), "utf8");
  const catchStart = app.indexOf("const result = await publicRelay.browserPairing!.poll();");
  assert.notEqual(catchStart, -1);
  const catchIndex = app.indexOf("} catch (error) {", catchStart);
  assert.notEqual(catchIndex, -1);
  const catchEnd = app.indexOf("\n      }", catchIndex);
  const pollBranch = app.slice(catchIndex, catchEnd);
  assert.match(pollBranch, /setRelayBrowserPairing\(undefined\)/);
  assert.match(pollBranch, /setRelayPairingError\(message\)/);
  assert.match(pollBranch, /actionFailed\(new Error\(message\)\)/);
  const menu = readFileSync(path.join(root, "src/menu-bar.tsx"), "utf8");
  assert.match(menu, /relayPairingError[\s\S]{0,180}RemotePairingFailureState error=\{relayPairingError\} onRetry=\{onRelayPairingRefresh\}/);
});
