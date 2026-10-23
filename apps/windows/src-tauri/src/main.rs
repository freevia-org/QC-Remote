#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    std::hint::black_box(concat!(
        "qc-remote-macos-app-store-build:v1:",
        env!("CARGO_PKG_VERSION"),
        ":end"
    ));
    #[cfg(windows)]
    if qc_native_platform::process::creation_probe::dispatch_helper_mode() {
        return;
    }
    if std::env::args().any(|argument| argument == "--local-mcp-bridge") {
        if let Err(error) = qc_remote_lib::run_local_mcp_bridge() {
            eprintln!("QC Remote local MCP bridge failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    qc_remote_lib::run();
}
