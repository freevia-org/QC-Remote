//! Native services shared by the GUI, resident MCP host and device broker.
//! No protocol state, credentials or device handles live in this crate.
#[cfg(target_os = "linux")]
pub mod paths;
#[cfg(target_os = "macos")]
pub use paths_macos as paths;
pub mod owner_identity;
#[cfg(target_os = "macos")]
pub mod paths_macos;
pub mod process;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod resident_marker;
pub mod thread_owner;
