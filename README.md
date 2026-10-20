# QC Remote

Control a Quad Cortex from a Windows PC, Linux desktop, Android phone, or Apple
mobile device, or let
a compatible AI app inspect the rig, make changes, and verify the result through
guarded MCP tools. QC Remote is free, open-source software from Freevia.

[Website](https://freevia.org/qcremote/) ·
[User guide](docs/USER-GUIDE.md) ·
[Latest release](https://github.com/freevia-org/QC-Remote/releases/tag/v0.1.188) ·
[Support & feedback](SUPPORT.md) ·
[Privacy](PRIVACY.md)

<p align="center">
  <img src="https://freevia.org/assets/qcremote/windows-app-real.png" alt="QC Remote for Windows showing the 7F Money for Nothing preset on a connected Quad Cortex" width="700">
  <img src="https://freevia.org/assets/qcremote/android-app-money-for-nothing.png" alt="QC Remote for Android showing the same 7F Money for Nothing preset" width="175">
</p>

*Real Windows and Android app captures of the same preset. The [website](https://freevia.org/qcremote/) and this README use the same image URLs, so the presentation screenshots cannot diverge. The [user guide](docs/USER-GUIDE.md) uses separate, clearly labeled demo-mode screenshots for its step-by-step instructions.*

## Download

| Platform | Current release | Primary download | Other channel |
| --- | --- | --- | --- |
| Windows 10/11, x64 | 0.1.188 (GitHub); Microsoft Store 0.1.188 in review | [Signed GitHub installer](https://github.com/freevia-org/QC-Remote/releases/download/v0.1.188/QC-Remote-Windows-0.1.188-x64-setup.exe) | [Microsoft Store](https://apps.microsoft.com/store/detail/XPFP2G7DPVK1C5) (still serves an older build) |
| Ubuntu 24.04, x86_64 | 0.1.187 | [Debian package](https://github.com/freevia-org/QC-Remote/releases/download/v0.1.187/qc-remote_0.1.187_amd64.deb) | — |
| Android 7.0+, USB host/OTG | 0.3.38 (GitHub APK and Google Play internal testing); public Google Play listing still serves 0.3.35 | [Signed GitHub APK](https://github.com/freevia-org/QC-Remote/releases/download/v0.1.188/qc-remote-0.3.38.apk) | [Google Play](https://play.google.com/store/apps/details?id=org.freevia.qcremote) · [Invited tester opt-in](https://play.google.com/apps/internaltest/4701527491652130240) |
| iPhone and iPad | Development preview; not released | Build from [`apps/ios`](apps/ios/README.md) | — |

The iPhone app provides one-way USB-MIDI controls only. Full direct USB control
is an experimental iPad DriverKit prototype for supported M-series iPads; it
requires Apple-approved DriverKit entitlements and hardware validation. See
[`apps/ios/README.md`](apps/ios/README.md) before building or using it.

Windows 0.1.188 and Android 0.3.38 are available from
[GitHub release v0.1.188](https://github.com/freevia-org/QC-Remote/releases/tag/v0.1.188). Windows 0.1.188 has been submitted to Microsoft Store and is in review; the Store currently serves an older build. Android 0.3.38 is on Google Play's internal testing track; the public Play listing still serves 0.3.35.
Linux 0.1.187 is for Ubuntu 24.04 x86_64 with signed GitHub build provenance. Exact-package testing passed for installation, native app/tray, MCP, and connected-device control and recovery on Ubuntu 24.04 WSL2/Xfce with a physical Quad Cortex over USB/IP. Bare-metal Linux has not been tested. See the [Linux 0.1.187 release notes](https://github.com/freevia-org/QC-Remote/releases/tag/v0.1.187), [Linux setup guide](docs/USER-GUIDE.md#linux), and [package verification instructions](docs/USER-GUIDE.md#verify-the-linux-download).

QC Remote was developed and tested with Quad Cortex firmware (CorOS) 4.1.0. It
does not contain, replace, or modify Quad Cortex firmware, and it does not
replace Neural DSP's own Cortex Control. Its device screens are independent
reconstructions made for interoperability, not Neural DSP assets. QC Remote is
independent and unofficial; it is not affiliated with, authorized, sponsored,
endorsed, or supported by Neural DSP Technologies Oy. See
[`TRADEMARKS.md`](TRADEMARKS.md).

## What you can do

- Control the device manually: switch Scenes, work on signal chains, edit
  parameters, and organize presets and setlists.
- Explore the virtual device in demo mode, online or offline, without hardware.
- Connect a local MCP-compatible desktop AI app on Windows or Linux, or use an MCP
  connector through the optional remote relay from a Windows or Android host.
- Set the AI access level in QC Remote: Read-only, Performance, Modify, or Full.
  The host enforces that ceiling independently of the AI app.

## Using QC Remote

The [user guide](docs/USER-GUIDE.md) explains what you need, how to connect a
Quad Cortex over USB, what the connection indicators mean, the access levels
and safety checks, how to connect an AI assistant, and what to try when
something does not connect. The Android app's requirements and USB permission
behavior are summarized in [`apps/android/README.md`](apps/android/README.md).

## Connecting an AI assistant (MCP)

QC Remote lets an AI assistant read and control a connected Quad Cortex
through the Model Context Protocol, in one of two ways:

- **On the same computer (Windows).** Open the **LOCAL** indicator in the app
  and choose your assistant. Claude Desktop gets a one-click extension;
  ChatGPT Desktop/Codex, Google Antigravity, Bionic (formerly LM Studio) and
  Jan are configured automatically; any other MCP client gets a copyable stdio
  configuration. The connection stays on the computer, although the assistant
  itself sends what it receives to its own provider. Details:
  [`apps/windows/README.md`](apps/windows/README.md).
- **From anywhere (Windows or Android).** Open the **REMOTE** indicator and
  pair the app with the relay, signing in with Google. Then add
  `https://qcrc.freevia.org/mcp` as a custom connector in Claude or ChatGPT.
  The relay is the Cloudflare Worker in
  [`services/qc-remote-cloud`](services/qc-remote-cloud), operated by Freevia;
  what it receives and keeps is described in [`PRIVACY.md`](PRIVACY.md).

Both routes enforce the access tier selected in the app (Read-only,
Performance, Modify, or Full). On either route, the assistant must confirm
deleting a preset or a setlist with you in the AI conversation; that is the
only confirmation, and the paired app does not ask a second time.

## What this repository contains

- **`apps/windows/`** — the Tauri (Rust + React) desktop app for Windows and
  Linux, with guarded live device control, workspaces, typed commands, and
  explicit device-save confirmation. Windows ships the broker as a sidecar;
  the Linux package runs that same broker as a resident host.
- **`apps/android/`** — a Capacitor (Java + React) mobile companion
  app, talking to the device over USB via a JNI binding to the shared native
  library.
- **`services/rust-mcp/`** (crate `qc-remote-mcp`) — the Model Context
  Protocol server that lets an AI assistant read and control a connected Quad
  Cortex through a device-backend-agnostic trait. The Windows app embeds it.
- **`services/device-broker/`** (crate `qc-device-broker`) — the native
  single-owner USB/HID broker library that holds the exclusive device
  connection and resident tray. Windows and Linux each ship its platform-native
  sidecar built from `apps/windows/src-tauri`.
- **`services/qc-remote-cloud/`** — the Cloudflare Worker relay that pairs a
  device host with a remote client for control away from the local network.
- **`packages/rust/quadcortex/`** — the unified Rust library for the Quad
  Cortex wire protocol, device state, sessions, transport, and backups.
  Its protobuf bindings and message registry are generated from the
  project-maintained protocol schema and checked for drift. A
  [guide and tutorial](packages/rust/quadcortex/docs/guide/index.md) with
  compile-checked examples is built into its API documentation.
- **`packages/rust/qc-windows-midi/`**, **`packages/rust/qc-windows-process/`**,
  **`packages/rust/quadcortex-windows/`**, **`packages/rust/qc-owner-only-file/`**,
  **`packages/rust/qc-relay-client/`**, **`packages/rust/qc-relay-protocol/`**,
  **`packages/rust/quadcortex-android/`**, **`packages/rust/qc-broker-framing/`**
  — supporting Rust crates: Windows MIDI transport, Windows process-liveness
  checks, the Windows USB/HID transport the broker uses, owner-only credential
  file storage, the outbound relay client, its shared wire frames, the Android
  JNI binding, and the length-prefixed framing that local broker clients and
  servers share.
- **`packages/typescript/{qc-client,qc-core,qc-form-factors,qc-theme,qc-ui}`**
  — the shared frontend layer used by both apps: generated action/gateway
  bindings, device-state reducers, form-factor-aware layout, the shared
  theme, and UI components.
- **`skills/qc-device-knowledge/`** — a Claude/Codex-compatible skill bundling
  structured compatibility information from publicly available Quad Cortex
  product documentation. The skill and its catalogue data are not covered by
  the open-source license (see [`LICENSE`](LICENSE), Schedule A). Built into
  the MCP server (`services/rust-mcp/src/server.rs` embeds it directly) and
  installable into local AI-assistant configurations by the Windows app.
- **`contracts/public/`** — the JSON contracts the apps, services, and library
  are generated from and tested against: the device action catalogue
  (`qc-actions.v1.json`), the gateway method list (`gateway-methods.v1.json`),
  the device domain model (`qc-domain.v1.json`), the relay wire profile
  (`qc-relay-profile.v1.json`), the broker and tray schemas
  (`native-broker.v1.schema.json`, `broker-tray-surface.v1.json`), and the host
  policy (`qc-host-policy.v1.json`: the suspended-device and uncertain-effect
  refusal markers, the preset-directory refresh message, and the dispatch kinds
  the Android host serves from its suspension cache). The
  `generated_*`, `generated-*` and `Generated*` files in the tree are snapshots
  of these contracts. Their generators and additional reference specifications remain private.
  Run `npm run contracts:check` for public value-level drift checks, and propose contract changes in an issue or pull request
  rather than editing the generated files by hand.

## Building

### Prerequisites

| Tool | Version | Needed for |
| --- | --- | --- |
| Node.js and npm | 22.6 or newer (the tests use `--experimental-strip-types`) | TypeScript packages, both apps' web layers, the Tauri and Capacitor CLIs, Wrangler (each installed by `npm install`) |
| Rust (rustup) | stable, at least 1.90 for the Windows app (see "Rust version floors" below); on Windows the MSVC toolchain (`stable-x86_64-pc-windows-msvc`), which `apps/windows/src-tauri/rust-toolchain.toml` pins | the Rust workspace, the Windows app, the Android native library |
| Visual Studio Build Tools (Desktop development with C++) and WebView2 | current | the Windows app (MSVC linker, Tauri runtime) |
| PowerShell 7 (`pwsh`) | 7 or newer | standalone Windows native validation (`scripts/test-windows-native.mjs`) and the Android native build script off Windows. Windows native validation refuses before Cargo dispatch if PowerShell 7 is unavailable. This is a development/validation prerequisite, not a prerequisite for running QC Remote. Gradle and the Windows-only `android:build:debug` and `branding:generate` scripts still use inbox Windows PowerShell. |
| JDK | 21 (Temurin in CI) | Android |
| Android SDK | platform 36, build-tools 36.0.0 | Android |
| Android NDK | 27.2.12479018, with `ANDROID_NDK_HOME` pointing at it | the Android native library |
| cargo-ndk | 4.1.2, with the Rust targets `aarch64-linux-android` and `x86_64-linux-android` | the Android native library |

For Android, install the last two with
`cargo install cargo-ndk --version 4.1.2 --locked` and
`rustup target add aarch64-linux-android x86_64-linux-android`.

#### Rust version floors

The Rust workspace and the Windows app are separate Cargo graphs with
different declared compiler requirements. CI builds both on current stable and
checks only the `quadcortex` library on its declared minimum.

| Graph | Declared lower bound | Where it comes from |
| --- | --- | --- |
| `apps/windows/src-tauri` (Windows app) | Rust 1.90 | `tauri-utils` 2.10.1 (locked in `apps/windows/src-tauri/Cargo.lock`) declares `edition = "2024"` and `rust-version = "1.90"`. The app manifest declares no `rust-version` of its own. |
| Root workspace (`cargo build --workspace`) | At least Rust 1.88 | `services/rust-mcp/Cargo.toml` declares `rust-version = "1.88"`. This component declaration does not establish the entire workspace's effective minimum on every target. |
| `quadcortex` library alone | Rust 1.87 | `packages/rust/quadcortex/Cargo.toml` declares `rust-version = "1.87"`; `.github/workflows/ci.yml` (`rust-msrv`) checks it. |

Only the `quadcortex` minimum is tested by a dedicated CI job. The other rows
record known lower bounds, not successful builds on those compiler versions.
Re-check the complete target-specific dependency graph after lockfile updates.

### Rust workspace

```sh
cargo build --workspace
cargo test --workspace
```

On pull requests, CI builds and tests the Rust workspace on `windows-latest`
only; several crates are Windows-specific. A second job also tests and documents
the `quadcortex` crate on `ubuntu-latest`. A push to `master` runs only the
public-history audit.

### Windows native shell tests

The shell is a separate Cargo workspace. On Windows, run its canonical
original-process-owned launcher from the repository root:

```sh
node scripts/test-windows-native.mjs --root .
```

Besides preparing the exact broker and harness artifacts, this launcher owns
an exclusive scratch directory for test health records and inherited broker
stderr. It removes those bytes only after its original process tree and host
have acknowledged retirement. An unacknowledged descendant retains the same
attribution and scratch directory rather than letting a thread-local cleanup
delete a live broker's diagnostics.

Standalone `cargo test` calls that reach Windows gateway/health/stderr
admission without this owned state now fail with the launcher command before
dispatch. Pure tests that never request that state are unaffected. A custom
focused runner must reuse `scripts/windows-owned-producer.mjs` with
`nativeTestState: true`, as well as the canonical artifact/environment setup;
merely setting an inherited scratch-path variable is not proof of cleanup.
This development contract is not an OS security boundary and does not change
installed application paths or non-Windows test behavior.

### TypeScript packages and apps

```sh
npm install
npm run build
```

`npm run build` builds every workspace that defines a `build` script
(`apps/windows`, `apps/android`, and any `packages/typescript/*` package that
has one). To build or typecheck a single workspace:

```sh
npm run build --workspace @qc-remote/windows
npm run typecheck --workspace @qc-remote/android
```

### Windows app

`apps/windows/src-tauri` is its own Cargo workspace (a different toolchain
and build-script setup than the rest of the Rust code) and is built
separately. Its Tauri configuration bundles one sidecar executable,
`qc-device-broker.exe`: the device broker, its notification icon and the
background MCP host in one process. Its build script refuses to run until it
exists under `apps/windows/src-tauri/binaries/`. A bare `cargo build` there
fails with ``resource path `binaries\qc-device-broker-x86_64-pc-windows-msvc.exe`
doesn't exist`` until it does.

For a check-only build, stage empty placeholders, as CI does:

```powershell
cd apps/windows/src-tauri
New-Item -ItemType File -Force binaries/qc-device-broker-x86_64-pc-windows-msvc.exe
cargo check --locked
```

For a runnable build, build the real sidecar and copy it over the
placeholder. The shipped broker links the app's MCP host, so it is built from
this crate (not from `services/device-broker`, whose own binary is a
broker-only developer build without the host); its build needs the placeholder
in place first, and it needs the `tray-sidecar` feature:

```powershell
# From apps/windows/src-tauri, with the placeholder above in place
cargo build --release --locked --bin qc-device-broker --features tray-sidecar
Copy-Item target/release/qc-device-broker.exe binaries/qc-device-broker-x86_64-pc-windows-msvc.exe
```

(If you set `CARGO_TARGET_DIR`, copy from there instead of `target/`.) Then
package the app; the NSIS installer is written under
`apps/windows/src-tauri/target/release/bundle/nsis/`:

```powershell
npm run tauri:build --workspace @qc-remote/windows
```

For development, `npm run tauri:dev --workspace @qc-remote/windows` runs the
app against the Vite dev server, with the same sidecar requirement.

### Android app

With the prerequisites above installed:

```sh
npm install
npm run android:sync --workspace @qc-remote/android
cd apps/android/android
sh ./gradlew assembleDebug        # on Windows: .\gradlew.bat assembleDebug
```

`android:sync` builds the web layer and copies it into the Gradle project with
Capacitor. Gradle's `preBuild` then cross-compiles
`packages/rust/quadcortex-android` into the APK through
`scripts/build-android-rust.ps1`. The debug APK is written to
`apps/android/android/app/build/outputs/apk/debug/`. Release signing is not
part of this repository.

### Cloud relay

```sh
npm install
npm run cloud:check   # wrangler types + tsc --noEmit + wrangler deploy --dry-run
```

The relay is an npm workspace of the root `package.json`, so its dependencies
are locked by the root `package-lock.json`; it has no lockfile of its own. Run
`npm ci` and `npm install` from the repository root. `npm ci` run from inside
`services/qc-remote-cloud` prunes the root `node_modules` to that one
workspace.

## License, contributing, and security

- [`LICENSE`](LICENSE) — what is licensed under `MIT OR Apache-2.0`, and what
  is not: brand assets, third-party components, and device-derived and
  third-party-sourced material (reference catalogues, transcribed screen
  content and traced device iconography in Schedule A, screen reconstructions
  in Schedule B), each listed as whole files. Those files are covered by
  [`LICENSES/LicenseRef-QC-Remote-Reference.txt`](LICENSES/LicenseRef-QC-Remote-Reference.txt),
  and a package that contains, embeds, links, or bundles one declares
  `(MIT OR Apache-2.0) AND LicenseRef-QC-Remote-Reference`.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — the inbound contribution policy
  (Developer Certificate of Origin 1.1).
- [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) — the Contributor Covenant 2.1.
- [`SECURITY.md`](SECURITY.md) — how to report a vulnerability.
- [`PRIVACY.md`](PRIVACY.md) — what the optional relay receives and keeps.
- [`TRADEMARKS.md`](TRADEMARKS.md) — compatibility and trademark notice.
- [`THIRD_PARTY-NOTICES.md`](THIRD_PARTY-NOTICES.md) — third-party components
  bundled with this repository's builds.
