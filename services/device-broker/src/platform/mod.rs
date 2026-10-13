//! Native services at the edge of the one shared broker.
//!
//! Protocol, scheduling, authorization and device state are deliberately not
//! platform services. Windows retains its existing native implementations.
pub(crate) mod midi;
#[cfg(target_os = "linux")]
pub mod paths;
#[cfg(target_os = "macos")]
pub mod paths_macos;
pub(crate) mod process;
#[cfg(unix)]
pub(crate) mod shutdown;
