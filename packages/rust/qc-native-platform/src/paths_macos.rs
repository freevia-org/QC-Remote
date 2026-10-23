//! Persistent macOS state shared by the GUI and broker.
use std::path::{Path, PathBuf};

#[cfg(not(feature = "macos-app-store"))]
const APP_DIRECTORY: &str = "org.freevia.qcremote";

#[cfg(feature = "macos-app-store")]
pub fn default_directory() -> PathBuf {
    crate::app_store::shared_directory()
        .expect("Mac App Store shared broker storage is unavailable")
}

#[cfg(not(feature = "macos-app-store"))]
pub fn default_directory() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|home| {
            home.join("Library")
                .join("Application Support")
                .join(APP_DIRECTORY)
        })
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("qc-remote-{}", unsafe { libc::geteuid() }))
        })
}

/// Create a private 0700 app directory and reject symlinks or writable
/// ancestors before the broker stores descriptors, settings or credentials.
pub fn prepare_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    use std::path::Component;
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(
            "The broker directory must be an absolute path without parent traversal".into(),
        );
    }
    // SAFETY: geteuid has no pointer arguments or preconditions.
    let uid = unsafe { libc::geteuid() };
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::DirBuilder::new().mode(0o700).create(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(format!("Create {}: {error}", current.display())),
                }
                std::fs::symlink_metadata(&current).map_err(|error| error.to_string())?
            }
            Err(error) => return Err(format!("Inspect {}: {error}", current.display())),
        };
        let sticky_root = metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || (metadata.uid() != uid && metadata.uid() != 0)
            || (metadata.mode() & 0o022 != 0 && !sticky_root)
        {
            return Err(format!(
                "Unsafe broker directory component: {}",
                current.display()
            ));
        }
        if current == path && (metadata.uid() != uid || metadata.mode() & 0o777 != 0o700) {
            return Err(format!(
                "Broker directory must be owned by the current user with mode 0700: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is after the Unix epoch")
                .as_nanos();
            let path = std::fs::canonicalize(std::env::temp_dir())
                .expect("canonicalize system temporary directory")
                .join(format!(
                    "qc-macos-private-paths-{}-{nonce}",
                    std::process::id()
                ));
            std::fs::create_dir(&path).expect("create isolated path fixture");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("secure isolated path fixture");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove isolated path fixture");
        }
    }

    #[test]
    fn creates_a_private_application_directory_and_rejects_a_broadened_mode() {
        let scratch = Scratch::new();
        let application_data = scratch.0.join("Application Support").join("QC Remote");

        prepare_directory(&application_data).expect("create private application data path");
        let metadata = std::fs::symlink_metadata(&application_data).expect("read leaf metadata");
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        assert_eq!(metadata.mode() & 0o777, 0o700);

        std::fs::set_permissions(&application_data, std::fs::Permissions::from_mode(0o755))
            .expect("broaden fixture mode");
        assert!(prepare_directory(&application_data).is_err());
    }

    #[test]
    fn refuses_a_symlink_in_the_application_data_path() {
        let scratch = Scratch::new();
        let target = scratch.0.join("real");
        let link = scratch.0.join("linked");
        std::fs::create_dir(&target).expect("create real parent");
        std::os::unix::fs::symlink(&target, &link).expect("create parent symlink");

        assert!(prepare_directory(&link.join("QC Remote")).is_err());
    }
}
