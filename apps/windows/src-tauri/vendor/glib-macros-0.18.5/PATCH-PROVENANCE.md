# glib-macros 0.18.5 compatibility patch

This directory starts from the published `glib-macros` 0.18.5 crate archive.
The adjacent archive is pinned by SHA-256 in `PATCH-PROVENANCE.json`, and CI
compares every vendored file with that archive before allowing only the exact
manifest dependency migration, the `proc_macro_error3` import alias, and the
two `publish = false` packaging fields.

The patch also trims trailing whitespace in one rustdoc blank line and removes
the extra blank line at the end of the copied license file; both exact
normalizations are asserted by the provenance test.

The crate uses Syn 2. Its old `proc-macro-error` 1.x dependency pulls the
unmaintained `proc-macro-error` 1.0.4 package. The replacement is
`proc-macro-error3` 3.1.1 with only its `syn2-error` feature enabled. The
dependency key deliberately remains `proc-macro-error3`: the replacement
attribute expands through `::proc_macro_error3::entry_point`, so a Cargo
package alias under the old dependency key would produce invalid generated
code. The `extern crate` alias preserves this crate's existing internal
`proc_macro_error` imports.
