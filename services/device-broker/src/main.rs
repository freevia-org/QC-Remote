//! The broker-only developer build. The shipped `qc-device-broker.exe` is
//! built from apps/windows/src-tauri (`src/bin/qc-device-broker.rs`), which
//! calls the same [`qc_device_broker::cli::main`] with the background MCP host
//! linked in; this one has no host, so as a resident it shows the icon and
//! holds the descriptor but serves no MCP.
fn main() {
    qc_device_broker::cli::main(qc_device_broker::cli::Binary {
        version: env!("CARGO_PKG_VERSION"),
        #[cfg(target_os = "linux")]
        default_directory: Some(qc_device_broker::platform::paths::default_directory),
        #[cfg(not(target_os = "linux"))]
        default_directory: None,
        host: None,
        build_marker: None,
    });
}
