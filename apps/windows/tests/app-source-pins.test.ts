import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8").replaceAll("\r\n", "\n");

test("the device workflow session is built once; hooks spread it and keep no private copy (WF-10)", () => {
  assert.equal((app.match(/captureSessionCurrent: \(\) => \{/g) ?? []).length, 1, "one captureSessionCurrent closure");
  assert.match(app, /useExpressionAssignment\(\{\s*\.\.\.deviceWorkflowSession,/);
  assert.match(app, /useDeviceSettingsWorkflow\(\{\s*\.\.\.deviceWorkflowSession,/);
  assert.match(app, /captureSessionCurrent: deviceWorkflowSession\.captureSessionCurrent/);
  // Device settings are keyed to the live device, not Demo readiness: an explicit override.
  assert.match(app, /sessionKey: `\$\{connection\.source\}:[^`]*\$\{deviceReady\}/);
});
