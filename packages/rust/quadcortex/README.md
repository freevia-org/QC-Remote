# quadcortex

`quadcortex` is QC Remote's single reusable Rust library for compatible Quad
Cortex hardware. It contains HID report framing, protobuf schemas, typed
command builders, response decoding, normalized device state, session and
transport policy, guarded operation planning, catalog handling, transfers, and
native backup support.

It is deliberately free of I/O. The crate does **not** open a platform USB or
MIDI handle and does not contain Windows, Android, JNI, Tauri, or MCP
presentation code. A host supplies a monotonic clock and executes the byte
writes the library asks for; the library owns what to send, when, and how to
interpret what comes back. The Windows and Android apps in this repository are
two such hosts sharing this one implementation.

## Status

- **Version `0.1.0`, not published to crates.io.** The manifest carries
  `publish = false` until the publisher and registry decision is made, so
  `cargo publish` is refused. Depend on it by path or by git, as described
  below.
- **Pre-1.0.** Breaking changes can land in any release. See
  [Stability](#stability) for what is deliberate and what is only kept for
  compatibility.
- **Toolchain.** Rust edition 2021. The minimum supported Rust version
  (`rust-version` in the manifest) is **1.87**: it was verified by building the
  library and its examples with Rust 1.87 against the repository's `Cargo.lock`
  (`cargo +1.87 check -p quadcortex --lib --examples --locked`), and Rust 1.86
  fails because `src/device/transport.rs` uses `u*::is_multiple_of`, which
  stabilized in 1.87. The dependency floor is lower (Rust 1.85, `prost` 0.14).
  The test suite also builds and passes on 1.87
  (`cargo +1.87 test -p quadcortex --locked`). CI runs that command in its
  `rust-msrv` job on pull requests, besides building and testing on current
  stable; a push to `master` runs only the public-history audit. A dependency
  update may raise the minimum. When you depend on the crate without `--locked`,
  Cargo may resolve newer dependency versions with a higher floor than the one
  verified here.
- **Device compatibility.** The compatibility profile (`quadcortex::profile`)
  follows the wire protocol of Neural DSP Cortex Control 4.1.0 and the USB
  vendor/product identifiers it lists. This repository records no per-firmware
  test matrix, so treat compatibility as firmware-sensitive and unverified for
  any firmware beyond what the profile names.

## Using it

Inside this repository (or any checkout of it), depend on the path:

```toml
quadcortex = { path = "../quadcortex" }
```

From another project, depend on the git repository and pin a revision (the
library is a member of a larger workspace; Cargo finds it by package name):

```toml
quadcortex = { git = "https://github.com/freevia-org/QC-Remote", rev = "<commit>" }
```

Build the API reference locally with:

```text
cargo doc -p quadcortex --no-deps --open
```

## The seven public domains

```text
quadcortex::protocol   -- wire framing, generated messages, response decoding
quadcortex::commands   -- typed outbound command builders
quadcortex::state      -- normalized device state decoding
quadcortex::device     -- platform-neutral device state, workflow, and policy
quadcortex::session    -- session/connection lifecycle
quadcortex::backup     -- device backup stream lifecycle and assembly
quadcortex::transport  -- transport-layer (HID/USB) framing and lifecycle
```

Those responsibilities are isolated as modules rather than separate public
crates:

| Domain | Use it to |
| --- | --- |
| `protocol` | Frame and reassemble HID reports, name and decode any declared message type, decode correlated replies, read the compatibility profile. |
| `commands` | Build the outbound message for an action (select a scene, set the tempo, change a parameter) as a message type plus protobuf payload. |
| `state` | Feed it the messages the device pushes and receive normalized, serializable state updates; it also validates edits against the model catalog. |
| `device` | Plan and verify guarded writes, track the preset library and catalogs, and drive the initialization and preset workflows. |
| `session` | Sequence a native session's startup and lifecycle batches (`SessionDriver`), each bound to a session epoch so a callback from an older session is recognised. Command admission and session staleness live in `device::owner`. |
| `backup` | Follow the device's uncorrelated backup stream (replay prevention, liveness, progress) and assemble the finished backup. Backup-name and document validation live in `device::request`. |
| `transport` | Run the HID report framing and handshake state machine with a clock and a byte-oriented write action. |

## Examples

Six runnable examples need no hardware:

```text
cargo run -p quadcortex --example frame_round_trip
cargo run -p quadcortex --example replay_trace
cargo run -p quadcortex --example state_decoder
cargo run -p quadcortex --example command_tour
cargo run -p quadcortex --example state_watcher
cargo run -p quadcortex --example startup_walkthrough
```

- `frame_round_trip` builds commands, frames them into HID reports,
  reassembles and decodes them.
- `replay_trace` replays the crate's synthetic, sanitized report trace through
  the frame assembler.
- `state_decoder` turns device messages into normalized state updates.
- `command_tour` encodes typed commands and operations, and shows the typed
  error a bad value produces instead of a frame.
- `state_watcher` feeds a synthetic message stream to the state runtime and
  prints each update as JSON.
- `startup_walkthrough` runs the whole staged startup against synthetic replies.

## Guide and tutorial

A guide for Rust developers is built into the API documentation as the `guide`
module: what the library is and is not, a layer map, an offline quick start,
tutorials for reading state and for sending commands safely, sessions and
suspension, backups, transports, testing without a device, troubleshooting, and
a register of what is not established. Build it with:

```text
cargo doc -p quadcortex --no-deps --open
```

The pages are Markdown under [`docs/guide/`](docs/guide/index.md). Every Rust
snippet in them is a documentation test: most run offline against synthetic
bytes, and the few that need a device are compile-only and labelled as such. The
authors did not run any snippet against a unit. The guide module exists only
while rustdoc runs, so it does not change the compiled library or its seven
stable domains.

## Minimal framing example

```rust
use quadcortex::protocol::framing::{decode_reports, encode_message};

let reports = encode_message(7, b"payload");
let reports = reports.into_iter().map(Vec::from).collect::<Vec<_>>();
let frame = decode_reports(&reports)?;

assert_eq!(frame.message_type, 7);
assert_eq!(frame.payload, b"payload");
# Ok::<(), quadcortex::protocol::framing::FrameError>(())
```

This only demonstrates framing. Sending arbitrary protocol messages to a device
can mutate persistent state. Higher-level callers should use the guarded
operations and verification policy under `quadcortex::device`.

## Stability

`tests/public_api.rs` pins the public surface to exactly the seven domain
modules above, each tagged `// domain-module` in `src/lib.rs`. Adding or
removing a domain is a deliberate change that has to touch that test.

The lower-level modules those domains draw from (`compression`, `framing`,
`generated_payloads`, `inbound_telemetry`, `message_registry`, `profile`,
`responses`, `sync`, `wire`, plus the generated `domain` constants and `proto`
bindings) remain available at their historical crate-root paths for
compatibility. They are not curated entry points: new code should reach
wire-level APIs through `quadcortex::protocol`, and these paths may change.

The test pins the module set, not individual signatures. Until 1.0, expect
signatures and types inside the domains to change between releases.

## Verification

From the repository root:

```text
cargo test -p quadcortex --locked
cargo clippy -p quadcortex --all-targets --locked -- -D warnings
cargo fmt -p quadcortex -- --check
RUSTDOCFLAGS="-D warnings" cargo doc -p quadcortex --no-deps --locked
```

Run `npm run contracts:check` at the public repository root for value-level
protocol drift checks. The maintainer packaging check (which builds the crate,
inspects its allowlisted contents, then runs every test and documentation
example from the unpacked `.crate` artifact) remains private maintainer tooling.

## Protocol schema and generated code

The protobuf bindings, message registry, compatibility profile, and the
`domain`, `generated_payloads`, and `device::generated_gateway` constants are
generated from the project-maintained schema and contracts and checked for
drift. The generators and additional reference specifications remain private,
so propose schema changes in an issue or pull request rather than
editing generated Rust files by hand. The generators emit documentation:
every item of `domain`, `profile`, `message_registry`, and
`device::generated_gateway` is documented, as is every struct of
`generated_payloads` and each of its fields whose meaning is established. The
protobuf bindings document only the message actions, the message type table,
and the route id enumerations, and the other generated items are not
documented individually.

## License

The crate's source is available under `MIT OR Apache-2.0`, at your option; see
[`LICENSE`](https://github.com/freevia-org/QC-Remote/blob/master/LICENSE),
[`LICENSE-MIT`](https://github.com/freevia-org/QC-Remote/blob/master/LICENSE-MIT),
and
[`LICENSE-APACHE`](https://github.com/freevia-org/QC-Remote/blob/master/LICENSE-APACHE).
Because the crate embeds `src/device/native_library_seeds.rs`, a
device-derived file listed in Schedule A of
[`LICENSE`](https://github.com/freevia-org/QC-Remote/blob/master/LICENSE) for
reference and interoperability only, its manifest declares
`(MIT OR Apache-2.0) AND LicenseRef-QC-Remote-Reference` (text in
[`LICENSES/LicenseRef-QC-Remote-Reference.txt`](https://github.com/freevia-org/QC-Remote/blob/master/LICENSES/LicenseRef-QC-Remote-Reference.txt)).
Third-party components retain their own notices under `licenses/`.

The packaged crate carries the same texts: `LICENSE`, `LICENSE-MIT` and
`LICENSE-APACHE` at the crate root, and the reference-material terms as
`licenses/LicenseRef-QC-Remote-Reference.txt` (byte-identical copies of the
repository files, kept in step by `scripts/verify-license-carveout.mjs`). The
crate keeps its notices in `licenses/` rather than a `LICENSES/` directory,
which would collide with it on case-insensitive file systems.

QC Remote is independent and unofficial. It is not affiliated with, authorized,
sponsored, endorsed, or supported by Neural DSP Technologies Oy. Neural DSP and
Quad Cortex are trademarks of Neural DSP Technologies Oy and are referenced
only to identify compatibility. Device compatibility is firmware-sensitive.
