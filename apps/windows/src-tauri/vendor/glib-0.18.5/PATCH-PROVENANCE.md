# glib 0.18.5 backport

This directory is the published `glib` 0.18.5 crate source from
`https://crates.io/api/v1/crates/glib/0.18.5/download`. The checked-in archive
matches the SHA-256 recorded in the pre-patch Tauri lockfile, and CI compares
every extracted archive file to this vendor tree. The crate version,
license, upstream repository, and dependency declarations remain unchanged.

The only source change is the two-line `VariantStrIter` output pointer
correction from gtk-rs-core commit
[40b7211](https://github.com/gtk-rs/gtk-rs-core/commit/40b7211): the pointer
written by `g_variant_get_child` is mutable and the call receives its mutable
address. A local regression test exercises forward, reverse, `nth`, `nth_back`,
and `last` paths in an app integration test; CI runs it in optimized test mode.
No version spoofing is used. `PATCH-PROVENANCE.json` pins the archive hash, original and patched
source-file hashes, and upstream fix; the CI provenance test checks them and
allows no source differences outside the single patched file.
