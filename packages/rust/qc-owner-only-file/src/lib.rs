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
pub use windows::{
    create_new_owner_only, create_owner_only_directory, is_owner_only, is_owner_only_directory,
    write_staged,
};

/// Reserve a fresh empty regular file, protected and verified before any
/// caller-supplied bytes. A collision is never replaced. The caller owns the
/// returned handle and subsequent staging cleanup/publication.
#[cfg(unix)]
pub fn create_new_owner_only(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let checked = file
        .set_permissions(fs::Permissions::from_mode(0o600))
        .and_then(|()| {
            if unix_owner_only(&file)? {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Fresh file is not owner-private",
                ))
            }
        });
    if let Err(error) = checked {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
pub fn create_new_owner_only(_path: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Owner-only file protection is unsupported",
    ))
}

/// Create a directory protected to the current user, or accept an existing
/// directory only when its ownership and permissions already meet that policy.
/// Existing paths are never modified or removed. Returns `true` only when this
/// call created the directory; callers may use that result to scope cleanup.
#[cfg(unix)]
pub fn create_owner_only_directory(path: &Path) -> Result<bool, String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let created = match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => {
            return Err(format!(
                "Could not create owner-only directory {}: {error}",
                path.display()
            ));
        }
    };

    if created {
        // DirBuilder's mode is subject to umask. Tighten only the directory we
        // just created; never change permissions on a pre-existing path.
        if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o700)) {
            let _ = fs::remove_dir(path);
            return Err(format!(
                "Could not protect owner-only directory {}: {error}",
                path.display()
            ));
        }
    }

    match is_owner_only_directory(path) {
        Ok(true) => Ok(created),
        Ok(false) => {
            if created {
                let _ = fs::remove_dir(path);
            }
            Err(format!(
                "Directory {} is not private to this user",
                path.display()
            ))
        }
        Err(error) => {
            if created {
                let _ = fs::remove_dir(path);
            }
            Err(error)
        }
    }
}

/// Verify a regular directory is owned by the current user and inaccessible
/// to group/other users. The final path component is inspected without
/// following a symlink.
#[cfg(unix)]
pub fn is_owner_only_directory(path: &Path) -> Result<bool, String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect directory {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(false);
    }
    // SAFETY: geteuid has no arguments or preconditions.
    let uid = unsafe { libc::geteuid() };
    Ok(metadata.uid() == uid && metadata.mode() & 0o077 == 0)
}

#[cfg(not(any(unix, windows)))]
pub fn create_owner_only_directory(path: &Path) -> Result<bool, String> {
    let _ = path;
    Err("Owner-only directories are unsupported on this platform".into())
}

#[cfg(not(any(unix, windows)))]
pub fn is_owner_only_directory(path: &Path) -> Result<bool, String> {
    let _ = path;
    Err("Owner-only directories are unsupported on this platform".into())
}

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
    use std::io::Write;
    let mut file = create_new_owner_only(path)?;
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
    read_owner_only_bounded(path, usize::MAX)
}

/// Read one verified owner-private regular file through the same held handle,
/// refusing content larger than `maximum_bytes`, including growth after metadata.
/// This bounds bytes allocated/read, not the duration of an already-started OS read.
#[cfg(any(unix, windows))]
pub fn read_owner_only_bounded(path: &Path, maximum_bytes: usize) -> io::Result<Vec<u8>> {
    #[cfg(unix)]
    let mut file = {
        let file = open_unix_document(path)?;
        if !unix_owner_only(&file)? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Credential file is not a current-user, owner-only regular file",
            ));
        }
        file
    };
    #[cfg(windows)]
    let mut file = windows::open_owner_only_document(path)?;
    if file.metadata()?.len() > maximum_bytes as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Owner-only document exceeds its byte budget",
        ));
    }
    read_with_byte_budget(&mut file, maximum_bytes)
}

#[cfg(not(any(unix, windows)))]
pub fn read_owner_only_bounded(_path: &Path, _maximum_bytes: usize) -> io::Result<Vec<u8>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Owner-only reading is unsupported on this platform",
    ))
}

/// Collect from an already-open reader with a finite software byte ceiling.
/// This primitive alone does not verify filesystem ownership or permissions;
/// use [`read_owner_only_bounded`] when that protection is required.
pub fn read_with_byte_budget(
    reader: &mut impl io::Read,
    maximum_bytes: usize,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        // Observe at most one byte beyond the budget, never append that byte.
        let capacity = maximum_bytes
            .saturating_sub(bytes.len())
            .saturating_add(1)
            .min(chunk.len());
        let read = match reader.read(&mut chunk[..capacity]) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            return Ok(bytes);
        }
        if read > maximum_bytes.saturating_sub(bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Owner-only document exceeds its byte budget",
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
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
    #[cfg(windows)]
    let temporary = windows::stage_fresh(contents, next_path)?;
    #[cfg(not(any(unix, windows)))]
    let temporary = {
        let _ = (contents, next_path);
        return Err("Owner-only publication is unsupported on this platform".into());
        #[allow(unreachable_code)]
        PathBuf::new()
    };
    publish_reserved_stage_with(OwnedPublicationStage(Some(temporary)), path, |from, to| {
        fs::rename(from, to)
    })
}

// Own only the name freshly reserved by our staging API. Caller-controlled parents and
// same-user hostile path replacement are outside this module's publication trust boundary.
// In particular this is unwind cleanup, not a certification against adversarial path swaps.
struct OwnedPublicationStage(Option<PathBuf>);
impl Drop for OwnedPublicationStage {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

fn publish_reserved_stage_with(
    mut stage: OwnedPublicationStage,
    path: &Path,
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), String> {
    for attempt in 0..50 {
        match rename(stage.0.as_ref().expect("reserved stage is armed"), path) {
            Ok(()) => {
                // Ownership transferred; never delete a new occupant at the old source name.
                stage.0 = None;
                return Ok(());
            }
            Err(error) if attempt == 49 => {
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
    #[cfg(any(unix, windows))]
    #[test]
    fn fresh_owner_only_opener_protects_before_write_and_preserves_collisions() {
        use std::io::Write;
        let path = scratch("private-empty-open");
        let mut file = create_new_owner_only(&path).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 0);
        assert!(is_owner_only(&path).unwrap());
        file.write_all(b"owner-private bytes").unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert_eq!(
            create_new_owner_only(&path).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&path).unwrap(), b"owner-private bytes");
        assert!(is_owner_only(&path).unwrap());
    }

    struct Scratch(PathBuf);

    impl std::ops::Deref for Scratch {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl AsRef<Path> for Scratch {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let directory = self.0.parent().expect("scratch owns a fresh parent");
            if let Err(error) = fs::remove_dir_all(directory) {
                if error.kind() != io::ErrorKind::NotFound {
                    eprintln!(
                        "Could not clean owned test directory {}: {error}",
                        directory.display()
                    );
                }
            }
        }
    }

    fn scratch(name: &str) -> Scratch {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..64 {
            let directory = std::env::temp_dir().join(format!(
                "qc-owner-claim-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&directory) {
                Ok(()) => return Scratch(directory.join("device-broker.json")),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("Could not reserve test directory: {error}"),
            }
        }
        panic!("Could not reserve a fresh test directory");
    }

    #[test]
    fn owned_test_directories_are_removed_on_assertion_unwind() {
        let mut root = None;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let path = scratch("assertion-unwind");
            root = Some(path.parent().unwrap().to_owned());
            fs::write(&path, b"partial fixture").unwrap();
            panic!("injected fixture failure");
        }));
        assert!(outcome.is_err());
        assert!(!root.unwrap().exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn publication_rename_unwind_cleans_owned_stage_without_changing_previous_document() {
        let destination = scratch("rename-unwind");
        write_staged(&destination, b"previous complete bytes").unwrap();
        let temporary = destination.with_extension("owned.tmp");
        write_staged(&temporary, b"new complete bytes").unwrap();
        let collision = destination.with_extension("unowned.tmp");
        fs::write(&collision, b"other owner's bytes").unwrap();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            publish_reserved_stage_with(
                OwnedPublicationStage(Some(temporary.clone())),
                &destination,
                |_, _| panic!("injected rename unwind"),
            )
        }));
        assert!(outcome.is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"previous complete bytes");
        assert!(!temporary.exists());
        assert_eq!(fs::read(&collision).unwrap(), b"other owner's bytes");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn publication_success_disarms_stage_before_a_new_source_name_occupant() {
        let destination = scratch("rename-disarm");
        let temporary = destination.with_extension("owned.tmp");
        write_staged(&temporary, b"complete successor").unwrap();
        publish_reserved_stage_with(
            OwnedPublicationStage(Some(temporary.clone())),
            &destination,
            |from, to| {
                fs::rename(from, to)?;
                fs::write(from, b"new occupant after transfer")?;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"complete successor");
        assert_eq!(
            fs::read(&temporary).unwrap(),
            b"new occupant after transfer"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn bounded_owner_reader_accepts_exact_bytes_and_refuses_oversize() {
        let path = scratch("bounded-read");
        write_staged(&path, b"private bytes").unwrap();
        assert_eq!(
            read_owner_only_bounded(&path, 13).unwrap(),
            b"private bytes"
        );
        assert_eq!(
            read_owner_only_bounded(&path, 12).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            read_owner_only_bounded(&path, 0).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        publish_atomically(&path, b"").unwrap();
        assert_eq!(read_owner_only_bounded(&path, 0).unwrap(), Vec::<u8>::new());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn owner_reader_growth_after_metadata_is_still_bounded_on_the_held_file() {
        use std::io::Write;
        let path = scratch("growing-read");
        write_staged(&path, b"old").unwrap();
        #[cfg(unix)]
        let mut reader = open_unix_document(&path).unwrap();
        #[cfg(windows)]
        let mut reader = windows::open_owner_only_document(&path).unwrap();
        assert_eq!(reader.metadata().unwrap().len(), 3);
        let mut writer = OpenOptions::new().append(true).open(&path).unwrap();
        writer.write_all(b" growth").unwrap();
        writer.sync_all().unwrap();
        drop(writer);
        assert_eq!(
            read_with_byte_budget(&mut reader, 3).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(read_owner_only_bounded(&path, 10).unwrap(), b"old growth");
    }

    #[test]
    fn bounded_reader_observes_only_one_extra_byte_and_preserves_io_errors() {
        struct Counting {
            read: usize,
        }
        impl io::Read for Counting {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                bytes.fill(1);
                self.read += bytes.len();
                Ok(bytes.len())
            }
        }
        let mut reader = Counting { read: 0 };
        assert_eq!(
            read_with_byte_budget(&mut reader, 17).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(reader.read, 18);
        struct InterruptedOnce {
            interrupted: bool,
            bytes: io::Cursor<Vec<u8>>,
        }
        impl io::Read for InterruptedOnce {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                if !self.interrupted {
                    self.interrupted = true;
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "injected interruption",
                    ));
                }
                io::Read::read(&mut self.bytes, bytes)
            }
        }
        let mut interrupted = InterruptedOnce {
            interrupted: false,
            bytes: io::Cursor::new(b"ok".to_vec()),
        };
        assert_eq!(read_with_byte_budget(&mut interrupted, 2).unwrap(), b"ok");
        struct Failure;
        impl io::Read for Failure {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected read refusal",
                ))
            }
        }
        assert_eq!(
            read_with_byte_budget(&mut Failure, 17).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
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
