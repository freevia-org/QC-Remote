# tauri-plugin-single-instance 2.5.5 zbus 6 downstream port

This is the published `tauri-plugin-single-instance` 2.5.5 crate source from
`https://crates.io/api/v1/crates/tauri-plugin-single-instance/2.5.5/download`.
The archive hash matches the old application Cargo.lock. The Linux backend is
ported from upstream Tauri plugins-workspace PR #3759 at commit `1dba017`; the
CI verifier pins both source preimages and downstream file hashes in
`PATCH-PROVENANCE.json`.

The port uses zbus 6's async service API, retains callback forwarding of the
secondary process's argv and working directory, validates D-Bus name/path
before connecting, and releases the instance name during teardown. The two
upstream D-Bus integration tests are run from the consuming application
workspace so they use the exact locked QCR dependency graph. They cover
successful forwarding, name release, cleanup, invalid and missing bus
addresses, delivery failure, and single/current-thread Tauri runtime cases.

The vendored crate keeps its upstream package version and license. Both Cargo
manifests add only `publish = false` plus the zbus 6 dependency/runtime feature
changes; this application-owned fork is not published as a new plugin release.
