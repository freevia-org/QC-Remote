What `quadcortex` is, what it deliberately is not, and how much you can lean
on its API. Read this page first if you are deciding whether the crate fits.

## What the library is

`quadcortex` is a Rust library for the wire protocol, device state and session
policy of compatible Quad Cortex hardware. It contains:

- HID report **framing**: splitting a message into 129-byte reports and
  reassembling reports into a message ([`protocol::framing`](crate::protocol::framing)).
- The generated **protobuf** bindings and a table of every declared message type
  ([`protocol::proto`](crate::protocol::proto),
  [`protocol::message_registry`](crate::protocol::message_registry)).
- **Command builders** that turn an intent ("select scene 3", "set tempo") into a
  message type plus payload ([`commands`](crate::commands)).
- **Response decoding** for replies that correlate with a request
  ([`protocol::responses`](crate::protocol::responses)).
- **Normalized device state**: a decoder that folds the messages the unit pushes
  into small, serializable updates and a current-state snapshot
  ([`state`](crate::state), [`device`](crate::device)).
- **Session and transport policy**: handshake pacing, keepalive, startup
  sequencing, tickets that bind work to one session, suspension handling
  ([`transport`](crate::transport), [`session`](crate::session),
  [`device::owner`](crate::device::owner)).
- **Guarded write planning**: validation of a write against the state you last
  observed, and the predicate that tells you whether it landed
  ([`device::request`](crate::device::request)).
- **Backup assembly**: a runtime that reassembles the device's backup stream
  into one JSON document ([`backup`](crate::backup)).

The defining property is that the library **performs no I/O**. It never opens a
USB or MIDI handle. A host supplies a monotonic clock in milliseconds, executes
the byte writes the library hands back, and feeds the bytes it reads into the
library. Everything in this guide that runs does so without hardware for exactly
that reason.

## What the library is not

- **Not a USB or HID driver.** Enumeration, opening, reading, writing and
  threading are yours. [Section 8](crate::guide::transports) says what the
  repository's own hosts do, without making them part of this crate's contract.
- **Not an application.** There is no UI, no MCP server and no Windows, Android,
  JNI or Tauri code in the crate.
- **Not an official SDK.** It is an independent interoperability implementation.
  It follows the wire protocol described by the compatibility profile
  ([`protocol::profile`](crate::protocol::profile)), which names USB vendor and
  product identifiers and a host-version string the library announces to the
  device. The repository records no per-firmware test matrix, so compatibility
  is firmware-sensitive and unverified beyond what the profile names.
- **Not complete.** Messages of the cloud-account, login, backup-forwarding and
  license families travel over an encrypted trailer that this build does not
  implement. They are rejected by name in both directions
  ([`FrameError::UnsupportedEncryptedMessageType`](crate::protocol::framing::FrameError::UnsupportedEncryptedMessageType)).
  There is no builder that restores a backup onto a unit; see
  [Backups](crate::guide::backups).
- **Not a safe wrapper for arbitrary traffic.** The low-level builders in
  [`commands`](crate::commands) and the framing functions will encode whatever
  you ask. Sending arbitrary protocol messages to a device can mutate persistent
  state. The guarded layer is [`device::request`](crate::device::request); see
  [Sending commands safely](crate::guide::send_commands).

## Status, stability and toolchain

- **Version `0.1.0`, not published to crates.io.** The manifest sets
  `publish = false`. Depend on the crate by path or by git revision; the crate
  README shows both forms.
- **Pre-1.0.** Signatures and types inside the domains can change between
  releases.
- **Stable surface: seven domain modules.** `tests/public_api.rs` pins the crate
  root to exactly [`protocol`](crate::protocol), [`commands`](crate::commands),
  [`state`](crate::state), [`device`](crate::device),
  [`session`](crate::session), [`backup`](crate::backup) and
  [`transport`](crate::transport). The test pins the *set of modules*, and names
  one type from each; it does not pin individual signatures. The lower-level
  modules those domains draw from (`framing`, `responses`, `message_registry`,
  `wire`, `profile`, and others) remain at their historical crate-root paths for
  compatibility, but new code should reach them through `protocol`. This guide
  does so.
- **Minimum supported Rust version: 1.87.** It was verified by building the
  library, examples and tests with 1.87 against the repository's `Cargo.lock`;
  1.86 fails because the crate uses `is_multiple_of`, which stabilized in 1.87.
  The repository's CI builds the crate on exactly 1.87 as a separate job. A
  dependency update may raise the floor, and resolving dependencies without the
  lock file may pick versions with a higher floor than the one verified.
- **Platform neutral by construction.** The crate has no platform-specific code
  or dependencies. The repository's CI builds and tests it on Windows; this
  guide establishes nothing about other platforms.

The guide itself is documentation only. It is compiled by a module that exists
only under `cfg(doc)`, so it is not part of the compiled library and does not
change the seven-domain surface.

## A first look at the profile

The compatibility profile is a set of constants generated from the project's
contracts. They are the numbers the rest of the library's policy is built from.

```rust
use quadcortex::protocol::profile;

// USB identifiers a host would match when enumerating devices.
println!("vendor {:#06x}, products {:?}", profile::VENDOR_ID, profile::SUPPORTED_PRODUCT_IDS);
assert!(profile::supports_product_id(profile::SUPPORTED_PRODUCT_IDS[0]));
assert!(!profile::supports_product_id(0));

// Timing and size policy the session and backup runtimes use. All times are
// milliseconds, and every runtime takes the host's own monotonic clock.
assert_eq!(profile::KEEPALIVE_INTERVAL_MS, 1_000);
assert_eq!(profile::HANDSHAKE_TIMEOUT_MS, 30_000);
assert_eq!(profile::MAX_FRAME_BYTES, 1024 * 1024);
```

The last three assertions pin today's values so that this page cannot silently
go stale when the generated profile changes.

> **What to verify before relying on this.** The profile follows one specific
> host-protocol version, named by `profile::CORTEX_CONTROL_VERSION`, and the
> device identifiers it lists. Whether a firmware release other than the ones
> the maintainers have used accepts the same handshake is not established here.
