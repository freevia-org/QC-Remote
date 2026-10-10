//! Device-backup planning, validation, and assembly.
//!
//! Re-exports [`crate::device::backup`], the platform-neutral lifecycle for
//! the QC's uncorrelated `LocalBackup` stream. Hosts provide only a
//! monotonic clock and execute the returned I/O action; replay prevention,
//! liveness, progress, and stream-boundary policy live here because they are
//! properties of the protocol, not of any one host.

pub use crate::device::backup::*;
