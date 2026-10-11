//! Linux paths are per-user and never inherit Windows' shared temp fallback.
use std::path::{Path, PathBuf};

const APP_DIRECTORY: &str = "org.freevia.qcremote";

/// Resolve without mutating the filesystem. Call [`prepare_directory`] before
/// reading or publishing credentials. Relative XDG values are ignored.
pub fn default_directory() -> PathBuf {
    resolve(
        std::env::var_os("XDG_RUNTIME_DIR")
            .as_deref()
            .map(Path::new),
        std::env::var_os("XDG_STATE_HOME").as_deref().map(Path::new),
        std::env::var_os("HOME").as_deref().map(Path::new),
    )
}

fn resolve(runtime: Option<&Path>, state: Option<&Path>, home: Option<&Path>) -> PathBuf {
    runtime
        .filter(|path| path.is_absolute())
        .map(|root| root.join(APP_DIRECTORY))
        .or_else(|| {
            state
                .filter(|path| path.is_absolute())
                .map(|root| root.join(APP_DIRECTORY))
        })
        .or_else(|| {
            home.filter(|path| path.is_absolute())
                .map(|root| root.join(".local/state").join(APP_DIRECTORY))
        })
        .unwrap_or_else(|| {
            // SAFETY: geteuid takes no pointers and has no preconditions.
            let uid = unsafe { libc::geteuid() };
            std::env::temp_dir().join(format!("qc-remote-{uid}"))
        })
}

/// Creates missing components with mode 0700, refusing symlinks, foreign
/// ownership and writable ancestors. Root-owned sticky /tmp is permitted;
/// the final directory itself must be owned by this uid and mode 0700.
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
    // SAFETY: geteuid takes no pointers and has no preconditions.
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
    use std::os::unix::fs::{symlink, PermissionsExt};

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            Self(std::env::temp_dir().join(format!("qc-linux-paths-{name}-{}", std::process::id())))
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn xdg_resolution_is_absolute_and_prioritized() {
        assert_eq!(
            resolve(
                Some(Path::new("/run/user/1000")),
                Some(Path::new("/state")),
                None
            ),
            Path::new("/run/user/1000/org.freevia.qcremote")
        );
        assert_eq!(
            resolve(Some(Path::new("relative")), Some(Path::new("/state")), None),
            Path::new("/state/org.freevia.qcremote")
        );
        assert_eq!(
            resolve(
                None,
                Some(Path::new("relative")),
                Some(Path::new("/home/me"))
            ),
            Path::new("/home/me/.local/state/org.freevia.qcremote")
        );
    }

    #[test]
    fn creates_private_components_and_refuses_public_directory() {
        let scratch = Scratch::new("permissions");
        let path = scratch.0.join("nested");
        prepare_directory(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(prepare_directory(&path).is_err());
    }

    #[test]
    fn refuses_symlinks_and_parent_traversal() {
        let scratch = Scratch::new("symlink");
        prepare_directory(&scratch.0).unwrap();
        symlink(&scratch.0, scratch.0.join("link")).unwrap();
        assert!(prepare_directory(&scratch.0.join("link/child")).is_err());
        assert!(prepare_directory(&scratch.0.join("../other")).is_err());
        assert!(prepare_directory(Path::new("relative")).is_err());
    }
}
