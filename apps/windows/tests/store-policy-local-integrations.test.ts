import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import test from "node:test";
import {
  antigravityIntegrationPresentation,
  chatgptIntegrationPresentation,
  claudeExtensionPresentation,
  localRowButtons,
  managedMcpIntegrationPresentation,
} from "../src/local-integration-presentation.ts";

test("missing AI clients do not expose an install/download action", () => {
  const absent = [
    claudeExtensionPresentation({ claudeInstalled: false } as never),
    chatgptIntegrationPresentation({ chatgptInstalled: false, integrationInstalled: false } as never),
    antigravityIntegrationPresentation({ antigravityInstalled: false, integrationInstalled: false } as never),
    managedMcpIntegrationPresentation("Jan", { integrationInstalled: false } as never, false, undefined),
  ];

  for (const presentation of absent) {
    assert.equal(presentation.primary, undefined);
    assert.deepEqual(localRowButtons(presentation, {
      install: () => assert.fail("third-party install action must not be reachable"),
    }), []);
  }
});

test("an unsupported client version does not offer a third-party update link", () => {
  const presentation = managedMcpIntegrationPresentation("Jan", {
    integrationInstalled: false,
    state: "app-update-required",
    appVersion: "1.0",
  } as never, true, undefined);

  assert.equal(presentation.primary, undefined);
  assert.deepEqual(localRowButtons(presentation, {
    install: () => assert.fail("third-party update action must not be reachable"),
  }), []);
});

test("the Windows UI and native command surface contain no client download routes", async () => {
  const root = new URL("../", import.meta.url);
  const files = [
    "src/App.tsx",
    "src/menu-bar.tsx",
    "src/tauri-transport.ts",
    "src-tauri/src/client_packages.rs",
    "src-tauri/src/lib.rs",
  ];
  const forbidden = [
    "open_claude_desktop_download",
    "open_chatgpt_desktop_download",
    "open_antigravity_download",
    "open_lm_studio_download",
    "open_jan_download",
    "claude.ai/download",
    "chatgpt.com/download",
    "antigravity.google/download",
    "lmstudio.ai/download",
    "jan.ai/download",
  ];

  for (const file of files) {
    const source = await readFile(fileURLToPath(new URL(file, root)), "utf8");
    for (const token of forbidden) assert.equal(source.includes(token), false, `${file} still contains ${token}`);
  }
});
