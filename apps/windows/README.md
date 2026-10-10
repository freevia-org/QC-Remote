# QC Remote for Windows

Tauri 2 desktop composition root for the Quad Cortex device surface, application menus, keyboard and mouse input, local/remote MCP access, settings, and native gateway lifecycle.

The Windows app uses the same `@qc-remote/ui` screens and `@qc-remote/core` behavior as Android. Windows-specific code owns desktop lifecycle, menus, workspace integration, MCP client packaging, and the adapter from the stable gateway transport to the shared device port.

## Using the app

To install, connect a Quad Cortex, read the USB, LOCAL and REMOTE indicators, or
pair an AI assistant, see the end-user guide:
[`docs/USER-GUIDE.md`](../../docs/USER-GUIDE.md). The app also has its own
guide under **Help > User Guide** (F1). This file describes the app for
developers and maintainers.

## Current slice

- scalable QC chassis and CorOS-style control surface;
- live scenes, footswitches, tempo, block editing, preset browsing, routing, and parameter editing;
- local `.qcw` workspaces and confirmed device-save workflows;
- connection diagnostics, safe reconnect/reset, and redacted diagnostic export;
- optimistic device updates with authoritative reconciliation and rollback;
- a **LOCAL** indicator for local MCP integrations, including direct Claude Desktop extension install/update/version detection, native zero-dependency ChatGPT Desktop/Codex, Google Antigravity, Bionic (formerly LM Studio) and Jan routes, a copyable stdio configuration for any other MCP client, and manual setup packages for supported clients and VS Code;
- a **REMOTE** indicator for QR pairing, outbound relay status, reconnect/unpair actions, and relay-reported client/platform activity;
- a **USB** indicator for the physical device session.

Every pairing but Claude Desktop runs the installed QC Remote executable itself with `--local-mcp-bridge`, so none of them needs Node. Every managed route also names its host twice -- `--local-mcp-client <client id>` on the command line and `QC_REMOTE_LOCAL_MCP_CLIENT` in `env` -- because a host that keeps the configured environment to itself leaves the bridge announcing no client, and LOCAL then cannot report a working pairing as connected:

| Pairing | What it writes | Where |
| --- | --- | --- |
| Claude Desktop | a `.mcpb` extension carrying the JavaScript bridge `server/index.cjs` | Claude Desktop's own extension store, via Claude Desktop |
| ChatGPT Desktop / Codex | the `qc-remote` entry only, and the `qc-device-knowledge` skill | `%USERPROFILE%\.codex\config.toml`, `%USERPROFILE%\.codex\skills\qc-device-knowledge` |
| Google Antigravity | an MCP route that runs the installed QC Remote executable, a tool-permission entry, and the `qc-device-knowledge` skill (pairing also sweeps the versioned executable copies that releases through 0.1.142 left under `%LOCALAPPDATA%\QC Remote\integrations\`) | `%USERPROFILE%\.gemini\config\mcp_config.json`, `%USERPROFILE%\.gemini\config\config.json`, `%USERPROFILE%\.gemini\config\skills\qc-device-knowledge` |
| Bionic (formerly LM Studio) | Bionic: the `servers` entry named `qc-remote` only, with `enabled: true`; pre-rename LM Studio 0.3.x: the `qc-remote` entry only, with `disabled: false`; and the `qc-device-knowledge` skill | Bionic: `%USERPROFILE%\.lmstudio\apps\bionic\.internal\ng-mcp.json`; LM Studio 0.3.x: `%USERPROFILE%\.lmstudio\mcp.json`; skill: `%USERPROFILE%\.lmstudio\skills\qc-device-knowledge` |
| Jan | the `qc-remote` entry only, with `active: true` | `mcp_config.json` in Jan's data directory (`%APPDATA%\Jan\data` unless `settings.json` moves it) |
| Other MCP stdio | nothing -- the configuration is copied to the clipboard | wherever the user pastes it |
| VS Code, manual Codex | example configurations inside the downloadable setup bundle | wherever the user puts them |

Bionic is gated per numbering series: every 1.x Bionic build qualifies, and a pre-rename LM Studio install still requires 0.3.17 or newer. Jan requires 0.6.10 or newer. Below the minimum for its own series, LOCAL offers an app update instead of pairing. The row is headed by the application the installed version belongs to - Bionic, or LM Studio for the legacy 0.3.x line - so the name and the update verdict always come from the same reading. Each managed route is backed up before replacement and leaves unrelated MCP servers untouched. Unpair removes only QC Remote's own entry and, where pairing installed it, the `qc-device-knowledge` skill it manages, restoring any copy pairing had backed up.

The bundled `qc-device-broker` is the only process that opens QC HID. The app exposes authenticated loopback MCP through that broker and can establish an outbound-only authenticated relay. Both paths enforce the selected Read-only, Performance, Modify, or Full access tier.

## Run

From the repository root, after `npm install`, and with the
`qc-device-broker` sidecar executable staged as the root
[README](../../README.md#windows-app) describes:

```powershell
npm run tauri:dev --workspace @qc-remote/windows
```

For the webview alone, without the native shell:

```powershell
npm run dev --workspace @qc-remote/windows
```

## Build and validate

```powershell
npm run typecheck --workspace @qc-remote/windows
npm run build --workspace @qc-remote/windows
npm run tauri:build --workspace @qc-remote/windows
```

`tauri:build` produces the NSIS installer under
`apps/windows/src-tauri/target/release/bundle/nsis`; it needs the real
`qc-device-broker` sidecar (the device broker, its notification icon and the
background MCP host in one executable) in `apps/windows/src-tauri/binaries`
first (see the root README). Windows native builds use the MSVC toolchain declared in
`apps/windows/src-tauri/rust-toolchain.toml`.

For native tests, run `node scripts/test-windows-native.mjs` from the public
repository root. This builds the broker and library test harness once, then
passes their exact Cargo artifact paths to the tests. Unit tests never launch
Cargo themselves. The runner disables release-sidecar packaging only in its
child processes and preserves the caller's environment.

`apps/windows/package.json` is the desktop version source of truth; it is kept
in step with `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`. The
signed release pipeline and its version-bump tooling are maintainer tooling
and not part of this repository.
