#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|argument| argument == "--local-mcp-bridge") {
        if let Err(error) = qc_remote_lib::run_local_mcp_bridge() {
            eprintln!("QC Remote local MCP bridge failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    qc_remote_lib::run();
}
