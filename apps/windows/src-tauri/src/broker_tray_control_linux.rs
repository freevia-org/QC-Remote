//! Linux resident-host launch and XDG autostart used by the shared tray
//! settings controller. The broker's owner-only resident marker is the
//! authority for status; process names are never used to infer ownership.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use super::serving_broker_published;

const AUTOSTART_FILE: &str = "qc-remote.desktop";
const OWNERSHIP_MARKER: &str = "X-QCRemote-Managed=org.freevia.qcremote-resident-v1";
const STARTUP_BOUND: Duration = Duration::from_secs(3);
const REAPER_POLL: Duration = Duration::from_millis(100);
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub(super) fn is_running(directory: &Path) -> bool {
    qc_native_platform::resident_marker::is_running(directory)
}

pub(super) fn locate() -> Result<PathBuf, String> {
    let app = std::env::current_exe()
        .map_err(|error| format!("Locate the running QC Remote executable: {error}"))?;
    let directory = app
        .parent()
        .ok_or("The QC Remote executable has no parent directory")?;
    locate_resident_host(directory)
        .ok_or_else(|| "The qc-device-broker sidecar is missing beside QC Remote".to_owned())
}

fn locate_resident_host(directory: &Path) -> Option<PathBuf> {
    let architecture = std::env::consts::ARCH;
    [
        "qc-device-broker",
        &format!("qc-device-broker-{architecture}-unknown-linux-gnu"),
    ]
    .into_iter()
    .map(|name| directory.join(name))
    .find(|candidate| candidate.is_file())
}

pub(super) fn spawn(
    executable: &Path,
    directory: &Path,
    supersede_user_quit: bool,
) -> Result<(), String> {
    // Establish process-wide child ownership before a host can be spawned.
    let reaper = reaper_children()?;
    let mut command = Command::new(executable);
    command
        .arg("--resident")
        .arg("--directory")
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    if !supersede_user_quit {
        command.arg("--keep-user-quit");
    }
    let mut child = crate::spawn_with_broker_stderr(&mut command)
        .map_err(|error| format!("Start the QC Remote background host: {error}"))?;
    let child_pid = child.id();
    let child_identity = qc_native_platform::process::os_process_created_at(child_pid);
    let deadline = Instant::now() + STARTUP_BOUND;
    let mut clean_exit = None;

    loop {
        if is_running(directory) {
            send_to_reaper(reaper, child);
            return Ok(());
        }

        if clean_exit.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => clean_exit = Some(status),
                Ok(None) => {}
                Err(error) => {
                    return stop_unpublished_child(
                        child,
                        child_identity,
                        directory,
                        reaper,
                        format!("Check QC Remote background host startup: {error}"),
                    );
                }
            }
        }

        if let Some(status) = clean_exit {
            if !status.success() {
                return Err(format!(
                    "The QC Remote background host exited during startup ({status})"
                ));
            }
            if !supersede_user_quit
                && crate::device_broker_descriptor::read_user_quit(
                    &crate::device_broker_descriptor::device_broker_descriptor_path(directory),
                )
                .is_some()
            {
                return Err(
                    "The QC Remote background host was not started: QC Remote was quit, and only Connect or reopening QC Remote starts it again".to_owned(),
                );
            }
            if !serving_broker_published(directory) {
                return Err("The QC Remote background host exited before publishing a broker descriptor or resident marker".to_owned());
            }
            // Another process may already serve the state directory and turn
            // resident after observing this setting. Wait for that exact
            // resident marker; a live broker descriptor alone is not a tray.
        }

        if Instant::now() >= deadline {
            if is_running(directory) {
                send_to_reaper(reaper, child);
                return Ok(());
            }
            return stop_unpublished_child(
                child,
                child_identity,
                directory,
                reaper,
                format!(
                    "The QC Remote background host did not become resident within {} seconds",
                    STARTUP_BOUND.as_secs()
                ),
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn stop_unpublished_child(
    mut child: Child,
    child_identity: Option<u64>,
    directory: &Path,
    reaper: &Arc<Mutex<Vec<Child>>>,
    error: String,
) -> Result<(), String> {
    // The broker is allowed to become resident between the caller's last
    // marker read and this cleanup. Hand it to the reaper rather than stopping
    // a now-published host.
    if is_running(directory) || serving_broker_published(directory) {
        send_to_reaper(reaper, child);
        return Err(error);
    }

    let exact_child = child_identity.is_some_and(|identity| {
        qc_native_platform::process::os_process_is_alive(child.id())
            && qc_native_platform::process::os_process_created_at(child.id()) == Some(identity)
    });
    if exact_child {
        let _ = child.kill();
    }
    // The process-wide reaper retains the handle until it exits. Never block
    // desktop startup in an unbounded Child::wait fallback.
    send_to_reaper(reaper, child);
    Err(error)
}

fn send_to_reaper(reaper: &Arc<Mutex<Vec<Child>>>, child: Child) {
    reaper
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(child);
}

/// Spawn a short-lived Linux desktop helper and transfer its handle to the
/// process-wide reaper, so completed launches do not remain zombies.
pub(crate) fn spawn_owned(mut command: Command) -> Result<(), String> {
    let reaper = reaper_children()?;
    let child = command
        .spawn()
        .map_err(|error| format!("Could not start desktop helper: {error}"))?;
    send_to_reaper(reaper, child);
    Ok(())
}

static CHILD_REAPER: OnceLock<Result<Arc<Mutex<Vec<Child>>>, String>> = OnceLock::new();

fn reaper_children() -> Result<&'static Arc<Mutex<Vec<Child>>>, String> {
    CHILD_REAPER
        .get_or_init(|| {
            let children = Arc::new(Mutex::new(Vec::<Child>::new()));
            let reaper_children = Arc::clone(&children);
            thread::Builder::new()
                .name("qc-broker-child-reaper".to_owned())
                .spawn(move || reap_children(reaper_children))
                .map(|_| children)
                .map_err(|error| format!("Start QC Remote child reaper: {error}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn reap_children(children: Arc<Mutex<Vec<Child>>>) {
    loop {
        thread::sleep(REAPER_POLL);
        let mut children = children
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut index = 0;
        while index < children.len() {
            match children[index].try_wait() {
                Ok(Some(_)) => {
                    // try_wait already reaped this child; wait returns its
                    // cached exit status without blocking the reaper.
                    let _ = children.swap_remove(index).wait();
                }
                Ok(None) | Err(_) => index += 1,
            }
        }
    }
}

pub(super) fn set_run_entry(executable: &Path, directory: &Path) -> Result<(), String> {
    let autostart = autostart_directory()?;
    let uid = current_uid()?;
    ensure_autostart_directory(&autostart, uid)?;
    write_autostart_entry(&autostart, executable, directory, uid)
}

fn write_autostart_entry(
    autostart: &Path,
    executable: &Path,
    directory: &Path,
    uid: u32,
) -> Result<(), String> {
    let contents = desktop_entry(executable, directory)?;
    let target = autostart.join(AUTOSTART_FILE);
    match fs::symlink_metadata(&target) {
        Ok(_) => {
            let existing = read_owned_entry(&target, uid)?;
            if !is_our_entry(&existing) {
                return Err(format!(
                    "Refusing to replace an unrelated autostart entry: {}",
                    target.display()
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Inspect QC Remote autostart entry: {error}")),
    }
    atomic_write(autostart, &target, contents.as_bytes())
}

pub(super) fn remove_run_entry() -> Result<(), String> {
    let autostart = autostart_directory()?;
    remove_autostart_entry(&autostart, current_uid()?)
}

fn remove_autostart_entry(autostart: &Path, uid: u32) -> Result<(), String> {
    let target = autostart.join(AUTOSTART_FILE);
    let existing = match read_owned_entry(&target, uid) {
        Ok(contents) => contents,
        Err(_)
            if fs::symlink_metadata(&target)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(())
        }
        Err(error) => return Err(error),
    };
    if !is_our_entry(&existing) {
        return Err(format!(
            "Refusing to remove an unrelated autostart entry: {}",
            target.display()
        ));
    }
    fs::remove_file(&target).map_err(|error| format!("Remove QC Remote autostart entry: {error}"))
}

fn autostart_directory() -> Result<PathBuf, String> {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".config"))
        })
        .ok_or_else(|| "No absolute XDG_CONFIG_HOME or HOME is available".to_owned())?;
    Ok(root.join("autostart"))
}

fn current_uid() -> Result<u32, String> {
    fs::metadata(qc_native_platform::paths::default_directory())
        .map(|metadata| metadata.uid())
        .map_err(|error| format!("Read the owner of the QC Remote state directory: {error}"))
}

fn ensure_autostart_directory(path: &Path, uid: u32) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("The XDG autostart directory must be an absolute safe path".to_owned());
    }
    // Create one component at a time so a symlink or shared writable parent is
    // refused before any entry can be written through it.
    let private_gid = qc_native_platform::paths::private_primary_group();
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => validate_directory_component(&current, &metadata, uid, private_gid)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::DirBuilder::new().mode(0o700).create(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(format!("Create {}: {error}", current.display())),
                }
                let metadata = fs::symlink_metadata(&current)
                    .map_err(|error| format!("Inspect {}: {error}", current.display()))?;
                validate_directory_component(&current, &metadata, uid, private_gid)?;
            }
            Err(error) => return Err(format!("Inspect {}: {error}", current.display())),
        }
    }
    Ok(())
}

/// Group-write is tolerated only on the user's own directories in their
/// private primary group (stock Ubuntu makes `~/.config` 0775); everything
/// else follows `qc_native_platform::paths::directory_component_is_safe`.
fn validate_directory_component(
    path: &Path,
    metadata: &fs::Metadata,
    uid: u32,
    private_gid: Option<u32>,
) -> Result<(), String> {
    if !qc_native_platform::paths::directory_component_is_safe(metadata, uid, private_gid) {
        return Err(format!(
            "Unsafe XDG autostart directory component: {} (mode {:04o}; group-write is accepted only on your own directories in your private group)",
            path.display(),
            metadata.mode() & 0o7777
        ));
    }
    Ok(())
}

fn read_owned_entry(path: &Path, uid: u32) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Inspect QC Remote autostart entry: {error}"))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
    {
        return Err(format!(
            "Refusing an unsafe QC Remote autostart entry: {}",
            path.display()
        ));
    }
    fs::read(path).map_err(|error| format!("Read QC Remote autostart entry: {error}"))
}

fn is_our_entry(contents: &[u8]) -> bool {
    std::str::from_utf8(contents)
        .is_ok_and(|text| text.lines().any(|line| line == OWNERSHIP_MARKER))
}

fn desktop_entry(executable: &Path, directory: &Path) -> Result<String, String> {
    let executable = quote_desktop_argument(executable, true)?;
    let directory = quote_desktop_argument(directory, false)?;
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=QC Remote Resident Host\nNoDisplay=true\nTerminal=false\n{OWNERSHIP_MARKER}\nExec={executable} --resident --directory {directory} --keep-user-quit\n"
    ))
}

fn quote_desktop_argument(path: &Path, executable: bool) -> Result<String, String> {
    let value = path
        .to_str()
        .ok_or("Autostart command path is not valid Unicode")?;
    if value.chars().any(char::is_control) {
        return Err("Autostart command path contains a control character".to_owned());
    }
    if executable && value.contains('=') {
        return Err("Autostart executable path contains a reserved equal sign".to_owned());
    }
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            // Exec is itself a quoted command inside a desktop-entry string:
            // the key-file parser consumes one escaping layer before Exec.
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '$' | '`' => {
                quoted.push_str("\\\\");
                quoted.push(character);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

fn atomic_write(directory: &Path, target: &Path, contents: &[u8]) -> Result<(), String> {
    let temporary = directory.join(format!(
        ".{AUTOSTART_FILE}.{}.{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed),
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| format!("Create temporary QC Remote autostart entry: {error}"))?;
    let result = (|| {
        file.write_all(contents)
            .map_err(|error| format!("Write QC Remote autostart entry: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Flush QC Remote autostart entry: {error}"))?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Protect QC Remote autostart entry: {error}"))?;
        fs::rename(&temporary, target)
            .map_err(|error| format!("Publish QC Remote autostart entry: {error}"))?;
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("Flush XDG autostart directory: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            Self(root)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn locates_only_linux_broker_names_beside_the_app() {
        let temporary = TestDirectory::new("qc-linux-sidecar");
        let root = &temporary.0;
        let sidecar = root.join("qc-device-broker");
        assert_eq!(locate_resident_host(root), None);
        fs::write(&sidecar, b"broker").unwrap();
        assert_eq!(locate_resident_host(root), Some(sidecar));
    }

    #[test]
    fn running_status_uses_the_broker_resident_marker_not_an_image_name() {
        let root = TestDirectory::new("qc-linux-resident-status");
        qc_native_platform::paths::prepare_directory(&root.0).unwrap();
        assert!(!is_running(&root.0));
        let marker = qc_native_platform::resident_marker::ResidentMarker::acquire(&root.0).unwrap();
        assert!(is_running(&root.0));
        drop(marker);
        assert!(!is_running(&root.0));
    }

    #[test]
    fn child_reaper_owns_a_live_child_until_exit_without_blocking_the_caller() {
        let reaper = reaper_children().unwrap();
        let child = Command::new("sh")
            .args(["-c", "sleep 1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let started = Instant::now();
        send_to_reaper(reaper, child);
        assert!(started.elapsed() < STARTUP_BOUND);
        assert!(reaper
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|child| child.id() == pid));

        while started.elapsed() < Duration::from_secs(3) {
            let retained = reaper
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .any(|child| child.id() == pid);
            if !retained {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("child reaper retained exited child {pid} beyond the bounded test wait");
    }

    #[test]
    fn desktop_exec_quotes_arguments_and_neutralizes_field_codes() {
        let entry = desktop_entry(
            Path::new("/opt/QC Remote/bin/host%f\\name\"$`"),
            Path::new("/home/me/.local/state/QC Remote"),
        )
        .unwrap();
        assert!(entry.contains(
            "Exec=\"/opt/QC Remote/bin/host%%f\\\\\\\\name\\\\\"\\\\$\\\\`\" --resident --directory \"/home/me/.local/state/QC Remote\" --keep-user-quit"
        ));
        assert!(entry.contains(OWNERSHIP_MARKER));
        let exec_line = entry
            .lines()
            .find(|line| line.starts_with("Exec="))
            .expect("desktop entry has an Exec line");
        assert!(!exec_line.contains('\n'));
        assert!(quote_desktop_argument(Path::new("/tmp/with\nnewline"), true).is_err());
        assert!(quote_desktop_argument(Path::new("/tmp/qc=broker"), true).is_err());
        assert!(quote_desktop_argument(Path::new("/state/qc=broker"), false).is_ok());
    }

    #[test]
    fn autostart_replaces_and_removes_only_its_private_marked_entry() {
        let root = TestDirectory::new("qc-linux-autostart");
        let directory = root.0.join("autostart");
        let uid = fs::metadata(&root.0).unwrap().uid();
        ensure_autostart_directory(&directory, uid).unwrap();
        let target = directory.join(AUTOSTART_FILE);
        let first = desktop_entry(Path::new("/opt/qc-broker"), Path::new("/state/qc")).unwrap();
        atomic_write(&directory, &target, first.as_bytes()).unwrap();
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(is_our_entry(&read_owned_entry(&target, uid).unwrap()));

        write_autostart_entry(
            &directory,
            Path::new("/opt/new broker"),
            Path::new("/state/qc"),
            uid,
        )
        .unwrap();
        assert!(String::from_utf8(fs::read(&target).unwrap())
            .unwrap()
            .contains("/opt/new broker"));

        fs::write(&target, b"[Desktop Entry]\nName=Someone else's autostart\n").unwrap();
        let unrelated = fs::read(&target).unwrap();
        assert!(!is_our_entry(&fs::read(&target).unwrap()));
        assert!(remove_autostart_entry(&directory, uid).is_err());
        assert!(target.exists(), "an unrelated entry is never removed");
        assert!(write_autostart_entry(
            &directory,
            Path::new("/opt/qc-broker"),
            Path::new("/state/qc"),
            uid,
        )
        .is_err());
        assert_eq!(fs::read(&target).unwrap(), unrelated);
        fs::write(&target, first.as_bytes()).unwrap();
        write_autostart_entry(
            &directory,
            Path::new("/opt/qc-broker"),
            Path::new("/state/qc"),
            uid,
        )
        .unwrap();
        remove_autostart_entry(&directory, uid).unwrap();
        assert!(!target.exists(), "our marked entry is removed");
    }

    #[test]
    fn gio_exec_parser_preserves_unusual_executable_and_state_paths() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::process::CommandExt;

        let root = TestDirectory::new("qc-linux-desktop-exec-parser");
        let executable_directory = root.0.join("bin space $cash `tick` \"quote\"");
        fs::create_dir_all(&executable_directory).unwrap();
        let executable = executable_directory.join("qc\\broker");
        let capture = root.0.join("argv.bin");
        fs::write(
            &executable,
            b"#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$QC_AUTOSTART_ARGV_CAPTURE.tmp\" && mv \"$QC_AUTOSTART_ARGV_CAPTURE.tmp\" \"$QC_AUTOSTART_ARGV_CAPTURE\"\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

        let directory = root
            .0
            .join("state space $cash `tick` \"quote\" %f = ok\\dir");
        let desktop_path = root.0.join(AUTOSTART_FILE);
        fs::write(
            &desktop_path,
            desktop_entry(&executable, &directory).unwrap(),
        )
        .unwrap();

        let launcher = Command::new("gio")
            .arg("launch")
            .arg(&desktop_path)
            .env("QC_AUTOSTART_ARGV_CAPTURE", &capture)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("GIO is installed with the Linux desktop runtime");
        let launch_pid = launcher.id();
        let mut owned_group = GioLaunchGroup {
            launcher,
            pgid: launch_pid,
        };
        let launcher = &mut owned_group.launcher;
        let launch_deadline = Instant::now() + STARTUP_BOUND;
        let status = loop {
            if let Some(status) = launcher.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= launch_deadline {
                panic!("GIO did not finish launching the desktop entry within the bound");
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success(), "GIO rejected the generated desktop entry");

        let deadline = Instant::now() + STARTUP_BOUND;
        while !capture.is_file() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let actual = fs::read(&capture).expect("fixture executable wrote its argv");
        let mut expected = Vec::new();
        for argument in [
            "--resident",
            "--directory",
            directory.to_str().unwrap(),
            "--keep-user-quit",
        ] {
            expected.extend_from_slice(argument.as_bytes());
            expected.push(0);
        }
        assert_eq!(actual, expected, "GIO changed argument boundaries or bytes");
    }

    struct GioLaunchGroup {
        launcher: Child,
        pgid: u32,
    }

    impl Drop for GioLaunchGroup {
        fn drop(&mut self) {
            // GIO may return after spawning the target. The dedicated process
            // group keeps both launcher and target within this test's ownership.
            if let Ok(mut killer) = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", self.pgid)])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                let deadline = Instant::now() + Duration::from_secs(1);
                while Instant::now() < deadline {
                    if killer.try_wait().ok().flatten().is_some() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                if killer.try_wait().ok().flatten().is_none() {
                    let _ = killer.kill();
                    let _ = killer.wait();
                }
            }
            if self.launcher.try_wait().ok().flatten().is_none() {
                let _ = self.launcher.kill();
                let _ = self.launcher.wait();
            }
        }
    }

    #[test]
    fn autostart_ancestor_group_write_needs_the_private_group() {
        let root = TestDirectory::new("qc-linux-autostart-group");
        let uid = fs::metadata(&root.0).unwrap().uid();
        let gid = fs::metadata(&root.0).unwrap().gid();
        let check = |mode: u32, uid: u32, private_gid: Option<u32>| {
            fs::set_permissions(&root.0, fs::Permissions::from_mode(mode)).unwrap();
            let metadata = fs::symlink_metadata(&root.0).unwrap();
            validate_directory_component(&root.0, &metadata, uid, private_gid)
        };
        // Stock Ubuntu ~/.config: own directory, private group, 0775.
        check(0o775, uid, Some(gid)).unwrap();
        // Shared group, 0777 and a foreign owner stay refused.
        let error = check(0o775, uid, None).unwrap_err();
        assert!(error.contains("Unsafe XDG autostart directory component"));
        assert!(check(0o777, uid, Some(gid)).is_err());
        assert!(check(0o775, uid.wrapping_add(1), Some(gid)).is_err());

        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o775)).unwrap();
        let directory = root.0.join("autostart");
        ensure_autostart_directory(&directory, uid).unwrap();
    }
}
