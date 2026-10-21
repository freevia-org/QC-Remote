# QC Remote device broker

The broker runs the shared `quadcortex` session, command, framing, state,
verification and backup engines. Windows and Linux link this crate into one
resident broker/tray/MCP executable. The standalone binary in this crate also
supports headless operation through the same authenticated loopback transport.

The Linux desktop host, resident tray/MCP broker and performance MIDI backend
are implemented. Historical hardware results below cover the headless broker;
they do not establish the new desktop tray or performance MIDI behavior on
hardware.

## Native Linux desktop and resident host

The desktop source retains its shared `apps/windows` path. It builds as a
native Tauri/WebKit app on Linux. Its sibling `qc-device-broker` links the tray,
USB owner and background MCP host in one process. A browser development preview
does not provide native broker connectivity.

On Ubuntu, install the native build prerequisites:

```sh
sudo apt-get install build-essential pkg-config libwebkit2gtk-4.1-dev \
  libgtk-3-dev libxdo-dev libssl-dev libayatana-appindicator3-dev \
  librsvg2-dev libdbus-1-dev libsecret-1-dev
```

With Node 22 and Rust stable available, build from the repository root:

```sh
npm ci
npm run build --workspace @qc-remote/windows
TAURI_CONFIG='{"bundle":{"externalBin":[]}}' \
  cargo +stable build --locked --manifest-path apps/windows/src-tauri/Cargo.toml \
  --features tray-sidecar,custom-protocol --bins
./apps/windows/src-tauri/target/debug/qc-remote
```

The explicit toolchain overrides the app directory's Windows MSVC toolchain.
`custom-protocol` embeds the built frontend, so this executable needs no Vite
server. The Cargo-only override disables external-binary packaging, not the
host build: keep both executables together. Linux package metadata is in
`apps/windows/src-tauri/tauri.linux.conf.json`; these commands do not produce
an installer.

### Build an unsigned Debian package

On a native Ubuntu/Debian desktop build host with the prerequisites above,
Node 22, Rust stable and `dpkg-deb` installed, first install the npm workspace
dependencies with `npm ci`, then run this from the repository root:

```sh
python3 scripts/package-linux-deb.py
```

The script builds the broker for the host target, stages it under Tauri's
required target-suffixed external-binary name, and asks Tauri to build only the
`.deb`. It removes only the sidecar file it created, including on build or
verification failure. It then checks that the package contains both
executables and the scoped udev rules at
`/usr/share/qc-remote/70-qc-remote.rules`. An existing target-suffixed sidecar
is a hard error and is never replaced. For a bounded smoke package from an
already-built pair, pass both executables and a new output path:

```sh
python3 scripts/package-linux-deb.py \
  --prebuilt-app /path/to/qc-remote \
  --broker-binary /path/to/qc-device-broker \
  --output /path/to/new/qc-remote-preview.deb
```

That path uses a fresh temporary Cargo target directory and Tauri's debug
bundle mode, does not compile either executable, and checks the exact package
members and both executable SHA-256 hashes before copying the verified `.deb`
to the new output path. The command prints the verified package path. Package
outputs are unsigned and are not published or release-signed installers.

The package ships the rules file but does not install it into `/etc`. After
installing the package, an administrator may install the narrowly scoped rules
and reload udev:

```sh
sudo install -m 0644 /usr/share/qc-remote/70-qc-remote.rules /etc/udev/rules.d/70-qc-remote.rules
sudo udevadm control --reload-rules
```

Reconnect the device after reloading. The rule grants active-seat access only
to the supported product IDs on hidraw and raw MIDI; it does not change kernel
driver binding.

Launch in a graphical user session with D-Bus and a StatusNotifierItem tray
host. A desktop without a tray host cannot display the broker icon. The broker
must register its icon before applying a tray idle hold. Tray preferences and
MCP grants share the persistent directory described below. Enabling login
startup writes an owned entry under `${XDG_CONFIG_HOME:-$HOME/.config}/autostart`.
The background host also supports `--resident`; the standalone headless build
does not link the MCP host.

The app's LOCAL panel provides the native STDIO command for other MCP clients.
It uses `qc-remote --local-mcp-bridge`, which attaches to the same host. Linux
pairing credentials use the desktop's Secret Service keyring; an unavailable
or locked keyring produces an error rather than storing credentials in plain
text. Tray confirmation dialogs use `zenity` or `kdialog` when installed.

## Build and test on Linux

Install stable Rust and a C toolchain. From the repository root:

```sh
cargo build --locked -p qc-device-broker
cargo test --locked -p qc-device-broker
cargo clippy --locked -p qc-device-broker --all-targets -- -D warnings
python3 -m unittest scripts/test_package_linux_deb.py
```

The Python tests exercise package-member/hash verification, exclusive sidecar
staging and process/temp cleanup; they do not invoke Cargo, npm, a package
manager or device I/O.

The Linux backend uses hidraw. Do not install a libusb driver or detach the
kernel's USB audio/MIDI driver. Hardware permissions are a separate step;
building and the automated tests require no attached Quad Cortex.

## Measured Linux HID completion

The Linux writer has one measured control-status `EPIPE` exception, scoped to
VID/PID `152a:880a`, HID interface 5, and its exact 32-byte report descriptor.
A hardware probe sent one complete 129-byte type-52 `ResetCommsBuffers`
handshake report. hidraw returned `EPIPE`, and the device then replied with a
type-52 message containing the exact fresh session UUID; the optional request
ID was absent. The shared session layer still requires that fresh UUID echo
before startup can advance. Other Linux device profiles, short writes,
and other errors remain fail-closed. A follow-up live broker test completed
synchronization in 13.245 seconds, exchanged 33 outbound and over 440 inbound
logical messages, served two authenticated clients and a session-tagged
snapshot, and notified both clients before a clean SIGTERM exit. No
preset/settings mutation or audio/MIDI stream was exercised; the other QC
product ID `892f` remains untested.

## Permissions

For a systemd desktop session, install the supplied rules and reconnect the QC:

```sh
sudo install -m 0644 services/device-broker/linux/70-qc-remote.rules /etc/udev/rules.d/70-qc-remote.rules
sudo udevadm control --reload-rules
```

The rule grants the active local user's seat access to the two supported QC
product IDs (`152a:880a` and `152a:892f`) on hidraw and raw MIDI only. It does not
grant access to every HID or sound device. Check the actual node with `udevadm info --attribute-walk
--name=/dev/hidrawN` and `getfacl /dev/hidrawN`; do not assume its number remains
stable after reconnect. This follows the kernel's [hidraw interface](https://docs.kernel.org/hid/hidraw.html)
and systemd's [uaccess rules](https://github.com/systemd/systemd/blob/main/rules.d/70-uaccess.rules.in).

An SSH-only account is not an active local seat. An administrator can instead
create a dedicated `qc-remote` group, add only the broker user, and replace the
rules' `TAG+="uaccess"` with `GROUP="qc-remote", MODE="0660"`. Log in again
after changing group membership and reconnect the QC after reloading rules.
Do not run the broker as root or make hidraw nodes world-writable to bypass a
permission error. The supplied rule is not installed by a build or test.

## Headless use

Run as your ordinary user. The default discovery directory is
`$XDG_STATE_HOME/org.freevia.qcremote`, then
`$HOME/.local/state/org.freevia.qcremote`, or
`$XDG_RUNTIME_DIR/org.freevia.qcremote` when the preceding bases are unset.
Relative base paths are ignored. If none is available, it uses a private
`qc-remote-<uid>` directory under the system temporary directory.
Use an absolute, user-owned path. The broker's directory is mode `0700` and its
credential descriptor mode `0600`. Existing unsafe ownership, symlinks or
permissions are refused rather than repaired implicitly.

If startup reports an unsafe broker or XDG autostart directory component,
inspect the exact path named by the error and its parents with `namei -l` and
`stat -c '%U %a %n' PATH`. Confirm each directory is owned by your account and
is not a symlink before changing its permissions. If a named directory is
your own and only group/world write bits are the problem, remove those bits
from that exact directory with `chmod go-w PATH`, then retry. Do not run a
recursive permission change or weaken the broker's ownership checks; do not
change a shared/system-owned path.

For a usual systemd login:

```sh
./target/debug/qc-device-broker --self-check
./target/debug/qc-device-broker --broker-serve \
  --descriptor-path "${XDG_STATE_HOME:-$HOME/.local/state}/org.freevia.qcremote/device-broker.json" \
  --no-tray
```

Keep a client attached to keep this nonresident broker alive. It starts
disconnected. Add `--auto-connect` only for an intentional hardware session, or
ask it to connect through the existing RPC. With no authenticated participant
the shared five-second grace releases the device and exits. This headless
command registers no icon and never acquires a tray idle hold.

A client reads the private descriptor, connects to `127.0.0.1:<port>`, and sends
a four-byte big-endian length followed by this JSON as its first frame:

```json
{"method":"broker.authenticate","params":{"token":"<bearerToken from descriptor>"},"id":0}
```

Require the reply's `result.instanceId` to equal the descriptor's `instanceId`.
Subsequent frames use the existing JSON-RPC surface, for example
`{"jsonrpc":"2.0","id":1,"method":"system.status","params":{}}`.
Keep credentials out of command-line arguments, shell history and logs. Two
launches using the same directory elect one broker. Different directories do
not form one election; always use the same discovery directory for every client.

A device with no advertised serial uses a conservative identity tied to its
current hidraw node. After unplug/replug, explicitly select it again; the
broker must not silently redirect commands to a unit that reuses that node.

The short-lived control client discovers that same default directory:

```sh
./target/debug/qc-device-broker --control status
./target/debug/qc-device-broker --control quit --confirm
```

`release` closes USB without exiting the broker or detaching its clients,
even while a window or MCP client is attached. Ordinary requests cannot undo
that explicit Disconnect; `connect` reopens the session. Automatic idle release
is different: the next device request may reopen an idle-released session.

`quit --confirm` releases USB and records the user's Quit. MCP bridges observing
that broker generation close their MCP transport and exit; an open app window
stays open and disconnected until the user chooses Connect.
SIGTERM and SIGINT perform bounded shared shutdown and release the descriptor,
without recording a user Quit. A killed/crashed owner cannot clean up its files;
the next election reclaims only a demonstrably dead, unlocked claim. It never
kills another process. The bearer file is a boundary between users, not between
two processes running as the same user.

## Current boundaries

HID report I/O and Linux process/path services are platform adapters. The
Windows native HID concurrency implementation and Windows process safeguards
remain behind their Windows backends. Linux performance MIDI resolves the
selected HID identity to its physical USB ancestor, requires one matching ALSA
raw MIDI endpoint and rechecks the opened node before bounded writes. It never
chooses an output by its display name or substitutes HID for MIDI. Audio stays
on the Quad Cortex and the OS audio stack. Packaged installers are separate
from the native source/build validation described here.

Automated native Linux tests cover disconnected authenticated RPC, duplicate
launches, rejected authentication/framing, file modes, crash recovery,
SIGTERM and control-quit cleanup, including test-harness failure cleanup.
They do not exercise physical-device I/O. The live hardware checks above are
separate evidence and do not establish the full physical-device checklist.
