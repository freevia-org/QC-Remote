//! Pins the crate's deliberate public surface.
//!
//! `quadcortex` exposes exactly seven stable "domain" modules at its crate
//! root: `protocol`, `commands`, `state`, `device`, `session`, `backup`, and
//! `transport`. Everything else at the crate root (`compression`, `forward`,
//! `framing`, `generated_payloads`, `inbound_telemetry`,
//! `message_registry`, `profile`,
//! `responses`, `wire`, the generated `domain` constants, and the generated
//! `proto` bindings) is a lower-level module kept for compatibility, not a
//! curated entry point, and is deliberately excluded from this set.
//!
//! Rust has no runtime reflection over a crate's module tree, so this test
//! pins the surface two ways:
//!
//! 1. It names one concrete, public type from each domain, so the crate
//!    fails to compile if a domain module is removed, renamed, or stops
//!    re-exporting something real.
//! 2. It parses `src/lib.rs` for `// domain-module` markers and asserts that
//!    marker set is exactly the expected seven. Anyone adding an eighth
//!    stable domain (or accidentally dropping one) must touch this file's
//!    `EXPECTED_DOMAINS` list in the same change, which is the point: the
//!    seven-domain surface is deliberate, not accidental.

use quadcortex::{backup, commands, device, protocol, session, state, transport};

const EXPECTED_DOMAINS: [&str; 7] = [
    "protocol",
    "commands",
    "state",
    "device",
    "session",
    "backup",
    "transport",
];

#[test]
fn the_reusable_library_exposes_its_seven_public_domains() {
    let exported_types = [
        std::any::type_name::<protocol::framing::FrameAssembler>(),
        std::any::type_name::<commands::DeviceCommand>(),
        std::any::type_name::<state::StateDecoder>(),
        std::any::type_name::<device::PresetLibrary>(),
        std::any::type_name::<session::SessionDriver>(),
        std::any::type_name::<backup::BackupRuntime>(),
        std::any::type_name::<transport::TransportRuntime>(),
    ];

    assert!(
        exported_types
            .iter()
            .all(|name| name.starts_with("quadcortex::")),
        "every domain must re-export types under the `quadcortex` crate root, got {exported_types:?}"
    );
}

#[test]
fn lib_rs_declares_exactly_the_seven_stable_domain_modules() {
    let lib_source = include_str!("../src/lib.rs");

    let mut declared: Vec<&str> = lib_source
        .lines()
        .filter(|line| line.contains("// domain-module"))
        .map(|line| {
            line.trim_start()
                .strip_prefix("pub mod ")
                .and_then(|rest| rest.split(|c: char| c == ';' || c.is_whitespace()).next())
                .unwrap_or_else(|| {
                    panic!("line tagged `// domain-module` must read `pub mod <name>;`: {line}")
                })
        })
        .collect();
    declared.sort_unstable();

    let mut expected = EXPECTED_DOMAINS;
    expected.sort_unstable();

    assert_eq!(
        declared, expected,
        "src/lib.rs's `// domain-module`-tagged modules must be exactly {expected:?}; \
         update EXPECTED_DOMAINS here in the same change as any deliberate addition or removal"
    );
}
