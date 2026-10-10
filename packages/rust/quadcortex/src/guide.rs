#![doc = include_str!("../docs/guide/index.md")]
//!
//! This module exists only while rustdoc runs (`cfg(doc)`, set in `lib.rs`). It
//! is not part of the compiled library, so it does not add to the seven stable
//! domain modules pinned by `tests/public_api.rs`. Each page below is a
//! Markdown file under `docs/guide/`, and every Rust snippet in them is a
//! documentation test.

#[doc = include_str!("../docs/guide/01-overview.md")]
pub mod overview {}

#[doc = include_str!("../docs/guide/02-concepts.md")]
pub mod concepts {}

#[doc = include_str!("../docs/guide/03-offline-quick-start.md")]
pub mod offline_quick_start {}

#[doc = include_str!("../docs/guide/04-read-state.md")]
pub mod read_state {}

#[doc = include_str!("../docs/guide/05-send-commands.md")]
pub mod send_commands {}

#[doc = include_str!("../docs/guide/06-sessions.md")]
pub mod sessions {}

#[doc = include_str!("../docs/guide/07-backups.md")]
pub mod backups {}

#[doc = include_str!("../docs/guide/08-transports.md")]
pub mod transports {}

#[doc = include_str!("../docs/guide/09-testing.md")]
pub mod testing {}

#[doc = include_str!("../docs/guide/10-troubleshooting.md")]
pub mod troubleshooting {}

#[doc = include_str!("../docs/guide/11-limits.md")]
pub mod limits {}
