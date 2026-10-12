//! Native services at the edge of the one shared broker.
//!
//! Protocol, scheduling, authorization and device state are deliberately not
//! platform services. Windows retains its existing native implementations.
pub(crate) mod midi;
#[cfg(target_os = "linux")]
pub mod paths;
pub(crate) mod process;
#[cfg(target_os = "linux")]
pub(crate) mod shutdown;
