//! Owner-only, process-identity-bound claim for the per-user resident broker.
//!
//! The GUI reads this marker to distinguish a resident service from a live
//! descriptor owned by a short-lived client. It contains no broker token or
//! device information. A held claim prevents a second resident publisher;
//! the process identity makes a stale marker harmless after a crash or reboot.

use qc_owner_only_file::{self as private_file, ClaimedFile};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "device-broker-resident.json";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    version: u8,
    pid: u32,
    process_created_at: u64,
}

/// This process's resident claim. It is removed on drop, including ordinary
/// broker shutdown and supervisor unwind.
pub struct ResidentMarker {
    claim: Option<ClaimedFile>,
}

impl ResidentMarker {
    /// Atomically claim the marker path and publish this process's identity.
    pub fn acquire(directory: &Path) -> Result<Self, String> {
        let pid = std::process::id();
        let created_at = crate::process::os_process_created_at(pid)
            .ok_or("Cannot verify this process identity for resident marker")?;
        let contents = serde_json::to_vec(&Record {
            version: 1,
            pid,
            process_created_at: created_at,
        })
        .map_err(|error| format!("Encode resident marker: {error}"))?;
        let path = marker_path(directory);
        let claim = match ClaimedFile::try_claim(&path) {
            Ok(claim) => claim,
            Err(first) => {
                // Reap only an unlocked, stale owner-only generation. A live
                // publisher's claim makes this fail without touching it.
                if !private_file::is_owner_only(&path).unwrap_or(false)
                    || is_running(directory)
                    || private_file::release_if_unclaimed(&path).is_err()
                {
                    return Err(format!("Resident marker is already claimed: {first}"));
                }
                ClaimedFile::try_claim(&path)
                    .map_err(|error| format!("Claim resident marker: {error}"))?
            }
        };
        let mut marker = Self { claim: Some(claim) };
        marker
            .claim
            .as_mut()
            .expect("new resident claim")
            .write(&contents)?;
        Ok(marker)
    }

    /// Explicitly release before the resident supervisor exits.
    pub fn release(&mut self) {
        if let Some(claim) = self.claim.take() {
            let _ = claim.release();
        }
    }
}

impl Drop for ResidentMarker {
    fn drop(&mut self) {
        self.release();
    }
}

/// True only for a complete private marker naming a currently live process
/// with the same boot-and-start identity, and a still-held owner claim.
pub fn is_running(directory: &Path) -> bool {
    let path = marker_path(directory);
    let Ok(bytes) = private_file::read_owner_only(&path) else {
        return false;
    };
    let Some((pid, created_at)) = parse(&bytes) else {
        return false;
    };
    if !crate::process::os_process_is_alive(pid)
        || crate::process::os_process_created_at(pid) != Some(created_at)
    {
        return false;
    }
    claim_is_held(&path)
}

fn marker_path(directory: &Path) -> PathBuf {
    directory.join(FILE_NAME)
}

fn parse(bytes: &[u8]) -> Option<(u32, u64)> {
    if bytes.len() > 128 {
        return None;
    }
    let record = serde_json::from_slice::<Record>(bytes).ok()?;
    if record.version != 1 || record.pid == 0 {
        return None;
    }
    Some((record.pid, record.process_created_at))
}

fn claim_is_held(path: &Path) -> bool {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let claim_path = private_file::claim_path(path);
    let opened = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&claim_path);
    let Ok(claim) = opened else {
        return false;
    };
    let Ok(opened_metadata) = claim.metadata() else {
        return false;
    };
    let Ok(current_metadata) = std::fs::symlink_metadata(&claim_path) else {
        return false;
    };
    // Validate the handle we will lock, and ensure its path still names that
    // exact private inode. This avoids a permissions-check/open replacement
    // race around the sidecar.
    // SAFETY: geteuid has no arguments or preconditions.
    let uid = unsafe { libc::geteuid() };
    if !opened_metadata.is_file()
        || opened_metadata.uid() != uid
        || opened_metadata.mode() & 0o777 != 0o600
        || opened_metadata.nlink() != 1
        || !current_metadata.is_file()
        || current_metadata.file_type().is_symlink()
        || (opened_metadata.dev(), opened_metadata.ino())
            != (current_metadata.dev(), current_metadata.ino())
    {
        return false;
    }
    match claim.try_lock() {
        Ok(()) => false,
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "qc-resident-marker-{}-{}-{}",
                std::process::id(),
                crate::process::os_process_created_at(std::process::id()).unwrap_or_default(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reports_only_a_live_identity_with_a_held_private_claim() {
        let scratch = Scratch::new();
        let marker = ResidentMarker::acquire(&scratch.0).unwrap();
        assert!(is_running(&scratch.0));
        assert!(private_file::is_owner_only(&marker_path(&scratch.0)).unwrap());
        drop(marker);
        assert!(!is_running(&scratch.0));
        assert!(!marker_path(&scratch.0).exists());
        assert!(!private_file::claim_path(&marker_path(&scratch.0)).exists());
    }

    #[test]
    fn refuses_a_public_or_identity_mismatched_marker() {
        let scratch = Scratch::new();
        let path = marker_path(&scratch.0);
        let current_identity = crate::process::os_process_created_at(std::process::id()).unwrap();
        private_file::write_staged(
            &path,
            serde_json::to_vec(&Record {
                version: 1,
                pid: std::process::id(),
                process_created_at: current_identity,
            })
            .unwrap()
            .as_slice(),
        )
        .unwrap();
        assert!(!is_running(&scratch.0));
        let claim_path = private_file::claim_path(&path);
        private_file::write_staged(&claim_path, b"").unwrap();
        assert!(
            !is_running(&scratch.0),
            "the marker also needs a held claim"
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_running(&scratch.0));
    }

    #[test]
    fn a_stale_private_marker_is_replaced_only_after_its_claim_is_free() {
        let scratch = Scratch::new();
        let path = marker_path(&scratch.0);
        private_file::write_staged(
            &path,
            serde_json::to_vec(&Record {
                version: 1,
                pid: std::process::id(),
                process_created_at: crate::process::os_process_created_at(std::process::id())
                    .unwrap()
                    .wrapping_add(1),
            })
            .unwrap()
            .as_slice(),
        )
        .unwrap();
        let marker = ResidentMarker::acquire(&scratch.0).unwrap();
        assert!(is_running(&scratch.0));
        drop(marker);
        assert!(!is_running(&scratch.0));
    }
}
