//! Canonical, non-secret cross-process access policy.
//!
//! The file is intentionally separate from the keyring: every serving process
//! must be able to observe a revocation without waiting for a desktop keyring
//! service. The keyring is only a source for one-time legacy migration, which
//! the caller may perform with [`initialize_if_absent`]. An absent or unreadable
//! policy is always fail-closed (`ReadOnly`). The OS-provided app-data parent
//! is trusted; this module protects and verifies its dedicated policy child
//! and the records within it.

use qc_relay_client::AccessMode;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

const STORE_DIRECTORY: &str = "relay-access-policy";
const FILE_NAME: &str = "policy.json";
const MIGRATION_SENTINEL: &str = "relay-access-policy.migrated";
const MIGRATION_SENTINEL_CONTENTS: &[u8] = b"qc-remote-access-policy-v1\n";
const LOCK_NAME: &str = "relay-access-policy.lock";
const MAX_BYTES: u64 = 1024;
const LOCK_WAIT: Duration = Duration::from_secs(2);
const LOCK_RETRY: Duration = Duration::from_millis(10);

/// A validated snapshot. Revisions are opaque and change on each successful
/// write; callers should compare them for equality, not order them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub(crate) schema_version: u8,
    pub(crate) revision: String,
    pub(crate) mode: AccessMode,
    pub(crate) remote_enabled: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRecord {
    schema_version: u8,
    revision: String,
    mode: String,
    /// Old/partial records are disabled until explicit pairing enables them.
    #[serde(default)]
    remote_enabled: bool,
}

impl Record {
    fn new(mode: AccessMode, remote_enabled: bool) -> Self {
        Self {
            schema_version: 1,
            revision: format!("{:032x}", rand::random::<u128>()),
            mode,
            remote_enabled,
        }
    }

    fn encode(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&WireRecord {
            schema_version: self.schema_version,
            revision: self.revision.clone(),
            mode: self.mode.as_str().to_owned(),
            remote_enabled: self.remote_enabled,
        })
        .map_err(|error| format!("Could not encode the relay access policy: {error}"))
    }

    fn decode(bytes: &[u8]) -> Result<Self, String> {
        let wire: WireRecord = serde_json::from_slice(bytes)
            .map_err(|error| format!("The relay access policy is invalid: {error}"))?;
        if wire.schema_version != 1 {
            return Err("The relay access policy uses an unsupported schema version".into());
        }
        if wire.revision.len() != 32 || !wire.revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("The relay access policy revision is invalid".into());
        }
        let mode = AccessMode::parse(&wire.mode)
            .map_err(|_| "The relay access policy mode is invalid".to_string())?;
        Ok(Self {
            schema_version: wire.schema_version,
            revision: wire.revision,
            mode,
            remote_enabled: wire.remote_enabled,
        })
    }
}

fn policy_path(directory: &Path) -> PathBuf {
    directory.join(STORE_DIRECTORY).join(FILE_NAME)
}

fn store_directory(directory: &Path) -> PathBuf {
    directory.join(STORE_DIRECTORY)
}

fn migration_sentinel_path(directory: &Path) -> PathBuf {
    directory.join(MIGRATION_SENTINEL)
}

/// A permanent, owner-only marker in the trusted app-data parent records that
/// canonical policy initialization has begun. Its publication precedes every
/// canonical write, so losing the child store can never revive stale keyring
/// preferences. A malformed marker fails closed instead of being repaired.
fn migration_is_complete(directory: &Path) -> Result<bool, String> {
    let path = migration_sentinel_path(directory);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("Could not inspect relay policy marker: {error}")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 64 {
        return Err("The relay policy migration marker is unsafe".into());
    }
    if !qc_owner_only_file::is_owner_only(&path)
        .map_err(|error| format!("Could not verify relay policy marker permissions: {error}"))?
    {
        return Err("The relay policy migration marker is not owner-only".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = options
        .open(&path)
        .map_err(|error| format!("Could not read relay policy marker: {error}"))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("Could not inspect relay policy marker: {error}"))?;
    if !opened.is_file() || opened.len() > 64 {
        return Err("The relay policy migration marker is unsafe".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no arguments or preconditions.
        let uid = unsafe { libc::geteuid() };
        if opened.uid() != uid || opened.mode() & 0o777 != 0o600 || opened.nlink() != 1 {
            return Err("The relay policy migration marker is not owner-only".into());
        }
    }
    #[cfg(windows)]
    if !qc_owner_only_file::is_owner_only(&path)
        .map_err(|error| format!("Could not verify relay policy marker permissions: {error}"))?
    {
        return Err("The relay policy migration marker is not owner-only".into());
    }
    let mut contents = Vec::with_capacity(opened.len() as usize);
    file.take(65)
        .read_to_end(&mut contents)
        .map_err(|error| format!("Could not read relay policy marker: {error}"))?;
    if contents != MIGRATION_SENTINEL_CONTENTS {
        return Err("The relay policy migration marker is invalid".into());
    }
    Ok(true)
}

/// Called only while holding the stable writer lock. The marker is never
/// removed, including when a later canonical write fails.
fn ensure_migration_sentinel(directory: &Path) -> Result<(), String> {
    if migration_is_complete(directory)? {
        return Ok(());
    }
    let path = migration_sentinel_path(directory);
    qc_owner_only_file::publish_atomically(&path, MIGRATION_SENTINEL_CONTENTS)?;
    if !migration_is_complete(directory)? {
        return Err("Could not verify the relay policy migration marker".into());
    }
    Ok(())
}

fn ensure_directory(directory: &Path) -> Result<(), String> {
    validate_base_directory(directory)?;
    let private_directory = store_directory(directory);
    if !private_directory.exists() {
        qc_owner_only_file::create_owner_only_directory(&private_directory)
            .map_err(|error| format!("Could not protect the relay policy directory: {error}"))?;
    }
    validate_directory(&private_directory)
}

fn validate_base_directory(directory: &Path) -> Result<(), String> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            #[cfg(target_os = "linux")]
            qc_native_platform::paths::prepare_directory(directory)
                .map_err(|error| format!("Could not prepare the app data directory: {error}"))?;
            #[cfg(target_os = "macos")]
            qc_native_platform::paths_macos::prepare_directory(directory)
                .map_err(|error| format!("Could not prepare the app data directory: {error}"))?;
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            fs::create_dir_all(directory)
                .map_err(|error| format!("Could not create the app data directory: {error}"))?;
            let metadata = fs::symlink_metadata(directory)
                .map_err(|error| format!("Could not inspect the app data directory: {error}"))?;
            validate_base_metadata(directory, &metadata)
        }
        Err(error) => Err(format!("Could not inspect the app data directory: {error}")),
        Ok(metadata) => validate_base_metadata(directory, &metadata),
    }
}

fn validate_base_metadata(directory: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("The app data location is not a regular directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no arguments or preconditions.
        let uid = unsafe { libc::geteuid() };
        use std::os::unix::fs::PermissionsExt;
        if metadata.uid() != uid || metadata.permissions().mode() & 0o777 != 0o700 {
            return Err("The app data directory must be owned by this user with mode 0700".into());
        }
    }
    let _ = directory;
    Ok(())
}

fn validate_directory(directory: &Path) -> Result<(), String> {
    if !qc_owner_only_file::is_owner_only_directory(directory)
        .map_err(|error| format!("Could not verify relay policy directory: {error}"))?
    {
        return Err("The relay policy location is not a regular directory".into());
    }
    Ok(())
}

fn read_record_at(path: &Path) -> Result<Option<Record>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Could not inspect the relay access policy: {error}"
            ))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("The relay access policy is not a regular file".into());
    }
    if metadata.len() > MAX_BYTES {
        return Err("The relay access policy is too large".into());
    }
    if !qc_owner_only_file::is_owner_only(path)
        .map_err(|error| format!("Could not verify relay policy permissions: {error}"))?
    {
        return Err("The relay access policy is not owner-only".into());
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("Could not read the relay access policy: {error}"))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("Could not inspect the relay access policy: {error}"))?;
    if !opened.is_file() || opened.len() > MAX_BYTES {
        return Err("The relay access policy is not a bounded regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no arguments or preconditions.
        let uid = unsafe { libc::geteuid() };
        if opened.uid() != uid || opened.mode() & 0o777 != 0o600 || opened.nlink() != 1 {
            return Err("The relay access policy is not owner-only".into());
        }
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read the relay access policy: {error}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("The relay access policy is too large".into());
    }
    #[cfg(windows)]
    if !qc_owner_only_file::is_owner_only(path)
        .map_err(|error| format!("Could not verify relay policy permissions: {error}"))?
    {
        return Err("The relay access policy is not owner-only".into());
    }
    Record::decode(&bytes).map(Some)
}

/// Read the canonical policy. `Ok(None)` means only that no policy has been
/// initialized; malformed, unsafe, and unavailable files are errors.
pub(crate) fn read(directory: &Path) -> Result<Option<Record>, String> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Could not inspect the relay policy directory: {error}"
            ))
        }
        Ok(metadata) => validate_base_metadata(directory, &metadata)?,
    }
    let migration_complete = migration_is_complete(directory)?;
    let private_directory = store_directory(directory);
    match fs::symlink_metadata(&private_directory) {
        // Absence of the child directory means this installation has never
        // initialized a policy. Once the protected directory exists, a missing
        // record is loss/corruption, not permission to re-import stale legacy
        // preferences (which may still say Full after a later user revocation).
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return if migration_complete {
                Err("The initialized relay access policy directory is missing".into())
            } else {
                Ok(None)
            };
        }
        Err(error) => {
            return Err(format!(
                "Could not inspect the relay policy directory: {error}"
            ))
        }
        Ok(_) => {
            validate_directory(&private_directory)?;
            let path = policy_path(directory);
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err("The initialized relay access policy is missing".into());
                }
                Err(error) => {
                    return Err(format!(
                        "Could not inspect the relay access policy: {error}"
                    ));
                }
                Ok(_) => {}
            }
        }
    }
    read_record_at(&policy_path(directory))
}

/// The request-time fail-closed view used by serving hosts.
pub(crate) fn effective_mode(directory: &Path) -> AccessMode {
    read(directory)
        .ok()
        .flatten()
        .map_or(AccessMode::ReadOnly, |record| record.mode)
}

/// Request-time public relay session latch. Absence and every read failure
/// deny the remote session; local MCP admission does not consult this value.
pub(crate) fn effective_remote_enabled(directory: &Path) -> bool {
    read(directory)
        .ok()
        .flatten()
        .is_some_and(|record| record.remote_enabled)
}

fn acquire_writer(directory: &Path) -> Result<File, String> {
    ensure_directory(directory)?;
    let lock_path = store_directory(directory).join(LOCK_NAME);
    let started = Instant::now();
    loop {
        if !lock_path.exists() {
            create_lock_file(&lock_path)?;
        }
        #[cfg(unix)]
        let owner_only = true;
        #[cfg(not(unix))]
        let owner_only = qc_owner_only_file::is_owner_only(&lock_path)
            .map_err(|error| format!("Could not verify relay policy lock permissions: {error}"))?;
        if !owner_only {
            if started.elapsed() < LOCK_WAIT {
                thread::sleep(LOCK_RETRY);
                continue;
            }
            return Err("The relay policy lock is not owner-only".into());
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
        }
        let file = options
            .open(&lock_path)
            .map_err(|error| format!("Could not open the relay policy lock: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("Could not inspect the relay policy lock: {error}"))?;
        if !metadata.is_file() || metadata.len() != 0 {
            return Err("The relay policy lock is not a regular file".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // SAFETY: geteuid has no arguments or preconditions.
            let uid = unsafe { libc::geteuid() };
            if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o777 != 0o600 {
                return Err("The relay policy lock is not owner-only".into());
            }
        }
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < LOCK_WAIT => {
                drop(file);
                thread::sleep(LOCK_RETRY);
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err("Another relay policy update is still in progress".into());
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(format!("Could not lock the relay access policy: {error}"));
            }
        }
    }
}

/// Publish the persistent lock inode with a non-replacing hard link. The
/// staging directory is exclusively created for this attempt, so staging and
/// cleanup can never remove another writer's files.
fn pending_private_lock_stage(parent: &Path) -> Result<Option<PathBuf>, String> {
    for entry in fs::read_dir(parent)
        .map_err(|error| format!("Could not inspect retained lock stages: {error}"))?
    {
        let entry = entry
            .map_err(|error| format!("Could not inspect retained lock stage entry: {error}"))?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".relay-policy-lock-stage")
        {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

fn create_lock_file(lock_path: &Path) -> Result<(), String> {
    create_lock_file_with(
        lock_path,
        LOCK_WAIT,
        qc_owner_only_file::create_owner_only_directory,
    )
}

fn create_lock_file_with(
    lock_path: &Path,
    budget: Duration,
    mut create_private: impl FnMut(&Path) -> Result<bool, String>,
) -> Result<(), String> {
    let parent = lock_path
        .parent()
        .ok_or("Relay policy lock has no parent directory")?;
    // One fixed, exclusively-created stage bounds new artifacts even across
    // process restarts. Unknown legacy GUID stages are preserved and block
    // further stage allocation; a cooperating initializer may finish normally.
    let staging_directory = parent.join(".relay-policy-lock-stage");
    let deadline = Instant::now() + budget;
    loop {
        if lock_path
            .try_exists()
            .map_err(|error| format!("Could not inspect relay policy lock: {error}"))?
        {
            return Ok(()); // acquire_writer verifies permissions before opening.
        }
        let pending = pending_private_lock_stage(parent)?;
        if let Some(path) = pending {
            if Instant::now() >= deadline {
                return Err(format!("A relay policy lock stage remains at {}; it was preserved and no new stage was allocated. Inspect the stage's ownership and permissions before explicitly retrying.", path.display()));
            }
            thread::sleep(LOCK_RETRY.min(deadline.saturating_duration_since(Instant::now())));
            continue;
        }
        if Instant::now() >= deadline {
            return Err(
                "Relay policy lock staging admission timed out before creating a directory".into(),
            );
        }
        match create_private(&staging_directory).map_err(|error| {
            format!(
                "Could not create a private lock stage at {}: {error}",
                staging_directory.display()
            )
        })? {
            true => break,
            false => {
                // A concurrent initializer or unknown artifact owns this name.
                // Never remove it or allocate another name to evade admission.
                thread::sleep(LOCK_RETRY.min(deadline.saturating_duration_since(Instant::now())));
            }
        }
    }
    let staged_file = staging_directory.join("lock");
    if let Err(error) = qc_owner_only_file::write_staged(&staged_file, b"") {
        let _ = fs::remove_file(&staged_file);
        let _ = fs::remove_dir(&staging_directory);
        return Err(format!("Could not stage the relay policy lock: {error}; any unretired stage at {} is preserved for inspection", staging_directory.display()));
    }
    match fs::hard_link(&staged_file, lock_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            let _ = fs::remove_file(&staged_file);
            let _ = fs::remove_dir(&staging_directory);
            return Err(format!("Could not publish the relay policy lock: {error}; any unretired stage at {} is preserved for inspection", staging_directory.display()));
        }
    };
    let stage_removed = fs::remove_file(&staged_file).is_ok();
    let directory_removed = fs::remove_dir(&staging_directory).is_ok();
    if !stage_removed || !directory_removed {
        return Err(format!("Could not clean up the owned relay policy lock stage at {}; inspect its ownership and permissions before explicitly retrying", staging_directory.display()));
    }
    Ok(())
}
fn publish(directory: &Path, mode: AccessMode, remote_enabled: bool) -> Result<Record, String> {
    let record = Record::new(mode, remote_enabled);
    qc_owner_only_file::publish_atomically(&policy_path(directory), &record.encode()?)?;
    Ok(record)
}

/// Persist an explicit user choice, serializing concurrent writers and
/// publishing the complete new record atomically.
pub(crate) fn write(directory: &Path, mode: AccessMode) -> Result<Record, String> {
    let _lock = acquire_writer(directory)?;
    ensure_migration_sentinel(directory)?;
    // An explicit access-mode edit preserves a known pairing latch. If the
    // record is missing or malformed, repairing it must not reactivate remote.
    let remote_enabled = read_record_at(&policy_path(directory))
        .ok()
        .flatten()
        .is_some_and(|record| record.remote_enabled);
    publish(directory, mode, remote_enabled)
}

/// Change only the remote session latch, preserving a valid access mode. This
/// is called after explicit pair success or before unpair begins. Corrupt or
/// missing state repairs with the least-privileged access mode.
pub(crate) fn update_remote_enabled(directory: &Path, enabled: bool) -> Result<Record, String> {
    let _lock = acquire_writer(directory)?;
    ensure_migration_sentinel(directory)?;
    let mode = read_record_at(&policy_path(directory))
        .ok()
        .flatten()
        .map_or(AccessMode::ReadOnly, |record| record.mode);
    publish(directory, mode, enabled)
}

/// Complete the one-time migration only if the canonical file is still absent.
/// If another process has already made a choice, return it without clobbering
/// it. Callers must pass a mode only after bounded legacy-store migration has
/// completed successfully; absence itself always remains ReadOnly.
pub(crate) fn initialize_if_absent(
    directory: &Path,
    migrated_mode: AccessMode,
) -> Result<Record, String> {
    // The permanent marker, not the child directory, distinguishes committed
    // policy state from an interrupted/concurrent first initialization. The
    // writer lock itself lives in that child and may create it before lock
    // acquisition, so directory absence alone is not a stable election test.
    let private_directory = store_directory(directory);
    let sentinel_present = migration_is_complete(directory)?;
    let has_policy = match fs::symlink_metadata(&private_directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!(
                "Could not inspect the relay policy directory: {error}"
            ));
        }
        Ok(_) => {
            validate_directory(&private_directory)?;
            read_record_at(&policy_path(directory))?.is_some()
        }
    };
    let pristine = !sentinel_present && !has_policy;
    let _lock = acquire_writer(directory)?;
    match read_record_at(&policy_path(directory))? {
        Some(record) => {
            ensure_migration_sentinel(directory)?;
            Ok(record)
        }
        None if pristine && !migration_is_complete(directory)? => {
            ensure_migration_sentinel(directory)?;
            publish(directory, migrated_mode, true)
        }
        None => Err("The initialized relay access policy is missing".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_lock_stage_inspection_failure_blocks_repeated_allocations_and_preserves_bytes() {
        let scratch = Scratch::new("failed-stage-admission");
        ensure_directory(&scratch.0).unwrap();
        let parent = store_directory(&scratch.0);
        let lock = parent.join(LOCK_NAME);
        let mut calls = 0;
        let failed = create_lock_file_with(&lock, LOCK_WAIT, |path| {
            calls += 1;
            fs::create_dir(path).unwrap();
            fs::write(path.join("sentinel"), b"unverified artifact").unwrap();
            Err("injected post-create inspection failure; directory retained".into())
        });
        assert!(failed.unwrap_err().contains("private lock stage at"));
        assert_eq!(calls, 1);
        let stage = parent.join(".relay-policy-lock-stage");
        for _ in 0..3 {
            let refused = create_lock_file_with(&lock, Duration::ZERO, |_| {
                panic!("retained artifact must prevent another allocation")
            })
            .unwrap_err();
            assert!(refused.contains("was preserved and no new stage"));
            assert_eq!(
                fs::read(stage.join("sentinel")).unwrap(),
                b"unverified artifact"
            );
            assert!(!lock.exists());
        }
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
    }

    #[test]
    fn a_legacy_guid_stage_is_preserved_and_refuses_new_fixed_stage_allocation() {
        let scratch = Scratch::new("legacy-stage-admission");
        ensure_directory(&scratch.0).unwrap();
        let parent = store_directory(&scratch.0);
        let old = parent.join(".relay-policy-lock-stage-123-4-legacy");
        fs::create_dir(&old).unwrap();
        fs::write(old.join("sentinel"), b"another initializer's bytes").unwrap();
        let refused = create_lock_file_with(&parent.join(LOCK_NAME), Duration::ZERO, |_| {
            panic!("unknown legacy stage must prevent new allocation")
        })
        .unwrap_err();
        assert!(refused.contains("was preserved"));
        assert_eq!(
            fs::read(old.join("sentinel")).unwrap(),
            b"another initializer's bytes"
        );
        assert!(!parent.join(".relay-policy-lock-stage").exists());
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
    }
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Arc;

    // Scoped children borrow the root; release on unwind precedes scope's join.
    // Ordinary filesystem IO may still wait for the OS, but admission cannot strand them.
    struct ReleaseGate(Arc<AtomicBool>);
    impl Drop for ReleaseGate {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    fn concurrent_records(
        path: &Path,
        modes: [AccessMode; 8],
        operation: fn(&Path, AccessMode) -> Result<Record, String>,
    ) -> Vec<Record> {
        thread::scope(|scope| {
            let released = Arc::new(AtomicBool::new(false));
            let arrived = Arc::new(AtomicUsize::new(0));
            let _release_on_unwind = ReleaseGate(Arc::clone(&released));
            let workers: Vec<_> = modes
                .into_iter()
                .map(|mode| {
                    let released = Arc::clone(&released);
                    let arrived = Arc::clone(&arrived);
                    scope.spawn(move || {
                        arrived.fetch_add(1, Ordering::AcqRel);
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while !released.load(Ordering::Acquire) {
                            assert!(
                                Instant::now() < deadline,
                                "policy fixture gate not released"
                            );
                            thread::sleep(Duration::from_millis(1));
                        }
                        operation(path, mode).unwrap()
                    })
                })
                .collect();
            let deadline = Instant::now() + Duration::from_secs(5);
            while arrived.load(Ordering::Acquire) != 8 {
                assert!(
                    Instant::now() < deadline,
                    "policy fixture workers did not arrive"
                );
                thread::sleep(Duration::from_millis(1));
            }
            released.store(true, Ordering::Release);
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect()
        })
    }

    // The store intentionally bounds lock admission and can reject a contending
    // writer after LOCK_WAIT. This fixture verifies all eight writes eventually
    // publish distinct records, so retry only that pre-write rejection within a
    // separate finite test budget. Any other error remains a test failure.
    fn write_after_test_contention(path: &Path, mode: AccessMode) -> Result<Record, String> {
        const CONTENTION: &str = "Another relay policy update is still in progress";
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match write(path, mode) {
                Err(error) if error == CONTENTION && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                result => return result,
            }
        }
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "qc-access-policy-{label}-{}-{:032x}",
                std::process::id(),
                rand::random::<u128>()
            ));
            fs::create_dir(&path).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn policy_write_is_immediately_visible_to_independent_readers() {
        let scratch = Scratch::new("independent-readers");
        assert_eq!(effective_mode(&scratch.0), AccessMode::ReadOnly);
        assert!(!effective_remote_enabled(&scratch.0));
        let written = write(&scratch.0, AccessMode::Full).unwrap();
        assert!(!written.remote_enabled);
        for _ in 0..8 {
            assert_eq!(read(&scratch.0).unwrap(), Some(written.clone()));
            assert_eq!(effective_mode(&scratch.0), AccessMode::Full);
        }
    }

    #[test]
    fn migration_never_clobbers_a_choice_already_written() {
        let scratch = Scratch::new("migration-does-not-clobber");
        let user_choice = write(&scratch.0, AccessMode::ReadOnly).unwrap();
        let migration = initialize_if_absent(&scratch.0, AccessMode::Full).unwrap();
        assert_eq!(migration, user_choice);
        assert_eq!(read(&scratch.0).unwrap(), Some(user_choice));
    }

    #[test]
    fn precreated_private_directory_without_policy_is_still_first_initialization() {
        let scratch = Scratch::new("prepared-first-initialize");
        ensure_directory(&scratch.0).unwrap();
        assert!(!migration_is_complete(&scratch.0).unwrap());

        let migrated = initialize_if_absent(&scratch.0, AccessMode::Modify).unwrap();
        assert_eq!(migrated.mode, AccessMode::Modify);
        assert!(migrated.remote_enabled);
        assert_eq!(read(&scratch.0).unwrap(), Some(migrated));
    }

    #[test]
    fn pristine_migration_enables_remote_but_mode_changes_preserve_latch() {
        let scratch = Scratch::new("remote-latch");
        let migrated = initialize_if_absent(&scratch.0, AccessMode::Modify).unwrap();
        assert!(migrated.remote_enabled);
        assert_eq!(effective_mode(&scratch.0), AccessMode::Modify);
        assert!(effective_remote_enabled(&scratch.0));

        let disabled = update_remote_enabled(&scratch.0, false).unwrap();
        assert_eq!(disabled.mode, AccessMode::Modify);
        assert!(!effective_remote_enabled(&scratch.0));

        let mode_change = write(&scratch.0, AccessMode::Full).unwrap();
        assert_eq!(mode_change.mode, AccessMode::Full);
        assert!(!mode_change.remote_enabled);
        assert!(!effective_remote_enabled(&scratch.0));

        let reenabled = update_remote_enabled(&scratch.0, true).unwrap();
        assert_eq!(reenabled.mode, AccessMode::Full);
        assert!(effective_remote_enabled(&scratch.0));
    }

    #[test]
    fn legacy_or_repaired_records_default_remote_latch_to_disabled() {
        let scratch = Scratch::new("legacy-remote-latch");
        ensure_directory(&scratch.0).unwrap();
        let path = policy_path(&scratch.0);
        qc_owner_only_file::write_staged(
            &path,
            br#"{"schemaVersion":1,"revision":"0123456789abcdef0123456789abcdef","mode":"full"}"#,
        )
        .unwrap();
        assert!(!read(&scratch.0).unwrap().unwrap().remote_enabled);
        assert!(!effective_remote_enabled(&scratch.0));

        fs::remove_file(&path).unwrap();
        let repaired = write(&scratch.0, AccessMode::Full).unwrap();
        assert!(!repaired.remote_enabled);
        assert!(!effective_remote_enabled(&scratch.0));
    }

    #[test]
    fn deleted_policy_cannot_be_remigrated_from_stale_legacy_mode() {
        let scratch = Scratch::new("deleted-policy-stays-read-only");
        write(&scratch.0, AccessMode::ReadOnly).unwrap();
        fs::remove_file(policy_path(&scratch.0)).unwrap();

        assert!(read(&scratch.0).is_err());
        assert_eq!(effective_mode(&scratch.0), AccessMode::ReadOnly);
        assert!(initialize_if_absent(&scratch.0, AccessMode::Full).is_err());
        assert_eq!(effective_mode(&scratch.0), AccessMode::ReadOnly);
    }

    #[test]
    fn deleted_policy_directory_cannot_be_remigrated_from_stale_legacy_mode() {
        let scratch = Scratch::new("deleted-policy-directory-stays-disabled");
        initialize_if_absent(&scratch.0, AccessMode::Full).unwrap();
        write(&scratch.0, AccessMode::ReadOnly).unwrap();
        update_remote_enabled(&scratch.0, false).unwrap();
        fs::remove_dir_all(store_directory(&scratch.0)).unwrap();

        assert!(migration_is_complete(&scratch.0).unwrap());
        assert!(read(&scratch.0).is_err());
        assert_eq!(effective_mode(&scratch.0), AccessMode::ReadOnly);
        assert!(!effective_remote_enabled(&scratch.0));
        assert!(initialize_if_absent(&scratch.0, AccessMode::Full).is_err());
        assert!(!effective_remote_enabled(&scratch.0));
    }

    #[cfg(unix)]
    #[test]
    fn orphaned_lock_stage_hard_link_does_not_block_future_writes() {
        let scratch = Scratch::new("orphaned-lock-hard-link");
        write(&scratch.0, AccessMode::ReadOnly).unwrap();
        let lock = store_directory(&scratch.0).join(LOCK_NAME);
        let orphaned_stage = store_directory(&scratch.0).join(".orphaned-lock-stage");
        fs::hard_link(&lock, &orphaned_stage).unwrap();

        let next = write(&scratch.0, AccessMode::Full).unwrap();
        assert_eq!(next.mode, AccessMode::Full);
        assert_eq!(read(&scratch.0).unwrap(), Some(next));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn base_directory_creation_and_reads_preserve_private_linux_contract() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = Scratch::new("private-base");
        let base = scratch.0.join("new-app-data");
        ensure_directory(&base).unwrap();
        assert_eq!(
            fs::metadata(&base).unwrap().permissions().mode() & 0o777,
            0o700
        );

        fs::set_permissions(&base, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(read(&base).is_err());
        assert_eq!(effective_mode(&base), AccessMode::ReadOnly);
        assert!(write(&base, AccessMode::Full).is_err());
    }

    #[test]
    fn concurrent_writes_publish_complete_distinct_revisions() {
        let scratch = Scratch::new("concurrent-writes");
        let modes = [
            AccessMode::Full,
            AccessMode::Modify,
            AccessMode::Performance,
            AccessMode::ReadOnly,
            AccessMode::Full,
            AccessMode::Modify,
            AccessMode::Performance,
            AccessMode::ReadOnly,
        ];
        let records = concurrent_records(&scratch.0, modes, write_after_test_contention);
        let revisions: std::collections::BTreeSet<_> = records
            .iter()
            .map(|record| record.revision.clone())
            .collect();
        assert_eq!(revisions.len(), records.len());
        let final_record = read(&scratch.0).unwrap().unwrap();
        assert!(records.contains(&final_record));
    }

    #[test]
    fn concurrent_first_initializers_share_one_published_revision() {
        let scratch = Scratch::new("concurrent-initialize");
        let modes = [
            AccessMode::Full,
            AccessMode::Modify,
            AccessMode::Performance,
            AccessMode::ReadOnly,
            AccessMode::Full,
            AccessMode::Modify,
            AccessMode::Performance,
            AccessMode::ReadOnly,
        ];
        let records = concurrent_records(&scratch.0, modes, initialize_if_absent);
        assert!(records.iter().all(|record| *record == records[0]));
        assert_eq!(read(&scratch.0).unwrap(), Some(records[0].clone()));
    }

    #[test]
    fn missing_malformed_and_symlinked_policy_fail_closed() {
        let scratch = Scratch::new("fail-closed");
        assert_eq!(read(&scratch.0).unwrap(), None);
        assert_eq!(effective_mode(&scratch.0), AccessMode::ReadOnly);

        ensure_directory(&scratch.0).unwrap();
        let path = policy_path(&scratch.0);
        qc_owner_only_file::write_staged(&path, b"{").unwrap();
        assert!(read(&scratch.0).is_err());
        assert_eq!(effective_mode(&scratch.0), AccessMode::ReadOnly);
        fs::remove_file(&path).unwrap();

        let valid = write(&scratch.0, AccessMode::Full).unwrap();
        assert_eq!(valid.mode, AccessMode::Full);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read(&scratch.0).is_err());
            assert_eq!(effective_mode(&scratch.0), AccessMode::ReadOnly);
            fs::remove_file(&path).unwrap();
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let target = scratch.0.join("target.json");
            qc_owner_only_file::write_staged(&target, b"{}").unwrap();
            symlink(&target, &path).unwrap();
            assert!(read(&scratch.0).is_err());
            assert_eq!(effective_mode(&scratch.0), AccessMode::ReadOnly);
        }
    }

    #[test]
    fn partial_policy_fixture_start_unwind_releases_and_joins_the_started_worker() {
        let finished = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&finished);
        let result = std::panic::catch_unwind(move || {
            thread::scope(|scope| {
                let released = Arc::new(AtomicBool::new(false));
                let _release_on_unwind = ReleaseGate(Arc::clone(&released));
                scope.spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while !released.load(Ordering::Acquire) {
                        assert!(Instant::now() < deadline, "started fixture was stranded");
                        thread::sleep(Duration::from_millis(1));
                    }
                    observed.store(true, Ordering::Release);
                });
                panic!("injected next worker setup failure");
            })
        });
        assert!(result.is_err());
        assert!(
            finished.load(Ordering::Acquire),
            "scope returned before owned worker exit"
        );
    }
}
