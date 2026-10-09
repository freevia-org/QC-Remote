//! Wire protocol primitives: generated messages, framing, and response decoding.
//!
//! The underlying modules remain available at their historical crate-root
//! paths for compatibility. New consumers should enter wire-level APIs
//! through this domain module rather than reaching into the individual
//! crate-root modules directly.

pub use crate::compression;
pub use crate::framing;
pub use crate::generated_payloads;
pub use crate::message_registry;
pub use crate::profile;
pub use crate::proto;
pub use crate::responses;
pub use crate::wire;
