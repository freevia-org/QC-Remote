# macOS broker and desktop builds

The desktop uses `apps/windows` on macOS too. Install full Xcode with its
command-line tools, Node.js 22.6 or newer, and stable Rust through rustup.
Install the repository's JavaScript dependencies with `npm ci` at the root.

The desktop crate pins the Windows MSVC toolchain; override it for macOS:

```sh
export RUSTUP_TOOLCHAIN=stable
cd apps/windows
# Apple Silicon; use x86_64-apple-darwin on Intel.
target=aarch64-apple-darwin
TAURI_CONFIG='{"bundle":{"externalBin":[]},"build":{"frontendDist":"../"}}' \
  cargo build --release --locked --manifest-path src-tauri/Cargo.toml \
  --bin qc-device-broker --features desktop-broker --target "$target"
src-tauri/target/$target/release/qc-device-broker --self-check
mkdir -p src-tauri/binaries
cp src-tauri/target/$target/release/qc-device-broker \
  src-tauri/binaries/qc-device-broker-$target
npm run tauri:build:mac
```

The npm command builds the standalone menu app before packaging the desktop.
It exports the Windows broker's LED renderer for native macOS images. The
menu app compiles for the host architecture and includes that host's broker.

Outputs:

- `.artifacts/builds/QC Device Broker.app`: install in Applications to run
  independently of the editor.
- `apps/windows/src-tauri/target/release/bundle/macos/QC Remote.app`.
- `apps/windows/src-tauri/target/release/bundle/dmg/`: desktop installer.

QC Remote also embeds the menu app for startup and login-item registration.
The broker LED appears in the macOS menu bar. Reopening QC Device Broker
opens its menu; quitting QC Remote leaves the broker running. Use **Quit**
in the broker menu to disconnect clients and stop the broker.

macOS renders the same presentation as Windows: live preset, MCP endpoint,
client/session submenus, unlink confirmations, idle-release choices, and
Connect/Disconnect. Actions use the authenticated control connection and the
existing broker tray state thread. A stale or disabled action is rejected;
remote unlink requires confirmation. Menu polling does not count as an
attached client or renew the idle clock.

Local builds use ad-hoc signatures. Distribution signing and notarization need
appropriate Apple credentials.
