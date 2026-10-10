//! Shared verified owner-only staging for local credential publication.
//! See `qc_owner_only_file` for the protection boundary and Windows DACL checks.
#[cfg(test)]
pub(crate) use qc_owner_only_file::is_owner_only;
pub(crate) use qc_owner_only_file::write_staged;
