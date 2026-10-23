import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { desktopPlatform } from "../src/host-platform.ts";
import {
  antigravityIntegrationPresentation,
  chatgptIntegrationPresentation,
  claudeExtensionPresentation,
  localRowButtons,
  managedMcpIntegrationPresentation,
  offersThirdPartyAcquisition,
  type LocalProviderHandlers,
  type LocalProviderPresentation,
} from "../src/local-integration-presentation.ts";

// Microsoft Store policy 10.1.5 applies to the Windows build only. Every
// other platform keeps its install/update links.
// "unknown" stands for a host the webview cannot name; a literal undefined would
// fall through to the default parameter, which reads this test process's own
// navigator (Node reports win32 on Windows).
const NON_WINDOWS = ["Linux", "macOS", "unknown"] as const;

type Row = { name: string; presentation: (platform: string | undefined) => LocalProviderPresentation };

const absentRows = (integrationInstalled: boolean): Row[] => [
  { name: "Claude Desktop", presentation: (p) => claudeExtensionPresentation({ claudeInstalled: false } as never, false, false, false, p) },
  { name: "ChatGPT Desktop", presentation: (p) => chatgptIntegrationPresentation({ chatgptInstalled: false, integrationInstalled } as never, false, false, false, p) },
  { name: "Codex CLI", presentation: (p) => chatgptIntegrationPresentation({ chatgptInstalled: false, integrationInstalled, clientName: "Codex CLI" } as never, false, false, false, p) },
  { name: "Google Antigravity", presentation: (p) => antigravityIntegrationPresentation({ antigravityInstalled: false, integrationInstalled } as never, false, p) },
  { name: "Bionic", presentation: (p) => managedMcpIntegrationPresentation("Bionic", { integrationInstalled } as never, false, undefined, false, p) },
  { name: "LM Studio", presentation: (p) => managedMcpIntegrationPresentation("LM Studio", { integrationInstalled } as never, false, "the Program tab", false, p) },
  { name: "Jan", presentation: (p) => managedMcpIntegrationPresentation("Jan", { integrationInstalled } as never, false, undefined, false, p) },
];

// A handler for every act, including the install act, so a button that should
// not exist cannot hide behind a missing handler.
const allHandlers = (ran: string[]): LocalProviderHandlers => ({
  open: () => ran.push("open"),
  repair: () => ran.push("repair"),
  pair: () => ran.push("pair"),
  install: () => ran.push("install"),
  refresh: () => ran.push("refresh"),
  copy: () => ran.push("copy"),
  configure: () => ran.push("configure"),
  unpair: () => ran.push("unpair"),
});

const labels = (presentation: LocalProviderPresentation, platform: string | undefined) =>
  localRowButtons(presentation, allHandlers([]), platform).map((button) => `${button.act}:${button.label}`);

test("the acquisition rule is Windows-only", () => {
  assert.equal(offersThirdPartyAcquisition("Windows"), false);
  for (const platform of NON_WINDOWS) assert.equal(offersThirdPartyAcquisition(platform), true);
});

test("Windows: a missing AI client has no install action and a row that does not mention pairing", () => {
  for (const row of absentRows(false)) {
    const presentation = row.presentation("Windows");
    assert.equal(presentation.primary, undefined, row.name);
    assert.equal(presentation.canUnpair, false, row.name);
    assert.deepEqual(labels(presentation, "Windows"), [], row.name);
    assert.equal(presentation.detail, `${row.name} is not detected. It will appear here when it is installed.`);
    assert.doesNotMatch(presentation.detail, /pair|download|install (it|claude|chatgpt|codex|google|bionic|lm|jan)\b/i, row.name);
  }
});

// Claude's row has no leftover state: its extension lives in Claude's own
// registry, so an uninstalled Claude leaves nothing this app could unpair.
const leftoverRows = () => absentRows(true).filter((row) => row.name !== "Claude Desktop");

test("Windows: a leftover connection for a missing client offers Unpair and nothing to acquire", () => {
  for (const row of leftoverRows()) {
    const presentation = row.presentation("Windows");
    assert.equal(presentation.status, "Connection left over", row.name);
    assert.equal(presentation.canUnpair, true, row.name);
    assert.deepEqual(labels(presentation, "Windows"), ["unpair:Unpair"], row.name);
    assert.match(presentation.detail, /Unpair to remove it\.$/, row.name);
    assert.doesNotMatch(presentation.detail, /download|install \w/i, row.name);
  }
});

test("Windows: an unsupported client version has no update link, but a configured connection can still be unpaired", () => {
  for (const integrationInstalled of [false, true]) {
    const presentation = managedMcpIntegrationPresentation("Jan", {
      integrationInstalled,
      state: "app-update-required",
      appVersion: "1.0",
    } as never, true, undefined, false, "Windows");
    assert.equal(presentation.primary, undefined);
    assert.equal(presentation.detail, "Jan 1.0 is too old to speak local MCP.");
    assert.deepEqual(labels(presentation, "Windows"), integrationInstalled ? ["unpair:Unpair"] : []);
  }
});

test("Windows: the install act is unreachable even if a presentation asked for it", () => {
  const ran: string[] = [];
  const buttons = localRowButtons(
    { phase: "unavailable", status: "x", tone: "neutral", detail: "x", canUnpair: false, primary: { act: "install", label: "Install X" }, secondary: { act: "install", label: "Install Y" } },
    allHandlers(ran),
    "Windows",
  );
  assert.deepEqual(buttons, []);
  assert.deepEqual(ran, []);
});

test("Windows: installed clients keep Pair, Update, Open and Unpair", () => {
  const cases: Array<[string, LocalProviderPresentation, string[]]> = [
    ["Claude pair", claudeExtensionPresentation({ claudeInstalled: true, extensionInstalled: false, state: "not-installed" } as never, false, false, false, "Windows"), ["pair:Pair"]],
    ["Claude update", claudeExtensionPresentation({ claudeInstalled: true, extensionInstalled: true, state: "update-available", availableVersion: "2" } as never, false, false, false, "Windows"), ["repair:Update", "unpair:Unpair"]],
    ["ChatGPT pair", chatgptIntegrationPresentation({ chatgptInstalled: true, integrationInstalled: false, state: "not-installed" } as never, false, false, false, "Windows"), ["pair:Pair"]],
    ["ChatGPT update", chatgptIntegrationPresentation({ chatgptInstalled: true, integrationInstalled: true, state: "update-available" } as never, false, false, false, "Windows"), ["repair:Update", "unpair:Unpair"]],
    ["ChatGPT open", chatgptIntegrationPresentation({ chatgptInstalled: true, integrationInstalled: true, state: "current", connectionActive: true } as never, false, false, false, "Windows"), ["open:Open", "unpair:Unpair"]],
    ["Antigravity pair", antigravityIntegrationPresentation({ antigravityInstalled: true, integrationInstalled: false, state: "not-installed" } as never, false, "Windows"), ["pair:Pair"]],
    ["Antigravity update", antigravityIntegrationPresentation({ antigravityInstalled: true, integrationInstalled: true, state: "update-available" } as never, false, "Windows"), ["repair:Update", "unpair:Unpair"]],
    ["Jan pair", managedMcpIntegrationPresentation("Jan", { integrationInstalled: false, state: "not-installed" } as never, true, undefined, false, "Windows"), ["pair:Pair"]],
    ["Jan update", managedMcpIntegrationPresentation("Jan", { integrationInstalled: true, state: "update-available" } as never, true, undefined, false, "Windows"), ["repair:Update", "unpair:Unpair"]],
    ["Bionic open", managedMcpIntegrationPresentation("Bionic", { integrationInstalled: true, state: "current", connectionActive: true } as never, true, undefined, false, "Windows"), ["open:Open", "unpair:Unpair"]],
  ];
  for (const [name, presentation, expected] of cases) assert.deepEqual(labels(presentation, "Windows"), expected, name);
});

test("non-Windows platforms keep their install and update actions unchanged", () => {
  for (const platform of NON_WINDOWS) {
    const expectedLabel: Record<string, string> = {
      "Claude Desktop": "Install Claude Desktop",
      "ChatGPT Desktop": "Install ChatGPT Desktop",
      "Codex CLI": "Install Codex CLI",
      "Google Antigravity": "Install Antigravity",
      Bionic: "Install Bionic",
      "LM Studio": "Install LM Studio",
      Jan: "Install Jan",
    };
    for (const row of absentRows(false)) {
      const presentation = row.presentation(platform);
      assert.deepEqual(presentation.primary, { act: "install", label: expectedLabel[row.name] }, `${platform} ${row.name}`);
      if (row.name !== "Claude Desktop") assert.equal(presentation.detail, `Install ${row.name} to control QC Remote with AI`);
      const ran: string[] = [];
      const buttons = localRowButtons(presentation, allHandlers(ran), platform);
      assert.deepEqual(buttons.map((button) => button.label), [expectedLabel[row.name]]);
      buttons[0].run();
      assert.deepEqual(ran, ["install"]);
    }
    for (const row of leftoverRows()) {
      const presentation = row.presentation(platform);
      assert.equal(presentation.status, "Connection left over");
      assert.match(presentation.detail, / or install .+ to use it\.$/);
      assert.deepEqual(labels(presentation, platform), [`install:${expectedLabel[row.name]}`, "unpair:Unpair"]);
    }
    const stale = managedMcpIntegrationPresentation("Jan", { integrationInstalled: true, state: "app-update-required", appVersion: "1.0" } as never, true, undefined, false, platform);
    assert.deepEqual(stale.primary, { act: "install", label: "Update Jan" });
    assert.equal(stale.detail, "Jan 1.0 is too old to speak local MCP. Update Jan opens its download page.");
  }
});

test("without a platform argument the row follows the platform this webview reports", () => {
  const presentation = claudeExtensionPresentation({ claudeInstalled: false } as never);
  assert.deepEqual(presentation.primary, desktopPlatform === "Windows" ? undefined : { act: "install", label: "Install Claude Desktop" });
});

test("the native download commands and their registrations are compiled out of Windows builds only", async () => {
  const root = new URL("../", import.meta.url);
  const clientPackages = await readFile(fileURLToPath(new URL("src-tauri/src/client_packages.rs", root)), "utf8");
  const lib = await readFile(fileURLToPath(new URL("src-tauri/src/lib.rs", root)), "utf8");
  const commands = [
    "open_claude_desktop_download",
    "open_chatgpt_desktop_download",
    "open_antigravity_download",
    "open_lm_studio_download",
    "open_jan_download",
  ];
  for (const command of commands) {
    const definition = new RegExp(`#\\[cfg\\(not\\(windows\\)\\)\\]\\n#\\[tauri::command\\]\\npub fn ${command}\\(`);
    assert.match(clientPackages, definition, `${command} must be cfg(not(windows))`);
    const registration = new RegExp(`\\n\\s+#\\[cfg\\(not\\(windows\\)\\)\\]\\n\\s+${command},\\n`);
    assert.match(lib, registration, `${command} registration must be cfg(not(windows))`);
    assert.equal(lib.split(command).length - 1, 2, `${command} appears only in its gated import and gated registration`);
  }
  for (const constant of ["ANTIGRAVITY_DOWNLOAD_URL", "LM_STUDIO_DOWNLOAD_URL", "JAN_DOWNLOAD_URL"]) {
    assert.match(clientPackages, new RegExp(`#\\[cfg\\(not\\(windows\\)\\)\\]\\nconst ${constant}:`), `${constant} must be cfg(not(windows))`);
  }
  // The ungated import list must not name any of them.
  const ungated = lib.slice(lib.indexOf("use client_packages::{"), lib.indexOf("};", lib.indexOf("use client_packages::{")));
  for (const command of commands) assert.equal(ungated.includes(command), false, `${command} must not be in the ungated import`);
});

test("sandbox folder consent keeps Pair available instead of misreporting a broken configuration", () => {
  const common = { folderAccessRequired: true, state: "status-error", statusError: "Permission denied", integrationInstalled: false };
  const rows = [
    claudeExtensionPresentation({ ...common, claudeInstalled: true, extensionInstalled: false } as never),
    chatgptIntegrationPresentation({ ...common, chatgptInstalled: true } as never),
    antigravityIntegrationPresentation({ ...common, antigravityInstalled: true } as never),
    managedMcpIntegrationPresentation("Bionic", common as never, true, undefined),
    managedMcpIntegrationPresentation("Jan", common as never, true, undefined),
  ];
  for (const row of rows) {
    assert.equal(row.status, "Folder access needed");
    assert.equal(row.primary?.act, "pair");
    assert.equal(row.canUnpair, false);
    assert.match(row.detail, /existing pairing cannot be checked/);
  }
});
