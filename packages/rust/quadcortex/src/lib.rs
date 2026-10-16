//! One reusable Rust library for Quad Cortex protocol and device operations.
//!
//! Protocol bindings and registries are generated from the project-maintained
//! schema. Third-party components retain their notices under `licenses/`.
//!
//! Its seven stable public domains are [`protocol`], [`commands`], [`state`],
//! [`device`], [`session`], [`backup`], and [`transport`]; `tests/public_api.rs`
//! pins this exact set. The lower-level modules those domains draw from --
//! [`compression`], [`framing`], [`generated_payloads`],
//! [`inbound_telemetry`], [`message_registry`], [`profile`], [`responses`], [`time`],
//! [`wire`], plus the generated [`domain`] constants and [`proto`] bindings,
//! plus [`sync`] --
//! remain available at their historical crate-root paths for compatibility.
//! Native USB ownership and host UI integration remain thin application
//! adapters outside this crate: the library never opens a USB or MIDI handle,
//! so everything here can be exercised without hardware. Runnable examples are
//! in the crate's `examples/` directory.
//!
//! A guide and tutorial, with compile-checked snippets, is in the
//! [`guide`] module of the rendered documentation.

#![warn(missing_docs)]

/// The crate README, compiled as doctests so its examples cannot rot. Exists
/// only under `cargo test --doc`; it is not part of the public API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

// Guide and tutorial pages. Rustdoc-only: `cfg(doc)` leaves the compiled
// library, and so its public surface, unchanged. See `src/guide.rs`.
#[cfg(doc)]
pub mod guide;

pub mod backup; // domain-module: device backup/restore lifecycle
pub mod commands; // domain-module: typed outbound command builders
/// Gzip inflation of compressed payloads, bounded by the profile size limit.
pub mod compression;
pub mod device; // domain-module: platform-neutral device state, workflow, and policy
#[cfg(feature = "cloud-forward")]
mod device_zencryption;
/// Fixed device-domain constants: grid size, scene count, tempo range, routes
/// and colour tables.
///
/// Generated from the project-maintained contracts, documentation included.
pub mod domain;
pub mod framing;
mod generated_host_policy;
/// Typed JSON payloads exchanged with hosts (settings, state, block details).
///
/// Generated from the project-maintained contracts. Every struct is
/// documented, and so is each field whose meaning the contracts establish;
/// the few generated fields that carry no doc comment are the ones whose
/// meaning is not established.
#[allow(missing_docs)]
pub mod generated_payloads;
pub mod inbound_telemetry;
/// The complete `CortexMessageType` table and its encode/decode helpers.
///
/// Generated from the project-maintained schema, documentation included.
pub mod message_registry;
/// Shared compatibility profile: USB identifiers, size limits, timeouts and
/// MIDI controller numbers.
///
/// Generated from the project-maintained contracts, documentation included.
pub mod profile;
pub mod protocol; // domain-module: wire protocol framing, messages, and decoding
pub mod responses;
pub mod session; // domain-module: session/connection lifecycle
pub mod state; // domain-module: normalized device state decoding
pub mod sync;
/// Wall-clock helpers shared by desktop and broker adapters.
pub mod time;
pub mod transport; // domain-module: transport-layer framing and lifecycle
pub mod wire;

/// Protobuf bindings for the Quad Cortex wire protocol, generated at build time
/// from the schemas under `proto/`.
///
/// Generated code. The message actions, the message type table and the route
/// id enumerations carry documentation from the schema generator; the other
/// generated items are not documented individually.
#[allow(missing_docs)]
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/quadcortex.rs"));
}
