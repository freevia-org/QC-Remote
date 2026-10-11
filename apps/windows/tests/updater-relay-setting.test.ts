import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  DEFAULT_BROKER_TRAY_STATUS,
  UPDATER_RELAY_EXPLANATION,
  UPDATER_RELAY_LABEL,
  summarizeUpdaterRelay,
  updaterRelayStatusLine,
  type SettingsSetting
} from "../src/app-window-content.ts";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const source = (relative: string) => readFileSync(path.join(root, relative), "utf8");

// "Internet for the Quad Cortex" is an app setting that the Settings window
// writes through the shell. It is not a Quad Cortex action, so no gateway
// method, MCP tool or public contract may mention it: adding one would move the
// pinned action counts and tool-contract hashes.

test("the switch is on by default, on a fresh install and for settings that predate it", () => {
  assert.equal(DEFAULT_BROKER_TRAY_STATUS.updaterRelayEnabled, true);
});

test("the row says what it does in plain words", () => {
  assert.equal(UPDATER_RELAY_LABEL, "Internet for the Quad Cortex");
  assert.equal(
    UPDATER_RELAY_EXPLANATION,
    "Lets the Quad Cortex use this computer's internet connection to reach Neural DSP's servers (neuraldsp.com, plus any extra domain you add) for updates while it is connected over USB. QC Remote only passes the Quad Cortex's own requests through; they include the unit's serial number and its own credentials, which go to the address the unit asks for: Neural DSP's, or an extra domain you added."
  );
  // The row names the recipient and what it receives, not only the privacy page.
  assert.match(UPDATER_RELAY_EXPLANATION, /Neural DSP/);
  assert.match(UPDATER_RELAY_EXPLANATION, /serial number/);
  assert.match(UPDATER_RELAY_EXPLANATION, /credentials/);
});

test("relay diagnostics are read defensively, and an older broker's silence is null", () => {
  assert.equal(summarizeUpdaterRelay(undefined), null);
  assert.equal(summarizeUpdaterRelay(null), null);
  assert.equal(summarizeUpdaterRelay("on"), null);
  assert.deepEqual(summarizeUpdaterRelay({}), {
    enabled: null, requestsSeen: 0, requestsWhileDisabled: 0, inFlight: false, lastOutcome: null
  });
  assert.deepEqual(
    summarizeUpdaterRelay({ enabled: false, requestsSeen: 5, requestsWhileDisabled: 2.9, inFlight: true, lastOutcome: "completed" }),
    { enabled: false, requestsSeen: 5, requestsWhileDisabled: 2, inFlight: true, lastOutcome: "completed" }
  );
  for (const bad of ["true", 1, null, {}]) {
    assert.equal(summarizeUpdaterRelay({ enabled: bad })?.enabled, null);
  }
  for (const bad of [Number.NaN, "7", null, {}, []]) {
    assert.equal(summarizeUpdaterRelay({ requestsSeen: bad })?.requestsSeen, 0);
  }
  for (const bad of [7, "", "x".repeat(81), null, {}]) {
    assert.equal(summarizeUpdaterRelay({ lastOutcome: bad })?.lastOutcome, null);
  }
});

test("the summary keeps nothing but counts, the running flag and the outcome word", () => {
  // The broker's status is readable by the AI apps attached to it, so it no
  // longer carries a host, a path, the allowed list or the audit; and if an
  // older broker still sent them, nothing here would pass them on.
  const summary = summarizeUpdaterRelay({
    requestsSeen: 3,
    lastRefusedHost: "cdn.example.net",
    allowedDomains: ["neuraldsp.com", "cdn.example.net"],
    note: "The Quad Cortex asked 1 time(s)",
    recent: [{ outcome: "completed", host: "cloud-api.neuraldsp.com", path: "/private?token=SECRET" }]
  });
  assert.deepEqual(Object.keys(summary ?? {}).sort(), ["enabled", "inFlight", "lastOutcome", "requestsSeen", "requestsWhileDisabled"]);
  assert.doesNotMatch(JSON.stringify(summary), /example|neuraldsp|SECRET|private|token/);
});

test("the live line follows the stored switch while the broker agrees with it", () => {
  const idle = summarizeUpdaterRelay({});
  assert.equal(updaterRelayStatusLine(true, null), "On.");
  assert.equal(updaterRelayStatusLine(true, idle), "On. The Quad Cortex has not asked for anything yet.");
  assert.equal(updaterRelayStatusLine(false, null), "Off. The Quad Cortex cannot use this computer's internet connection.");
  assert.equal(updaterRelayStatusLine(false, idle), "Off. The Quad Cortex cannot use this computer's internet connection.");
  assert.equal(
    updaterRelayStatusLine(false, summarizeUpdaterRelay({ requestsWhileDisabled: 1 })),
    "Off. The Quad Cortex asked for the internet once while this was off."
  );
  assert.equal(
    updaterRelayStatusLine(false, summarizeUpdaterRelay({ requestsWhileDisabled: 3 })),
    "Off. The Quad Cortex asked for the internet 3 times while this was off."
  );
});

test("the live line reports what the broker is actually doing when it differs from the stored switch", () => {
  // A damaged settings file at start, or a read that failed: the app's reader
  // shows On, the broker keeps its last value, Off.
  const brokerOff = summarizeUpdaterRelay({ enabled: false, lastOutcome: "completed" });
  const line = updaterRelayStatusLine(true, brokerOff);
  assert.ok(line);
  assert.match(line, /^On in Settings, but .* still has it off\./);
  assert.match(line, /damaged/);
  const brokerOn = summarizeUpdaterRelay({ enabled: true });
  assert.match(updaterRelayStatusLine(false, brokerOn) ?? "", /^Off in Settings, but .* still has it on\./);
  // When they agree, or the broker says nothing, the ordinary lines apply.
  assert.equal(updaterRelayStatusLine(true, summarizeUpdaterRelay({ enabled: true })), "On. The Quad Cortex has not asked for anything yet.");
  assert.equal(updaterRelayStatusLine(false, summarizeUpdaterRelay({ enabled: false })), "Off. The Quad Cortex cannot use this computer's internet connection.");
  assert.equal(updaterRelayStatusLine(true, summarizeUpdaterRelay({})), "On. The Quad Cortex has not asked for anything yet.");
});

test("the live line reports a request in flight, a declined request and the last result", () => {
  assert.equal(
    updaterRelayStatusLine(true, summarizeUpdaterRelay({ inFlight: true, lastOutcome: "refused:domain-not-allowed" })),
    "On. Passing a request from the Quad Cortex through now."
  );
  assert.equal(
    updaterRelayStatusLine(true, summarizeUpdaterRelay({ lastOutcome: "refused:domain-not-allowed" })),
    "On. Declined the Quad Cortex's last request: only neuraldsp.com addresses and the extra domains you added are passed through."
  );
  assert.equal(
    updaterRelayStatusLine(true, summarizeUpdaterRelay({ lastOutcome: "completed" })),
    "On. The Quad Cortex's last request went through."
  );
  for (const outcome of ["timeout", "network:tls", "http-status", "too-large", "refused:host-is-ip-literal"]) {
    assert.equal(
      updaterRelayStatusLine(true, summarizeUpdaterRelay({ lastOutcome: outcome })),
      "On. The Quad Cortex's last request did not complete.",
      outcome
    );
  }
  // A declined request is the latest result only until a later one replaces it.
  assert.equal(
    updaterRelayStatusLine(true, summarizeUpdaterRelay({ refusedDomainRequests: 2, lastOutcome: "completed" })),
    "On. The Quad Cortex's last request went through."
  );
});

test("the live line never carries a host, a path, a query or a credential", () => {
  const line = updaterRelayStatusLine(true, summarizeUpdaterRelay({
    lastOutcome: "refused:domain-not-allowed",
    lastRefusedHost: "cdn.example.net",
    lastRefusedPath: "/private/image.bin?token=SECRET",
    recent: [{ outcome: "refused:domain-not-allowed", host: "cdn.example.net", path: "/private?token=SECRET" }]
  }));
  assert.ok(line);
  assert.doesNotMatch(line, /SECRET|private|token|image\.bin|example/);
});

test("the Settings window wires the row to the shell through its own request and command", () => {
  const window = source("apps/windows/src/app-window.tsx");
  assert.match(window, /UPDATER_RELAY_LABEL/);
  assert.match(window, /change\("updater-relay-enabled", event\.target\.checked\)/);
  assert.match(window, /updaterRelayStatusLine\(/);
  const transport = source("apps/windows/src/tauri-transport.ts");
  assert.match(transport, /callTauri<BrokerTrayStatus>\("set_broker_updater_relay_enabled", \{ enabled \}\)/);
  assert.match(transport, /"updater-relay-enabled": \{ request: SETTINGS_EVENTS\.updaterRelayRequest, result: SETTINGS_EVENTS\.updaterRelayResult \}/);
  assert.match(transport, /run: handlers\.setUpdaterRelayEnabled/);
  const app = source("apps/windows/src/App.tsx");
  assert.match(app, /brokerTray\.setUpdaterRelayEnabled\(enabled\)/);
  assert.match(app, /typeof enabled !== "boolean"/);
  const shell = source("apps/windows/src-tauri/src/lib.rs");
  assert.match(shell, /fn set_broker_updater_relay_enabled\(/);
  assert.match(shell, /^\s+set_broker_updater_relay_enabled,$/m);
  const settingNames: SettingsSetting[] = ["form-factor", "control-access-mode", "follow-qc-screen", "tray-enabled", "idle-disconnect-minutes", "updater-relay-enabled"];
  for (const name of settingNames) assert.match(transport, new RegExp(`"${name}": \\{ request:`), name);
});

test("no gateway method, MCP action or public contract exposes the switch to an AI tool", () => {
  const publicContracts = readdirSync(path.join(root, "contracts/public")).filter((name) => name.endsWith(".json"));
  assert.ok(publicContracts.length > 0);
  for (const name of publicContracts) {
    assert.doesNotMatch(source(`contracts/public/${name}`), /updater[-_ ]?relay|updaterRelay|set_broker_updater/i, name);
  }
  for (const generated of ["packages/typescript/qc-core/src/generated-actions.ts", "services/rust-mcp/src/generated_instructions.rs"]) {
    assert.doesNotMatch(source(generated), /updater[-_ ]?relay|updaterRelay|set_broker_updater/i, generated);
  }
  assert.doesNotMatch(source("services/device-broker/src/generated_broker_surface.rs"), /updater[-_ ]?relay|updaterRelay/i);
});
