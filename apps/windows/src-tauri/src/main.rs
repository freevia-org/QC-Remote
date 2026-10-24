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
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    if std::env::args().any(|argument| argument == "--local-mcp-http-headers") {
        if qc_remote_lib::run_local_mcp_http_headers().is_err() {
            // Never echo a descriptor, credential, command or HTTP response.
            eprintln!("QC Remote could not verify its local HTTP host. Open QC Remote and explicitly Connect or repair the pairing.");
            std::process::exit(1);
        }
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
