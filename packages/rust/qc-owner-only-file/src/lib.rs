//! Owner-only credential staging and atomic document replacement.
//!
//! On Windows every staged file has its protected, single-user DACL read back
//! before any bytes are written. See the Windows implementation for the trust
//! boundary: another process running as the same user can still read the file.
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{is_owner_only, write_staged};

/// Portable staging creates a fresh file with mode 0600 on Unix. Platforms
/// without a supported per-user protection mechanism fail closed.
#[cfg(not(windows))]
pub fn write_staged(path: &Path, contents: &[u8]) -> Result<(), String> {
    #[cfg(unix)]
    {
        write_staged_unix(path, contents).map_err(|error| error.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, contents);
        Err("Owner-only file protection is unsupported on this platform".into())
    }
}

#[cfg(unix)]
fn write_staged_unix(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let result = file.write_all(contents).and_then(|()| file.sync_all());
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

#[cfg(unix)]
fn stage_unix(contents: &[u8], mut next_path: impl FnMut() -> PathBuf) -> io::Result<PathBuf> {
    // A killed publisher may leave a stage with this PID/counter before a
    // reboot or PID reuse. Skip it without deleting somebody else's bytes.
    for _ in 0..64 {
        let temporary = next_path();
        match write_staged_unix(&temporary, contents) {
            Ok(()) => return Ok(temporary),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Could not reserve a fresh credential staging file",
    ))
}

#[cfg(unix)]
pub fn is_owner_only(path: &Path) -> Result<bool, String> {
    let file = open_unix_document(path).map_err(|error| error.to_string())?;
    unix_owner_only(&file).map_err(|error| error.to_string())
}

#[cfg(unix)]
fn open_unix_document(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    // NONBLOCK ensures a malicious FIFO cannot hang before we inspect type.
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(unix)]
fn unix_owner_only(file: &File) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no arguments or preconditions.
    let uid = unsafe { libc::geteuid() };
    Ok(metadata.is_file()
        && metadata.uid() == uid
        && metadata.mode() & 0o777 == 0o600
        && metadata.nlink() == 1)
}

/// Read precisely the owner-only regular file whose metadata was verified,
/// never a symlink or a replacement opened after the permission check.
#[cfg(unix)]
pub fn read_owner_only(path: &Path) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = open_unix_document(path)?;
    if !unix_owner_only(&file)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Credential file is not a current-user, owner-only regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Stage a complete owner-only document beside its destination, then replace
/// the destination atomically. Existing readers may keep their old snapshot.
pub fn publish_atomically(path: &Path, contents: &[u8]) -> Result<(), String> {
    static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or("Published file has no parent directory")?;
    let next_path = || {
        parent.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("document"),
            std::process::id(),
            NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
        ))
    };
    // Unix staging removes only a file it created. An existing name may
    // belong to an older process generation and is never ours to delete.
    #[cfg(unix)]
    let temporary = stage_unix(contents, next_path).map_err(|error| error.to_string())?;
    #[cfg(not(unix))]
    let temporary = next_path();
    #[cfg(not(unix))]
    if let Err(error) = write_staged(&temporary, contents) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    for attempt in 0..50 {
        match fs::rename(&temporary, path) {
            Ok(()) => return Ok(()),
            Err(error) if attempt == 49 => {
                let _ = fs::remove_file(&temporary);
                return Err(format!("Could not publish {}: {error}", path.display()));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    unreachable!()
}

/// Returns the exclusive claim sidecar path held by [`ClaimedFile`].
pub fn claim_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".claim");
    name.into()
}

fn claim_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::{
            Foundation::{GENERIC_READ, GENERIC_WRITE},
            Storage::FileSystem::{DELETE, FILE_SHARE_READ},
        };
        // DELETE access allows deleting this exact file by handle at release;
        // denying delete sharing prevents a reaper from unlinking a live claim.
        options
            .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
            .share_mode(FILE_SHARE_READ);
    }
    options
}

/// An exclusive create-new election claim, held independently of the readable
/// document so a rename never releases ownership or conflicts with this handle.
pub struct ClaimedFile {
    path: PathBuf,
    claim: Option<File>,
}

impl ClaimedFile {
    pub fn try_claim(path: &Path) -> io::Result<Self> {
        let lock_path = claim_path(path);
        let claim = claim_options().create_new(true).open(&lock_path)?;
        if let Err(error) = claim.try_lock() {
            return Err(io::Error::other(error));
        }
        ensure_current_claim(&claim, &lock_path)?;
        // Retain create_new on the document too: a broker from before the
        // sidecar protocol still owns this name and must never be overwritten.
        let mut document_options = OpenOptions::new();
        document_options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            document_options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        if let Err(error) = document_options.open(path) {
            let _ = remove_claim(claim, &lock_path);
            return Err(error);
        }
        Ok(Self {
            path: path.to_owned(),
            claim: Some(claim),
        })
    }

    pub fn write(&mut self, contents: &[u8]) -> Result<(), String> {
        if self.claim.is_none() {
            return Err("The publication claim has been released".into());
        }
        #[cfg(unix)]
        ensure_current_claim(
            self.claim.as_ref().expect("claim checked"),
            &claim_path(&self.path),
        )
        .map_err(|error| error.to_string())?;
        publish_atomically(&self.path, contents)
    }

    /// Remove the document before atomically deleting the held claim. On
    /// Windows deletion is by handle, so no path race can delete a successor.
    pub fn release(mut self) -> io::Result<()> {
        #[cfg(unix)]
        if let Some(claim) = self.claim.as_ref() {
            // Unix permits unlinking an open claim. An old holder whose name
            // was removed must not publish over or remove a successor.
            ensure_current_claim(claim, &claim_path(&self.path))?;
        }
        remove_document(&self.path)?;
        if let Some(claim) = self.claim.take() {
            remove_claim(claim, &claim_path(&self.path))?;
        }
        Ok(())
    }
}

fn remove_document(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(windows)]
fn remove_claim(claim: File, _path: &Path) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `claim` is an open `File` owned by this function, so its handle is valid for the
    // whole call, and `claim_options` opened it with DELETE access. `disposition` is a live
    // local of the type `FileDispositionInfo` names, and its own size is passed.
    let result = unsafe {
        SetFileInformationByHandle(
            claim.as_raw_handle().cast(),
            FileDispositionInfo,
            (&raw const disposition).cast(),
            std::mem::size_of_val(&disposition) as u32,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    drop(claim);
    Ok(())
}

#[cfg(not(windows))]
fn remove_claim(claim: File, path: &Path) -> io::Result<()> {
    ensure_current_claim(&claim, path)?;
    let result = fs::remove_file(path);
    drop(claim);
    result
}

// Unix permits unlinking an open file. A reaper that opened an earlier
// generation must not touch a successor after acquiring the old inode's lock.
#[cfg(unix)]
fn ensure_current_claim(claim: &File, path: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let held = claim.metadata()?;
    let current = fs::symlink_metadata(path)?;
    if !unix_owner_only(claim)? || !current.is_file() || current.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Unsafe publication claim",
        ));
    }
    if (held.dev(), held.ino()) != (current.dev(), current.ino()) {
        return Err(io::Error::other("Publication claim has been replaced"));
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_current_claim(_claim: &File, _path: &Path) -> io::Result<()> {
    // Windows deny-delete sharing pins the name while this handle is open.
    Ok(())
}

/// Reap a dead publisher, or a legacy descriptor without a sidecar. A live
/// claim refuses this operation. Keep the acquired claim until the document is
/// removed, then delete that exact claim by handle on Windows.
pub fn release_if_unclaimed(path: &Path) -> io::Result<()> {
    let lock_path = claim_path(path);
    match claim_options().open(&lock_path) {
        Ok(claim) => {
            claim.try_lock().map_err(io::Error::other)?;
            ensure_current_claim(&claim, &lock_path)?;
            remove_document(path)?;
            remove_claim(claim, &lock_path)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Acquire our own claim before deleting a legacy document; without
            // this, a new publisher could claim between the probe and delete.
            let claim = claim_options().create_new(true).open(&lock_path)?;
            claim.try_lock().map_err(io::Error::other)?;
            ensure_current_claim(&claim, &lock_path)?;
            remove_document(path)?;
            remove_claim(claim, &lock_path)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "qc-owner-claim-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        directory.join("device-broker.json")
    }

    #[cfg(unix)]
    #[test]
    fn unix_claim_and_placeholder_are_private_before_first_write() {
        let path = scratch("private-before-write");
        let claim = ClaimedFile::try_claim(&path).unwrap();
        assert!(is_owner_only(&path).unwrap());
        assert!(is_owner_only(&claim_path(&path)).unwrap());
        claim.release().unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unix_credentials_refuse_symlinks_hardlinks_and_public_modes() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let path = scratch("untrusted-files");
        write_staged(&path, b"secret").unwrap();
        let link = path.with_extension("link");
        symlink(&path, &link).unwrap();
        assert!(read_owner_only(&link).is_err());
        assert!(write_staged(&link, b"replacement").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"secret");
        fs::remove_file(&link).unwrap();
        fs::hard_link(&path, &link).unwrap();
        assert!(!is_owner_only(&path).unwrap());
        fs::remove_file(&link).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_owner_only(&path).unwrap());
        assert!(read_owner_only(&path).is_err());
        fs::remove_file(&path).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_atomic_publish_cleans_only_its_stage() {
        let path = scratch("failed-publish");
        fs::create_dir(&path).unwrap();
        assert!(publish_atomically(&path, b"secret").is_err());
        let entries: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(path.is_dir());
        fs::remove_dir(&path).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_preexisting_stage_is_preserved_and_the_next_name_is_published() {
        let path = scratch("stale-stage");
        let stale = path.with_extension("0.tmp");
        let fresh = path.with_extension("1.tmp");
        fs::write(&stale, b"older process's bytes").unwrap();
        let mut names = [stale.clone(), fresh.clone()].into_iter();
        let staged = stage_unix(b"new credential", || {
            names.next().expect("second name is free")
        })
        .unwrap();
        fs::rename(staged, &path).unwrap();
        assert_eq!(fs::read(&stale).unwrap(), b"older process's bytes");
        assert_eq!(read_owner_only(&path).unwrap(), b"new credential");
        assert!(!fresh.exists());
        fs::remove_file(&stale).unwrap();
        fs::remove_file(&path).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn stale_inode_cannot_remove_a_successor_claim() {
        let path = scratch("successor-inode");
        let first = ClaimedFile::try_claim(&path).unwrap();
        let old = claim_options().open(claim_path(&path)).unwrap();
        first.release().unwrap();
        let mut next = ClaimedFile::try_claim(&path).unwrap();
        next.write(b"successor").unwrap();
        assert!(ensure_current_claim(&old, &claim_path(&path)).is_err());
        assert!(remove_claim(old, &claim_path(&path)).is_err());
        assert_eq!(read_owner_only(&path).unwrap(), b"successor");
        next.release().unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn replaced_claim_prevents_old_holder_from_writing_or_releasing_successor() {
        let path = scratch("replaced-live-claim");
        let mut first = ClaimedFile::try_claim(&path).unwrap();
        first.write(b"predecessor").unwrap();
        // Model an external unlink, which Unix permits even for a held file.
        fs::remove_file(&path).unwrap();
        fs::remove_file(claim_path(&path)).unwrap();
        let mut next = ClaimedFile::try_claim(&path).unwrap();
        next.write(b"successor").unwrap();
        assert!(first.write(b"old refresh").is_err());
        assert!(first.release().is_err());
        assert_eq!(read_owner_only(&path).unwrap(), b"successor");
        assert!(release_if_unclaimed(&path).is_err());
        next.release().unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_live_claim_survives_reaping_and_atomic_refreshes() {
        let path = scratch("live");
        let mut claim = ClaimedFile::try_claim(&path).unwrap();
        claim.write(b"first complete document").unwrap();
        assert!(release_if_unclaimed(&path).is_err());
        claim.write(b"second complete document").unwrap();
        assert!(ClaimedFile::try_claim(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"second complete document");
        claim.release().unwrap();
        ClaimedFile::try_claim(&path).unwrap().release().unwrap();
    }

    #[test]
    fn a_dead_claim_can_be_reaped_without_waiting_for_descriptor_staleness() {
        let path = scratch("dead");
        let mut claim = ClaimedFile::try_claim(&path).unwrap();
        claim.write(b"dead publisher").unwrap();
        drop(claim); // Process exit closes the handle but leaves both names.
        release_if_unclaimed(&path).unwrap();
        assert!(!path.exists());
        assert!(!claim_path(&path).exists());
        ClaimedFile::try_claim(&path).unwrap().release().unwrap();
    }

    #[test]
    fn a_legacy_descriptor_still_blocks_a_new_election() {
        let path = scratch("legacy");
        fs::write(&path, b"legacy broker").unwrap();
        assert!(ClaimedFile::try_claim(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"legacy broker");
        assert!(!claim_path(&path).exists());
        release_if_unclaimed(&path).unwrap();
        ClaimedFile::try_claim(&path).unwrap().release().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn deleting_a_claim_by_handle_never_unlinks_a_successor() {
        // A queued reaper cannot retain a handle to the old claim and later
        // delete the next publisher's file: deny-write/delete sharing refuses
        // its open until deletion by the holder is already committed.
        let path = scratch("successor");
        for _ in 0..40 {
            let mut first = ClaimedFile::try_claim(&path).unwrap();
            first.write(b"first").unwrap();
            assert!(claim_options().open(claim_path(&path)).is_err());
            first.release().unwrap();
            let mut next = ClaimedFile::try_claim(&path).unwrap();
            next.write(b"next").unwrap();
            assert!(release_if_unclaimed(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"next");
            next.release().unwrap();
        }
    }
}
