//! Platform credential storage. macOS deliberately never accesses Keychain.
#[cfg(not(target_os = "macos"))]
pub(crate) use keyring::{Entry, Error};

#[cfg(target_os = "macos")]
pub(crate) use macos::{Entry, Error};

#[cfg(target_os = "macos")]
mod macos {
    use std::{io, path::PathBuf};

    #[derive(Debug)]
    pub(crate) enum Error {
        NoEntry,
        Unavailable,
    }

    pub(crate) struct Entry {
        directory: PathBuf,
        filename: String,
    }

    impl Entry {
        pub(crate) fn new(service: &str, account: &str) -> Result<Self, Error> {
            // Fixed internal identifiers only: never turn untrusted text into a path.
            if !matches!(service, "QC Remote" | "QC Control")
                || account.is_empty()
                || !account
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err(Error::Unavailable);
            }
            Ok(Self {
                directory: crate::broker_paths::app_local_data_dir().join("credentials"),
                filename: format!("{}-{account}", service.replace(' ', "-")),
            })
        }

        fn path(&self) -> Result<PathBuf, Error> {
            qc_native_platform::paths_macos::prepare_directory(&self.directory)
                .map_err(|_| Error::Unavailable)?;
            Ok(self.directory.join(&self.filename))
        }

        pub(crate) fn get_password(&self) -> Result<String, Error> {
            let bytes = qc_owner_only_file::read_owner_only_bounded(&self.path()?, 1024 * 1024)
                .map_err(|error| match error.kind() {
                    io::ErrorKind::NotFound => Error::NoEntry,
                    _ => Error::Unavailable,
                })?;
            String::from_utf8(bytes).map_err(|_| Error::Unavailable)
        }

        pub(crate) fn set_password(&self, value: &str) -> Result<(), Error> {
            if value.len() > 1024 * 1024 {
                return Err(Error::Unavailable);
            }
            qc_owner_only_file::publish_atomically(&self.path()?, value.as_bytes())
                .map_err(|_| Error::Unavailable)
        }

        pub(crate) fn delete_credential(&self) -> Result<(), Error> {
            // Validate the existing file before removing it, including its ownership.
            self.get_password()?;
            std::fs::remove_file(self.path()?).map_err(|_| Error::Unavailable)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::{symlink, PermissionsExt};

        fn entry() -> Entry {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let base = std::fs::canonicalize(std::env::temp_dir()).unwrap();
            let directory = base.join(format!(
                "qc-credentials-{}-{sequence}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            Entry {
                directory,
                filename: "synthetic".into(),
            }
        }

        #[test]
        fn shared_entries_persist_replace_and_delete_without_keychain() {
            let entry = entry();
            assert!(matches!(entry.get_password(), Err(Error::NoEntry)));
            entry.set_password("synthetic-one").unwrap();
            let second = Entry {
                directory: entry.directory.clone(),
                filename: entry.filename.clone(),
            };
            assert_eq!(second.get_password().unwrap(), "synthetic-one");
            second.set_password("synthetic-two").unwrap();
            assert_eq!(entry.get_password().unwrap(), "synthetic-two");
            assert_eq!(
                std::fs::metadata(entry.path().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&entry.directory)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            entry.delete_credential().unwrap();
            assert!(matches!(second.get_password(), Err(Error::NoEntry)));
            std::fs::remove_dir(entry.directory).unwrap();
        }

        #[test]
        fn refuses_symlinks_and_broadened_permissions() {
            let entry = entry();
            entry.set_password("synthetic").unwrap();
            let path = entry.path().unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(matches!(entry.get_password(), Err(Error::Unavailable)));
            std::fs::remove_file(&path).unwrap();
            let target = entry.directory.join("target");
            std::fs::write(&target, "untouched").unwrap();
            symlink(&target, &path).unwrap();
            assert!(matches!(entry.get_password(), Err(Error::Unavailable)));
            assert!(entry.delete_credential().is_err());
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
            std::fs::remove_dir_all(entry.directory).unwrap();
        }
    }
}
