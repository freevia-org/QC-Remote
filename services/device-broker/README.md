# QC Remote device broker

The broker runs the shared `quadcortex` session, command, framing, state,
verification and backup engines. Linux uses the headless binary in this crate;
Windows also links this crate into its existing broker/tray/MCP executable.
The Linux milestone exposes the existing authenticated loopback RPC transport.
It does not yet supply a Linux desktop shell or an MCP host.

## Build and test on Linux

Install stable Rust and a C toolchain. From the repository root:

```sh
cargo build --locked -p qc-device-broker
cargo test --locked -p qc-device-broker
cargo clippy --locked -p qc-device-broker --all-targets -- -D warnings
```

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
product IDs (`152a:880a` and `152a:892f`) on hidraw only. It does not grant access
to every HID device. Check the actual node with `udevadm info --attribute-walk
--name=/dev/hidrawN` and `getfacl /dev/hidrawN`; do not assume its number remains
stable after reconnect. This follows the kernel's [hidraw interface](https://docs.kernel.org/hid/hidraw.html)
and systemd's [uaccess rules](https://github.com/systemd/systemd/blob/main/rules.d/70-uaccess.rules.in).

An SSH-only account is not an active local seat. An administrator can instead
create a dedicated `qc-remote` group, add only the broker user, and replace the
two rules' `TAG+="uaccess"` with `GROUP="qc-remote", MODE="0660"`. Log in again
after changing group membership and reconnect the QC after reloading rules.
Do not run the broker as root or make hidraw nodes world-writable to bypass a
permission error. The supplied rule is not installed by a build or test.

## Headless use

Run as your ordinary user. The default discovery directory is
`$XDG_RUNTIME_DIR/org.freevia.qcremote`, then
`$XDG_STATE_HOME/org.freevia.qcremote`, or
`$HOME/.local/state/org.freevia.qcremote` when the preceding base is unset.
Relative base paths are ignored. If none is available, it uses a private
`qc-remote-<uid>` directory under the system temporary directory.
Use an absolute, user-owned path. The broker's directory is mode `0700` and its
credential descriptor mode `0600`. Existing unsafe ownership, symlinks or
permissions are refused rather than repaired implicitly.

For a usual systemd login:

```sh
./target/debug/qc-device-broker --self-check
./target/debug/qc-device-broker --broker-serve \
  --descriptor-path "$XDG_RUNTIME_DIR/org.freevia.qcremote/device-broker.json" \
  --no-tray
```

Keep a client attached to keep this nonresident broker alive. It starts
disconnected. Add `--auto-connect` only for an intentional hardware session, or
ask it to connect through the existing RPC. With no authenticated participant
the shared five-second grace releases the device and exits. Linux does not
pretend a tray icon exists, and `--resident`/autostart are not this milestone.

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

`quit --confirm` releases all this broker's clients and records the user's Quit.
SIGTERM and SIGINT perform bounded shared shutdown and release the descriptor,
without recording a user Quit. A killed/crashed owner cannot clean up its files;
the next election reclaims only a demonstrably dead, unlocked claim. It never
kills another process. The bearer file is a boundary between users, not between
two processes running as the same user.

## Current boundaries

HID report I/O and Linux process/path services are platform adapters. The
Windows native HID concurrency implementation and Windows process safeguards
remain behind their Windows backends. Linux performance MIDI is unsupported
in this milestone and must return an explicit error; HID command transport
must not silently substitute for a performance MIDI operation. Tray, logon
integration, packaged installers and desktop/MCP host discovery remain
follow-up work. Audio stays on the Quad Cortex and the OS audio stack.

Automated native Linux tests cover disconnected authenticated RPC, duplicate
launches, rejected authentication/framing, file modes, crash recovery,
SIGTERM and control-quit cleanup, including test-harness failure cleanup.
They do not exercise physical-device I/O. The live hardware checks above are
separate evidence and do not establish the full physical-device checklist.
