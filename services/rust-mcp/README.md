# QC Remote MCP (Rust)

Production MCP protocol surface for QC Remote. It publishes only the typed,
intent-level actions in `contracts/public/qc-actions.v1.json` through rmcp Streamable
HTTP and delegates execution to a principal-scoped `QcBackend`. Beside them it
serves one local, read-only tool, `search_qc_device_knowledge`, and the
`qc://device-knowledge/guide` and `qc://device-knowledge/catalog` resources,
all answered from `skills/qc-device-knowledge` without reaching the backend.

This crate is embedded in the Windows app, which serves it over authenticated
loopback HTTP (`apps/windows/src-tauri/src/local_access.rs`) and stdio
(`apps/windows/src-tauri/src/local_stdio.rs`). It has no standalone listener.
The public remote relay is the Cloudflare Worker in `services/qc-remote-cloud`,
which implements its own MCP endpoint from the same `contracts/public`
catalogue. Construct each `QcMcp` with a backend scoped to exactly one
principal and device route; never use a process-global device route. Every
caller must insert a `PrincipalRoute` into each request's Axum extensions;
calls without one fail closed before the backend is reached.

The backend contract receives an allowlisted gateway RPC and validated camelCase
parameters. Confirmation-only flags are consumed at the MCP boundary and are not
trusted or forwarded as device commands.

Run `cargo test --manifest-path services/rust-mcp/Cargo.toml`.
