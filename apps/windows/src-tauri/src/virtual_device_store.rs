//! Persistence for the demo (virtual) device.
//!
//! The demo device keeps one JSON document: its setlists, stored presets and
//! the preset being edited. The frontend owns the document's shape
//! (`packages/typescript/qc-ui/src/virtual-gateway.ts`); this side owns only
//! where it lives. The path is fixed here, never taken from the webview, so
//! these two commands can read and replace exactly one file and nothing else.
//!
//! Windows retains its existing local-data location. Unix uses the native
//! persistent user-state directory shared with the broker.

use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Far above any real library (256 slots per setlist of a few KB each), and
/// low enough that a runaway write cannot fill the disk.
const MAXIMUM_STORE_BYTES: usize = 64 * 1024 * 1024;

pub fn virtual_device_store_path() -> PathBuf {
    #[cfg(test)]
    let root = crate::runtime_health_path()
        .parent()
        .expect("test state root")
        .to_path_buf();
    #[cfg(all(not(test), unix))]
    let root = crate::broker_paths::app_local_data_dir();
    #[cfg(all(not(test), not(unix)))]
    let root = legacy_store_root();
    root.join("virtual-device").join("store.json")
}

#[cfg(not(test))]
fn legacy_store_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("QC Remote")
}

#[cfg(unix)]
fn prepare_store_directory(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        qc_native_platform::paths::prepare_directory(path)
    }
    #[cfg(target_os = "macos")]
    {
        qc_native_platform::paths_macos::prepare_directory(path)
    }
}

/// Copy an owned legacy document without ever replacing a concurrently saved
/// canonical document. The original is retained even when publication fails.
#[cfg(unix)]
fn migrate_legacy_store(canonical: &Path, legacy: &Path) -> Result<(), String> {
    migrate_legacy_store_for_user(canonical, legacy, unsafe { libc::geteuid() })
}

#[cfg(unix)]
fn migrate_legacy_store_for_user(canonical: &Path, legacy: &Path, uid: u32) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    match fs::symlink_metadata(canonical) {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let app_root = legacy
        .parent()
        .and_then(Path::parent)
        .ok_or("Missing legacy root")?;
    let base = app_root.parent().ok_or("Missing legacy base")?;
    // Resolve only the trusted OS/env base (Darwin /var and /tmp are aliases).
    // Every application-owned component below it is opened relative to a held
    // directory, with O_NOFOLLOW; untrusted final paths are never canonicalized.
    let base = match fs::canonicalize(base) {
        Ok(base) => base,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let base_handle = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&base)
        .map_err(|e| e.to_string())?;
    let open_child =
        |parent: &fs::File, name: &std::ffi::OsStr, directory: bool| -> std::io::Result<fs::File> {
            let name = CString::new(name.as_bytes())
                .map_err(|_| std::io::Error::other("Invalid legacy name"))?;
            let mut before = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe {
                libc::fstatat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    before.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let before = unsafe { before.assume_init() };
            // Ignore foreign data before entering even a 0700 app directory.
            if before.st_uid != uid {
                return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
            }
            let expected_type = if directory {
                libc::S_IFDIR
            } else {
                libc::S_IFREG
            };
            if before.st_mode & libc::S_IFMT != expected_type {
                return Err(std::io::Error::other(
                    "The previous demo path is not a regular owned document or directory",
                ));
            }
            let flags = libc::O_RDONLY
                | libc::O_NONBLOCK
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | if directory { libc::O_DIRECTORY } else { 0 };
            let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(unsafe { fs::File::from_raw_fd(fd) })
            }
        };
    let root = match open_child(
        &base_handle,
        app_root.file_name().ok_or("Missing legacy root name")?,
        true,
    ) {
        Ok(root) => root,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Could not safely open the previous demo folder: {error}"
            ))
        }
    };
    // A foreign user's /tmp/QC Remote is not this user's library. Ignore it
    // rather than blocking creation of a new native store.
    if root.metadata().map_err(|e| e.to_string())?.uid() != uid {
        return Ok(());
    }
    let mut component_path = PathBuf::new();
    for component in base.components() {
        component_path.push(component);
        let metadata = fs::symlink_metadata(&component_path).map_err(|e| e.to_string())?;
        let sticky_root = metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || (metadata.uid() != uid && metadata.uid() != 0)
            || (metadata.mode() & 0o022 != 0 && !sticky_root)
        {
            return Err("The previous demo store has an unsafe base directory.".into());
        }
    }
    if root.metadata().map_err(|e| e.to_string())?.mode() & 0o022 != 0 {
        return Err("The previous demo folder is writable by another user.".into());
    }
    let directory = match open_child(&root, std::ffi::OsStr::new("virtual-device"), true) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let metadata = directory.metadata().map_err(|e| e.to_string())?;
    if metadata.uid() != uid {
        return Ok(());
    }
    if metadata.mode() & 0o022 != 0 {
        return Err("The previous demo folder is writable by another user.".into());
    }
    let mut file = match open_child(&directory, std::ffi::OsStr::new("store.json"), false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if metadata.uid() != uid {
        return Ok(());
    }
    if !metadata.is_file() || metadata.mode() & 0o022 != 0 || metadata.nlink() != 1 {
        return Err("The previous demo store is not a safe owned regular file.".into());
    }
    let bytes = qc_owner_only_file::read_with_byte_budget(&mut file, MAXIMUM_STORE_BYTES)
        .map_err(|e| e.to_string())?;
    validate_store(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)?;
    let parent = canonical.parent().ok_or("Missing demo store folder")?;
    prepare_store_directory(parent)?;
    let staging = parent.join(format!(".migration-{:032x}.tmp", rand::random::<u128>()));
    let mut stage = StoreStage {
        path: staging.clone(),
        file: None,
        owned: false,
    };
    let mut output =
        qc_owner_only_file::create_new_owner_only(&staging).map_err(|e| e.to_string())?;
    stage.owned = true;
    output
        .write_all(&bytes)
        .and_then(|()| output.sync_all())
        .map_err(|e| e.to_string())?;
    drop(output);
    match fs::hard_link(&staging, canonical) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!(
            "Could not preserve the previous demo store: {error}"
        )),
    }
}

fn prepare_command_store() -> Result<PathBuf, String> {
    let path = virtual_device_store_path();
    #[cfg(unix)]
    {
        prepare_store_directory(path.parent().ok_or("Missing demo store folder")?)?;
        use std::os::unix::fs::MetadataExt;
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o022 != 0
            {
                return Err("The demo store is not a safe current-user document.".into());
            }
        }
        #[cfg(not(test))]
        migrate_legacy_store(
            &path,
            &legacy_store_root()
                .join("virtual-device")
                .join("store.json"),
        )?;
    }
    Ok(path)
}

/// The stored document, or `None` when the demo device has never been saved.
pub fn read_store(path: &Path) -> Result<Option<String>, String> {
    match fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(MAXIMUM_STORE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| format!("Could not read the demo device: {error}"))?;
            if bytes.len() > MAXIMUM_STORE_BYTES {
                return Err("The demo device document is too large to read.".into());
            }
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| format!("Could not read the demo device: {error}"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Could not read the demo device: {error}")),
    }
}

/// Replace the stored document. It must be a version 1 demo-device document;
/// anything else is refused rather than written over the user's library. The
/// new text is written beside the old file and renamed over it, so a failed
/// write leaves the previous document intact.
pub fn write_store(path: &Path, text: &str) -> Result<(), String> {
    validate_store(text)?;
    let directory = path
        .parent()
        .ok_or_else(|| "The demo device store has no parent directory.".to_string())?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create the demo device folder: {error}"))?;
    replace_store(path, text.as_bytes(), |file, bytes| file.write_all(bytes))
}

fn validate_store(text: &str) -> Result<(), String> {
    if text.len() > MAXIMUM_STORE_BYTES {
        return Err("The demo device document is too large to save.".into());
    }
    let document: Value = serde_json::from_str(text)
        .map_err(|error| format!("The demo device document is not valid JSON: {error}"))?;
    if document.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("The demo device document is not a version 1 virtual device document.".into());
    }
    Ok(())
}

/// A writer owns only its exclusively-created stage, including partial-write
/// and rename failures. Concurrent invocations never truncate or remove it.
struct StoreStage {
    path: PathBuf,
    file: Option<fs::File>,
    owned: bool,
}

impl Drop for StoreStage {
    fn drop(&mut self) {
        self.file.take();
        if self.owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn replace_store(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut fs::File, &[u8]) -> std::io::Result<()>,
) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or("The demo device store has no parent directory.")?;
    let mut stage = None;
    for _ in 0..8 {
        let staging = directory.join(format!(
            ".store-{}-{:032x}.tmp",
            std::process::id(),
            rand::random::<u128>()
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&staging) {
            Ok(file) => {
                stage = Some(StoreStage {
                    path: staging,
                    file: Some(file),
                    owned: true,
                });
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not save the demo device: {error}")),
        }
    }
    let mut stage = stage.ok_or("Could not create a unique demo device staging file.")?;
    write(stage.file.as_mut().expect("owned stage"), bytes)
        .and_then(|()| stage.file.as_ref().expect("owned stage").sync_all())
        .map_err(|error| format!("Could not save the demo device: {error}"))?;
    // Windows cannot rename an open file. Close our handle before publication.
    stage.file.take();
    fs::rename(&stage.path, path)
        .map_err(|error| format!("Could not save the demo device: {error}"))?;
    stage.owned = false;
    Ok(())
}

#[tauri::command]
pub fn read_virtual_device_store() -> Result<Option<String>, String> {
    read_store(&prepare_command_store()?)
}

#[tauri::command]
pub fn write_virtual_device_store(text: String) -> Result<(), String> {
    write_store(&prepare_command_store()?, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    impl Scratch {
        fn path(&self) -> PathBuf {
            self.0.join("virtual-device").join("store.json")
        }
    }

    fn scratch(name: &str) -> Scratch {
        let directory = std::env::temp_dir().join(format!(
            "qc-remote-virtual-store-{name}-{}-{:032x}",
            std::process::id(),
            rand::random::<u128>()
        ));
        fs::create_dir(&directory).expect("unique scratch root");
        Scratch(fs::canonicalize(directory).expect("canonical scratch root"))
    }

    #[test]
    fn the_store_lives_under_the_apps_local_data_folder() {
        let path = virtual_device_store_path();
        assert_eq!(
            path,
            crate::runtime_health_path()
                .parent()
                .unwrap()
                .join("virtual-device/store.json")
        );
    }

    #[test]
    fn a_missing_store_reads_as_none_and_a_write_round_trips() {
        let scratch = scratch("round-trip");
        let path = scratch.path();
        assert_eq!(read_store(&path).expect("read"), None);
        let text = "{\n  \"version\": 1,\n  \"session\": \"s\"\n}\n";
        write_store(&path, text).expect("write");
        assert_eq!(read_store(&path).expect("read").as_deref(), Some(text));
        assert!(
            !path.with_extension("json.tmp").exists(),
            "the staging file is renamed away"
        );
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn a_foreign_document_is_refused_and_leaves_the_library_intact() {
        let scratch = scratch("refuse");
        let path = scratch.path();
        let library = "{\"version\":1}";
        write_store(&path, library).expect("write");
        assert!(write_store(&path, "not json").is_err());
        assert!(write_store(&path, "{\"version\":2}").is_err());
        assert!(write_store(&path, "[1]").is_err());
        assert_eq!(read_store(&path).expect("read").as_deref(), Some(library));
    }

    #[test]
    fn failed_partial_write_and_failed_rename_remove_only_owned_staging() {
        let scratch = scratch("failures");
        let path = scratch.path();
        write_store(&path, "{\"version\":1}").unwrap();
        let foreign = path.with_extension("json.tmp");
        fs::write(&foreign, "another writer").unwrap();
        assert!(replace_store(&path, b"replacement", |file, _| {
            file.write_all(b"partial")?;
            Err(std::io::Error::other("injected write failure"))
        })
        .is_err());
        assert_eq!(
            read_store(&path).unwrap().as_deref(),
            Some("{\"version\":1}")
        );
        let blocked = path.parent().unwrap().join("directory-target");
        fs::create_dir(&blocked).unwrap();
        assert!(replace_store(&blocked, b"replacement", |file, bytes| file
            .write_all(bytes))
        .is_err());
        assert_eq!(fs::read_to_string(&foreign).unwrap(), "another writer");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 3);
    }

    #[test]
    fn concurrent_writes_publish_whole_documents_without_staging_leaks() {
        let scratch = scratch("concurrent");
        let path = scratch.path();
        let start = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            for index in 0..8 {
                let path = &path;
                let start = &start;
                scope.spawn(move || {
                    let text = format!(
                        "{{\"version\":1,\"writer\":{index},\"padding\":\"{}\"}}",
                        "x".repeat(8192)
                    );
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while !start.load(std::sync::atomic::Ordering::Acquire) {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "writer release timed out"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    write_store(path, &text).unwrap();
                });
            }
            start.store(true, std::sync::atomic::Ordering::Release);
        });
        let document: Value = serde_json::from_str(&read_store(&path).unwrap().unwrap()).unwrap();
        assert!(document["writer"].as_u64().unwrap() < 8);
        assert_eq!(document["padding"].as_str().unwrap().len(), 8192);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn oversized_existing_store_is_refused_on_read() {
        let scratch = scratch("read-limit");
        let path = scratch.path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(&path)
            .unwrap()
            .set_len(MAXIMUM_STORE_BYTES as u64 + 1)
            .unwrap();
        assert!(read_store(&path).unwrap_err().contains("too large"));
    }

    #[cfg(unix)]
    #[test]
    fn owned_legacy_is_copied_privately_and_retained_without_overwriting_canonical() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let scratch = scratch("migration");
        let legacy = scratch.0.join("legacy/virtual-device/store.json");
        write_store(&legacy, "{\"version\":1,\"old\":true}").unwrap();
        // Prior releases created ordinary 0644 files; those remain eligible.
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o644)).unwrap();
        let canonical = scratch.0.join("canonical/virtual-device/store.json");
        migrate_legacy_store(&canonical, &legacy).unwrap();
        assert_eq!(
            read_store(&canonical).unwrap(),
            read_store(&legacy).unwrap()
        );
        assert_eq!(fs::metadata(&canonical).unwrap().mode() & 0o777, 0o600);
        write_store(&canonical, "{\"version\":1,\"new\":true}").unwrap();
        migrate_legacy_store(&canonical, &legacy).unwrap();
        assert!(read_store(&canonical).unwrap().unwrap().contains("new"));
        assert!(read_store(&legacy).unwrap().unwrap().contains("old"));
        assert_eq!(
            fs::read_dir(canonical.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_legacy_paths_and_documents_are_not_imported() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let scratch = scratch("unsafe-migration");
        let legacy = scratch.0.join("legacy/virtual-device/store.json");
        let canonical = scratch.0.join("canonical/virtual-device/store.json");
        write_store(&legacy, "{\"version\":1}").unwrap();
        let link = scratch.0.join("linked");
        symlink(legacy.parent().unwrap().parent().unwrap(), &link).unwrap();
        assert!(migrate_legacy_store(&canonical, &link.join("virtual-device/store.json")).is_err());
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(migrate_legacy_store(&canonical, &legacy).is_err());
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&legacy, "{\"version\":2}").unwrap();
        assert!(migrate_legacy_store(&canonical, &legacy).is_err());
        assert!(!canonical.exists());
        assert!(legacy.exists());
    }

    #[cfg(unix)]
    #[test]
    fn failed_migration_retains_the_only_legacy_copy() {
        let scratch = scratch("failed-migration");
        let legacy = scratch.0.join("legacy/virtual-device/store.json");
        write_store(&legacy, "{\"version\":1}").unwrap();
        let blocked = scratch.0.join("blocked");
        fs::write(&blocked, "not a directory").unwrap();
        assert!(migrate_legacy_store(&blocked.join("store.json"), &legacy).is_err());
        assert_eq!(
            read_store(&legacy).unwrap().as_deref(),
            Some("{\"version\":1}")
        );
    }

    #[cfg(unix)]
    #[test]
    fn trusted_base_alias_works_but_symlinked_store_is_refused() {
        use std::os::unix::fs::symlink;
        let scratch = scratch("base-alias");
        let base = scratch.0.join("base");
        let legacy = base.join("QC Remote/virtual-device/store.json");
        write_store(&legacy, "{\"version\":1}").unwrap();
        let alias = scratch.0.join("base-alias");
        symlink(&base, &alias).unwrap();
        let canonical = scratch.0.join("canonical/virtual-device/store.json");
        migrate_legacy_store(
            &canonical,
            &alias.join("QC Remote/virtual-device/store.json"),
        )
        .unwrap();
        assert!(canonical.exists());
        fs::remove_file(&canonical).unwrap();
        fs::rename(&legacy, legacy.with_extension("original")).unwrap();
        symlink(legacy.with_extension("original"), &legacy).unwrap();
        assert!(migrate_legacy_store(&canonical, &legacy).is_err());
        assert!(!canonical.exists());
    }

    #[cfg(unix)]
    #[test]
    fn another_users_legacy_does_not_block_a_new_native_library() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = scratch("foreign-owner");
        let legacy = scratch.0.join("legacy/virtual-device/store.json");
        let canonical = scratch.0.join("canonical/virtual-device/store.json");
        write_store(&legacy, "{\"version\":1,\"foreign\":true}").unwrap();
        // Evaluate the real file metadata from a different user's perspective;
        // no chown, process environment mutation, or privileged fixture needed.
        fs::set_permissions(
            legacy.parent().unwrap().parent().unwrap(),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let other_uid = unsafe { libc::geteuid() }.wrapping_add(1);
        migrate_legacy_store_for_user(&canonical, &legacy, other_uid).unwrap();
        assert!(!canonical.exists());
        write_store(&canonical, "{\"version\":1,\"new\":true}").unwrap();
        assert!(read_store(&canonical).unwrap().unwrap().contains("new"));
        assert!(read_store(&legacy).unwrap().unwrap().contains("foreign"));
    }

    #[cfg(unix)]
    #[test]
    fn competing_migrations_publish_one_complete_copy_and_remove_only_their_stages() {
        let scratch = scratch("competing-migrations");
        let legacy = scratch.0.join("legacy/virtual-device/store.json");
        let canonical = scratch.0.join("canonical/virtual-device/store.json");
        let text = "{\"version\":1,\"session\":\"preserved\"}";
        write_store(&legacy, text).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let legacy = &legacy;
                let canonical = &canonical;
                scope.spawn(move || migrate_legacy_store(canonical, legacy).unwrap());
            }
        });
        assert_eq!(read_store(&canonical).unwrap().as_deref(), Some(text));
        assert_eq!(read_store(&legacy).unwrap().as_deref(), Some(text));
        assert_eq!(
            fs::read_dir(canonical.parent().unwrap()).unwrap().count(),
            1
        );
    }
    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_before_opening_it() {
        use std::os::unix::ffi::OsStrExt;
        let scratch = scratch("fifo");
        let legacy = scratch.0.join("legacy/virtual-device/store.json");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let name = std::ffi::CString::new(legacy.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let start = std::time::Instant::now();
        assert!(migrate_legacy_store(&scratch.path(), &legacy).is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert!(!scratch.path().exists());
    }
    #[cfg(unix)]
    #[test]
    fn foreign_inaccessible_app_root_is_skipped_before_open() {
        use std::os::unix::fs::PermissionsExt;
        struct Restore(PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
            }
        }
        let scratch = scratch("inaccessible-foreign");
        let root = scratch.0.join("legacy");
        let legacy = root.join("virtual-device/store.json");
        write_store(&legacy, "{\"version\":1}").unwrap();
        let restore = Restore(root.clone());
        fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
        migrate_legacy_store_for_user(
            &scratch.path(),
            &legacy,
            unsafe { libc::geteuid() }.wrapping_add(1),
        )
        .unwrap();
        assert!(!scratch.path().exists());
        drop(restore);
        assert_eq!(
            read_store(&legacy).unwrap().as_deref(),
            Some("{\"version\":1}")
        );
    }
}
