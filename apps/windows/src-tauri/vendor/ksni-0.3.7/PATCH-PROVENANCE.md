# ksni 0.3.7 zbus 6 downstream port

This directory vendors the published `ksni` 0.3.7 crate from
`https://crates.io/api/v1/crates/ksni/0.3.7/download`. The archive SHA-256 is
the exact checksum from the pre-migration Tauri Cargo.lock. `PATCH-PROVENANCE.json`
pins each changed file's archive and patched hashes; the CI verifier rejects
any other file difference.

The port follows the concrete zbus 6 migration checklist in upstream KSNI
[issue #44](https://github.com/iovxw/ksni/issues/44). It changes the connection
executor to zbus 6's `default-rt`, removes the zbus 5 internal-executor/manual
polling path, and updates the two `Value::Value` matches to the unified
`Value::Variant` representation. KSNI's application-facing `blocking` API and
its own async-io facade remain in use; the broker's direct zbus calls were
migrated separately to zbus 6 async APIs.

QC Remote's Linux broker dev-dependency enables the `qcr-test-layout` feature,
which re-exports KSNI's production `Layout` decoder only for the session-bus
integration regression. That regression decodes the real `GetLayout` response
through the same `ItemPropsValueHelper` path that contains the zbus 6 variant
conversion fix; normal application builds do not enable the feature.

The vendored manifests add only the pinned dependency/feature changes and
`publish = false`. The application does not publish this fork as a crate. The
Linux release workflow exercises the tray against a real private D-Bus session
and a StatusNotifierWatcher fixture, including registration, recovery,
retirement, and startup cleanup.
