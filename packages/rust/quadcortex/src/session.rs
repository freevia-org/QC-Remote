//! Device-session lifecycle and dispatch coordination.
//!
//! Re-exports [`crate::device::session`], the state machine that decides
//! when a session is stale, when it needs a lifecycle probe, and when a
//! queued command may dispatch.

pub use crate::device::session::*;
