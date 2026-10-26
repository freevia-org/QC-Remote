//! Device-backup planning, validation, and assembly.
//!
//! Re-exports [`crate::device::backup`], the platform-neutral lifecycle for
//! the QC's uncorrelated `LocalBackup` stream. Hosts provide only a
//! monotonic clock and execute the returned I/O action; replay prevention,
//! liveness, progress, and stream-boundary policy live here because they are
//! properties of the protocol, not of any one host.

pub use crate::device::backup::*;

mod generated_integrity;
/// Advisory, bounded checksum inspection without changing native backup documents.
pub mod integrity;
pub use generated_integrity::MAXIMUM_JSON_BYTES as MAXIMUM_CHECKSUM_JSON_BYTES;
pub use generated_integrity::{BackupChecksumReason, BackupChecksumReportV1, BackupChecksumStatus};
