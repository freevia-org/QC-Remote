//! Persistent Linux state shared by the GUI, resident host and broker.
//! Runtime descriptors carry boot-bound identities, so stale records are safe
//! across reboots while tray settings and MCP grants must survive them.
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
    state
        .filter(|path| path.is_absolute())
        .map(|root| root.join(APP_DIRECTORY))
        .or_else(|| {
            home.filter(|path| path.is_absolute())
                .map(|root| root.join(".local/state").join(APP_DIRECTORY))
        })
        .or_else(|| {
            runtime
                .filter(|path| path.is_absolute())
                .map(|root| root.join(APP_DIRECTORY))
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
/// Group-write on an ancestor is tolerated only for a directory this uid owns
/// whose group is this user's private primary group (see
/// [`directory_component_is_safe`]); stock Ubuntu creates `~/.local` and
/// `~/.config` as 0775 for that reason.
pub fn prepare_directory(path: &Path) -> Result<(), String> {
    // SAFETY: geteuid takes no pointers and has no preconditions.
    let uid = unsafe { libc::geteuid() };
    prepare_directory_as(path, uid, private_primary_group())
}

fn prepare_directory_as(path: &Path, uid: u32, private_gid: Option<u32>) -> Result<(), String> {
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
        if !directory_component_is_safe(&metadata, uid, private_gid) {
            return Err(format!(
                "Unsafe broker directory component: {}{}",
                current.display(),
                unsafe_component_hint(&metadata)
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

/// Whether one path component may sit on the way to a private directory.
/// It must be a real directory (never a symlink) owned by `uid` or root, and
/// must not be writable by anyone else: world-write is refused unless it is
/// root's sticky directory (/tmp), and group-write is accepted only when the
/// directory is owned by `uid` and its group is `private_gid`, the caller's
/// user-private primary group.
pub fn directory_component_is_safe(
    metadata: &std::fs::Metadata,
    uid: u32,
    private_gid: Option<u32>,
) -> bool {
    use std::os::unix::fs::MetadataExt;
    !metadata.file_type().is_symlink()
        && metadata.is_dir()
        && mode_is_safe(
            metadata.uid(),
            metadata.gid(),
            metadata.mode(),
            uid,
            private_gid,
        )
}

fn mode_is_safe(owner: u32, group: u32, mode: u32, uid: u32, private_gid: Option<u32>) -> bool {
    if owner != uid && owner != 0 {
        return false;
    }
    if mode & 0o022 == 0 {
        return true;
    }
    if owner == 0 && mode & 0o1000 != 0 {
        return true;
    }
    mode & 0o002 == 0 && owner == uid && private_gid == Some(group)
}

fn unsafe_component_hint(metadata: &std::fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    let mode = metadata.mode() & 0o7777;
    if mode & 0o020 != 0 && mode & 0o002 == 0 {
        format!(
            " (mode {mode:04o}: group-write is accepted only on your own directories in your private group; run chmod g-w on it)"
        )
    } else {
        format!(" (mode {mode:04o})")
    }
}

/// The caller's primary gid when that group is private to the user: its
/// /etc/group entry lists no member other than the user and no other account
/// in /etc/passwd uses it as primary group. Anything unreadable or ambiguous
/// yields `None`, which keeps group-write refused.
pub fn private_primary_group() -> Option<u32> {
    // SAFETY: geteuid and getegid take no pointers and have no preconditions.
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    let user = user_name(uid)?;
    let members = group_members(gid)?;
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    group_is_private(uid, gid, &user, &members, &passwd).then_some(gid)
}

fn group_is_private(uid: u32, gid: u32, user: &str, members: &[String], passwd: &str) -> bool {
    if members.iter().any(|member| member != user) {
        return false;
    }
    !passwd.lines().any(|line| {
        let mut fields = line.split(':');
        let (Some(_), Some(_), Some(entry_uid), Some(entry_gid)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return false;
        };
        entry_gid.parse() == Ok(gid) && entry_uid.parse() != Ok(uid)
    })
}

fn user_name(uid: u32) -> Option<String> {
    let mut buffer = vec![0u8; 4096];
    loop {
        // SAFETY: a zeroed passwd is a valid out-parameter; every pointer
        // handed to getpwuid_r outlives the call and the length is accurate.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if status == libc::ERANGE && buffer.len() < (1 << 20) {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 || found.is_null() {
            return None;
        }
        // SAFETY: on success pw_name points at a NUL-terminated string in `buffer`.
        return unsafe { std::ffi::CStr::from_ptr(entry.pw_name) }
            .to_str()
            .ok()
            .map(str::to_owned);
    }
}

fn group_members(gid: u32) -> Option<Vec<String>> {
    let mut buffer = vec![0u8; 4096];
    loop {
        // SAFETY: as in `user_name`, for getgrgid_r.
        let mut entry: libc::group = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::group = std::ptr::null_mut();
        let status = unsafe {
            libc::getgrgid_r(
                gid,
                &mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if status == libc::ERANGE && buffer.len() < (1 << 20) {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 || found.is_null() {
            return None;
        }
        let mut members = Vec::new();
        let mut cursor = entry.gr_mem;
        // SAFETY: gr_mem is a NULL-terminated array of NUL-terminated strings
        // stored in `buffer`, which is alive for this whole block.
        unsafe {
            while !cursor.is_null() && !(*cursor).is_null() {
                members.push(std::ffi::CStr::from_ptr(*cursor).to_str().ok()?.to_owned());
                cursor = cursor.add(1);
            }
        }
        return Some(members);
    }
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
            Path::new("/state/org.freevia.qcremote")
        );
        assert_eq!(
            resolve(Some(Path::new("relative")), Some(Path::new("/state")), None),
            Path::new("/state/org.freevia.qcremote")
        );
        assert_eq!(
            resolve(
                Some(Path::new("/run/user/1000")),
                Some(Path::new("relative")),
                Some(Path::new("/home/me"))
            ),
            Path::new("/home/me/.local/state/org.freevia.qcremote")
        );
        assert_eq!(
            resolve(Some(Path::new("/run/user/1000")), None, None),
            Path::new("/run/user/1000/org.freevia.qcremote")
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

    #[test]
    fn mode_policy_matrix() {
        let (me, gid) = (1000, 1000);
        let private = Some(gid);
        // Stock Ubuntu: own directory, private group, 0775.
        assert!(mode_is_safe(me, gid, 0o40775, me, private));
        assert!(mode_is_safe(me, gid, 0o40755, me, private));
        assert!(mode_is_safe(me, gid, 0o40700, me, private));
        // Shared group, or privacy unknown: group-write refused.
        assert!(!mode_is_safe(me, 100, 0o40775, me, private));
        assert!(!mode_is_safe(me, gid, 0o40775, me, None));
        // World-write refused even in the private group, sticky or not.
        assert!(!mode_is_safe(me, gid, 0o40777, me, private));
        assert!(!mode_is_safe(me, gid, 0o40757, me, private));
        assert!(!mode_is_safe(me, gid, 0o41777, me, private));
        // Foreign owners refused; root only without group-write unless sticky.
        assert!(!mode_is_safe(1001, gid, 0o40700, me, private));
        assert!(!mode_is_safe(1001, gid, 0o40775, me, private));
        assert!(mode_is_safe(0, 0, 0o40755, me, private));
        assert!(!mode_is_safe(0, gid, 0o40775, me, private));
        assert!(mode_is_safe(0, 0, 0o41777, me, private));
    }

    #[test]
    fn private_group_needs_sole_membership() {
        let passwd = "root:x:0:0::/root:/bin/sh\ntester:x:1000:1000::/home/tester:/bin/sh\n";
        assert!(group_is_private(1000, 1000, "tester", &[], passwd));
        assert!(group_is_private(
            1000,
            1000,
            "tester",
            &["tester".into()],
            passwd
        ));
        assert!(!group_is_private(
            1000,
            1000,
            "tester",
            &["tester".into(), "other".into()],
            passwd
        ));
        let shared = format!("{passwd}guest:x:1001:1000::/home/guest:/bin/sh\n");
        assert!(!group_is_private(1000, 1000, "tester", &[], &shared));
        assert!(group_is_private(
            1000,
            1000,
            "tester",
            &[],
            "garbage\n\n::\n"
        ));
    }

    fn effective_ids() -> (u32, u32) {
        // SAFETY: neither call takes pointers or has preconditions.
        unsafe { (libc::geteuid(), libc::getegid()) }
    }

    fn group_writable_parent(name: &str, mode: u32) -> (Scratch, PathBuf) {
        let scratch = Scratch::new(name);
        std::fs::create_dir_all(&scratch.0).unwrap();
        std::fs::set_permissions(&scratch.0, std::fs::Permissions::from_mode(mode)).unwrap();
        let target = scratch.0.join("state");
        (scratch, target)
    }

    #[test]
    fn accepts_private_group_writable_ancestor() {
        let (uid, gid) = effective_ids();
        let (_scratch, target) = group_writable_parent("private-0775", 0o775);
        prepare_directory_as(&target, uid, Some(gid)).unwrap();
    }

    #[test]
    fn refuses_group_writable_ancestor_in_shared_group() {
        let (uid, _) = effective_ids();
        let (_scratch, target) = group_writable_parent("shared-0775", 0o775);
        let error = prepare_directory_as(&target, uid, None).unwrap_err();
        assert!(
            error.contains("Unsafe broker directory component"),
            "{error}"
        );
        assert!(error.contains("0775"), "{error}");
    }

    #[test]
    fn refuses_world_writable_ancestor_even_in_private_group() {
        let (uid, gid) = effective_ids();
        let (_scratch, target) = group_writable_parent("world-0777", 0o777);
        assert!(prepare_directory_as(&target, uid, Some(gid)).is_err());
    }

    #[test]
    fn refuses_group_writable_ancestor_with_foreign_owner() {
        let (uid, gid) = effective_ids();
        let (_scratch, target) = group_writable_parent("foreign-0775", 0o775);
        // Present a different effective uid so the scratch directory is
        // someone else's (root-owned is never group-write-safe either).
        assert!(prepare_directory_as(&target, uid.wrapping_add(1), Some(gid)).is_err());
    }

    #[test]
    fn group_writable_ancestor_does_not_excuse_a_symlink() {
        let (uid, gid) = effective_ids();
        let (scratch, _) = group_writable_parent("symlink-0775", 0o775);
        let real = scratch.0.join("real");
        std::fs::create_dir(&real).unwrap();
        symlink(&real, scratch.0.join("link")).unwrap();
        assert!(prepare_directory_as(&scratch.0.join("link/state"), uid, Some(gid)).is_err());
    }
}
