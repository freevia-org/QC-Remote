//! Native services shared by the GUI, resident MCP host and device broker.
//! No protocol state, credentials or device handles live in this crate.
#[cfg(target_os = "linux")]
pub mod paths;
pub mod process;
#[cfg(target_os = "linux")]
pub mod resident_marker;
