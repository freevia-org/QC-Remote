//! macOS resident broker launch and per-user LaunchAgent registration.
//! The broker's owner-only resident marker remains the authority for status.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const LABEL: &str = "org.freevia.qcremote.resident";
const OWNERSHIP: &str = "QC Remote managed LaunchAgent v1";
const STARTUP_BOUND: Duration = Duration::from_secs(3);
const REAPER_POLL: Duration = Duration::from_millis(100);
static CHILD_REAPER: OnceLock<Result<Arc<Mutex<Vec<Child>>>, String>> = OnceLock::new();

pub(super) fn quit(directory: &Path) -> Result<(), String> {
    let executable = locate()?;
    let output = Command::new(&executable)
        .args(["--control", "quit", "--confirm", "--directory"])
        .arg(directory)
        .output()
        .map_err(|error| format!("Ask the QC Remote broker to quit: {error}"))?;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "The QC Remote broker returned an unreadable quit result ({error}); stderr: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })?;
    match report["exitCode"].as_i64() {
        Some(0 | 3) => Ok(()),
        Some(code) => Err(report["detail"].as_str().map_or_else(
            || format!("The QC Remote broker refused to quit (exit code {code})"),
            |detail| format!("The QC Remote broker refused to quit: {detail}"),
        )),
        None => Err("The QC Remote broker returned no quit status".into()),
    }
}

pub(super) fn is_running(directory: &Path) -> bool {
    qc_native_platform::resident_marker::is_running(directory)
}

pub(super) fn locate() -> Result<PathBuf, String> {
    let app = std::env::current_exe()
        .map_err(|error| format!("Locate the running QC Remote application: {error}"))?;
    let directory = app
        .parent()
        .ok_or("The QC Remote executable has no parent directory")?;
    let target = format!("{}-apple-darwin", std::env::consts::ARCH);
    [
        directory.join(format!("qc-device-broker-{target}")),
        directory.join("qc-device-broker"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .ok_or_else(|| "The qc-device-broker macOS sidecar is missing beside QC Remote".to_owned())
}

pub(super) fn validate_run_entry_location(executable: &Path) -> Result<(), String> {
    validate_installed_app_location(executable, &std::env::var_os("HOME").map(PathBuf::from))
}

/// Login jobs must point at a bundle that remains available after a disk image
/// is ejected or a downloaded app is moved. Only direct children of the two
/// conventional Applications folders are supported; resolve firmlinks and
/// symlinks before comparing so `/Applications` works on modern macOS too.
fn validate_installed_app_location(
    executable: &Path,
    home: &Option<PathBuf>,
) -> Result<(), String> {
    let bundle = executable
        .parent()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new("MacOS"))
        })
        .and_then(Path::parent)
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new("Contents"))
        })
        .and_then(Path::parent)
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == std::ffi::OsStr::new("app"))
        })
        .and_then(|path| path.canonicalize().ok());
    let supported_roots = std::iter::once(PathBuf::from("/Applications"))
        .chain(home.iter().map(|home| home.join("Applications")))
        .filter_map(|path| path.canonicalize().ok());
    if bundle.as_ref().is_some_and(|bundle| {
        supported_roots
            .into_iter()
            .any(|root| bundle.parent() == Some(root.as_path()))
    }) {
        return Ok(());
    }
    Err("QC Remote cannot enable its background login item from this location. Move QC Remote.app directly into /Applications or ~/Applications, launch it there, then turn this setting on again.".into())
}

pub(super) fn spawn(
    executable: &Path,
    directory: &Path,
    supersede_user_quit: bool,
) -> Result<(), String> {
    if let Some(helper) = menu_app(executable) {
        Command::new("/usr/bin/open")
            .arg("-g")
            .arg(helper)
            .arg("--args")
            .arg("--keep-user-quit")
            .arg("--directory")
            .arg(directory)
            .status()
            .map_err(|error| format!("Open the standalone broker menu: {error}"))?;
    }
    let reaper = reaper_children()?;
    let mut command = Command::new(executable);
    command
        .arg("--resident")
        .arg("--directory")
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(crate::broker_stderr_sink());
    if !supersede_user_quit {
        command.arg("--keep-user-quit");
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Start the QC Remote background host: {error}"))?;
    let child_identity = qc_native_platform::process::os_process_created_at(child.id());
    let deadline = Instant::now() + STARTUP_BOUND;
    let mut clean_exit = None;
    loop {
        if is_running(directory) {
            send_to_reaper(reaper, child);
            return Ok(());
        }
        if clean_exit.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    clean_exit = Some(status);
                }
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
        }
        if Instant::now() >= deadline {
            if is_running(directory) {
                send_to_reaper(reaper, child);
                return Ok(());
            }
            let published = super::serving_broker_published(directory);
            return stop_unpublished_child(
                child,
                child_identity,
                directory,
                reaper,
                if published {
                    "The broker serves, but did not become a resident background host within 3 seconds".into()
                } else {
                    "The QC Remote background host did not become resident within 3 seconds".into()
                },
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
    if is_running(directory) || super::serving_broker_published(directory) {
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
    send_to_reaper(reaper, child);
    Err(error)
}

/// Transfers ownership to a process-wide waiter so short-lived GUI launchers
/// do not leave zombies and long-lived helpers remain accounted for.
pub(crate) fn spawn_owned(mut command: Command) -> Result<(), String> {
    let reaper = reaper_children()?;
    let child = command
        .spawn()
        .map_err(|error| format!("Could not start desktop helper: {error}"))?;
    send_to_reaper(reaper, child);
    Ok(())
}

fn send_to_reaper(reaper: &Arc<Mutex<Vec<Child>>>, child: Child) {
    reaper
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(child);
}

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
                    let _ = children.swap_remove(index).wait();
                }
                Ok(None) | Err(_) => index += 1,
            }
        }
    }
}

#[cfg(feature = "macos-app-store")]
pub(super) fn set_run_entry(executable: &Path, _directory: &Path) -> Result<(), String> {
    validate_run_entry_location(executable)?;
    qc_native_platform::app_store::set_login_enabled(true)
}

#[cfg(not(feature = "macos-app-store"))]
pub(super) fn set_run_entry(executable: &Path, directory: &Path) -> Result<(), String> {
    // Keep sidecar lookup and foreground use available from any valid bundle;
    // only the persistent RunAtLoad entry requires a stable install location.
    validate_run_entry_location(executable)?;
    let path = launch_agent_path()?;
    ensure_launch_agent_directory(path.parent().expect("LaunchAgent has parent"))?;
    let contents = launch_agent(executable, directory)?;
    let existing_contents = match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
            {
                return Err(format!(
                    "Refusing to replace an unrelated LaunchAgent: {}",
                    path.display()
                ));
            }
            let existing = fs::read_to_string(&path)
                .map_err(|error| format!("Read existing QC Remote LaunchAgent: {error}"))?;
            if !existing.contains(OWNERSHIP) {
                return Err(format!(
                    "Refusing to replace an unrelated LaunchAgent: {}",
                    path.display()
                ));
            }
            Some(existing)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("Inspect QC Remote LaunchAgent: {error}")),
    };
    qc_owner_only_file::publish_atomically(&path, contents.as_bytes())
        .map_err(|error| format!("Write QC Remote LaunchAgent: {error}"))?;

    let domain = gui_domain()?;
    let loaded = Command::new("/bin/launchctl")
        .args(["print", &format!("{domain}/{LABEL}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    let changed = existing_contents.as_deref() != Some(contents.as_str());
    let resident_running = is_running(directory) || super::serving_broker_published(directory);
    register_launch_agent(
        &path,
        &domain,
        loaded,
        changed,
        resident_running,
        |arguments| {
            let status = Command::new("/bin/launchctl")
                .args(arguments)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|error| error.to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("launchctl status {status}"))
            }
        },
        || spawn(executable, directory, false),
    )
}

/// Registers the atomically published LaunchAgent plist. If the live broker
/// depends on the currently loaded definition, refresh is deferred until the
/// next GUI login so bootout cannot stop that broker. The injected runner keeps
/// reload ordering and failures testable without invoking launchctl.
fn register_launch_agent(
    path: &Path,
    domain: &str,
    loaded: bool,
    changed: bool,
    resident_running: bool,
    mut launchctl: impl FnMut(&[std::ffi::OsString]) -> Result<(), String>,
    recover_resident: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let mut run = |arguments: Vec<std::ffi::OsString>, action: &str| {
        launchctl(&arguments).map_err(|error| format!("{action} QC Remote at login: {error}"))
    };
    let mut booted_out = false;
    if loaded {
        if !changed {
            return Ok(());
        }
        // `bootout` terminates a running launchd-owned job. Keep the resident
        // broker alive. The on-disk plist is already updated atomically, and
        // launchd reads it again when the next GUI login domain is created.
        if resident_running {
            return Ok(());
        }
        run(
            vec![
                std::ffi::OsString::from("bootout"),
                std::ffi::OsString::from(format!("{domain}/{LABEL}")),
            ],
            "Refresh",
        )?;
        booted_out = true;
    }
    let result = run(
        [
            std::ffi::OsString::from("bootstrap"),
            std::ffi::OsString::from(domain),
            path.as_os_str().to_owned(),
        ]
        .into_iter()
        .collect(),
        "Register",
    );
    if let Err(error) = result {
        if booted_out {
            return match recover_resident() {
                Ok(()) => Err(error),
                Err(recovery_error) => Err(format!(
                    "{error}; the previous resident broker was stopped and recovery failed: {recovery_error}"
                )),
            };
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(feature = "macos-app-store")]
pub(super) fn remove_run_entry() -> Result<(), String> {
    qc_native_platform::app_store::set_login_enabled(false)
}

#[cfg(not(feature = "macos-app-store"))]
pub(super) fn remove_run_entry() -> Result<(), String> {
    let path = launch_agent_path()?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Inspect QC Remote LaunchAgent: {error}")),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(format!(
            "Refusing to remove an unrelated LaunchAgent: {}",
            path.display()
        ));
    }
    let contents = fs::read_to_string(&path)
        .map_err(|error| format!("Read QC Remote LaunchAgent: {error}"))?;
    if !contents.contains(OWNERSHIP) {
        return Err(format!(
            "Refusing to remove an unrelated LaunchAgent: {}",
            path.display()
        ));
    }
    // Removing the file prevents registration at the next login. Do not
    // `bootout` the current job: that would terminate a resident broker the
    // settings contract explicitly leaves running until its idle grace ends.
    fs::remove_file(&path).map_err(|error| format!("Remove QC Remote LaunchAgent: {error}"))
}

fn launch_agent_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("No absolute HOME is available for the macOS LaunchAgent")?;
    Ok(home
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

fn ensure_launch_agent_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| format!("Create LaunchAgents directory: {error}"))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Inspect LaunchAgents directory: {error}"))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
    {
        return Err(format!("Unsafe LaunchAgents directory: {}", path.display()));
    }
    Ok(())
}

fn gui_domain() -> Result<String, String> {
    let uid = unsafe { libc::geteuid() };
    if uid == 0 {
        return Err("QC Remote cannot register a LaunchAgent as root".into());
    }
    Ok(format!("gui/{uid}"))
}

fn launch_agent(executable: &Path, directory: &Path) -> Result<String, String> {
    let helper = menu_app(executable).map(|app| app.join("Contents/MacOS/QCDeviceBroker"));
    let executable = helper.as_deref().unwrap_or(executable);
    let executable = executable
        .to_str()
        .ok_or("Broker executable path is not valid Unicode")?;
    let directory = directory
        .to_str()
        .ok_or("Broker directory path is not valid Unicode")?;
    let mut arguments = vec![
        executable,
        "--resident",
        "--directory",
        directory,
        "--keep-user-quit",
    ];
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<!-- {OWNERSHIP} -->\n<plist version=\"1.0\"><dict><key>Label</key><string>{LABEL}</string><key>LimitLoadToSessionType</key><string>Aqua</string><key>RunAtLoad</key><true/><key>ProgramArguments</key><array>"
    );
    for argument in arguments.drain(..) {
        xml.push_str("<string>");
        xml.push_str(&xml_escape(argument));
        xml.push_str("</string>");
    }
    xml.push_str("</array></dict></plist>\n");
    Ok(xml)
}

fn menu_app(executable: &Path) -> Option<PathBuf> {
    #[cfg(feature = "macos-app-store")]
    let location = "Helpers/QC Device Broker.app";
    #[cfg(not(feature = "macos-app-store"))]
    let location = "Resources/QC Device Broker.app";
    let app = executable.parent()?.parent()?.join(location);
    app.join("Contents/MacOS/QCDeviceBroker")
        .is_file()
        .then_some(app)
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::{
        launch_agent, register_launch_agent, validate_installed_app_location, LABEL, OWNERSHIP,
    };
    use plist::Value;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "qc-macos-location-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn app_executable(bundle: &Path) -> PathBuf {
        bundle.join("Contents/MacOS/QC Remote")
    }

    #[test]
    fn login_item_accepts_only_a_direct_applications_bundle() {
        let scratch = Scratch::new();
        let home = scratch.0.join("home");
        let installed = home.join("Applications/QC Remote.app");
        std::fs::create_dir_all(installed.join("Contents/MacOS")).unwrap();

        validate_installed_app_location(&app_executable(&installed), &Some(home.clone()))
            .expect("the user's Applications folder is persistent");

        let staged = home.join("Applications/Downloaded/QC Remote.app");
        std::fs::create_dir_all(staged.join("Contents/MacOS")).unwrap();
        let error = validate_installed_app_location(&app_executable(&staged), &Some(home))
            .expect_err("nested staging folders are not stable install locations");
        assert!(error.contains("Move QC Remote.app directly into /Applications or ~/Applications"));
    }

    #[test]
    fn login_item_rejects_downloads_and_mounted_disk_images() {
        let scratch = Scratch::new();
        let home = scratch.0.join("home");
        let downloads = home.join("Downloads/QC Remote.app");
        let disk_image = scratch.0.join("Volumes/QC Remote/QC Remote.app");
        for app in [&downloads, &disk_image] {
            std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
            assert!(
                validate_installed_app_location(&app_executable(app), &Some(home.clone())).is_err()
            );
        }
    }

    fn rendered_calls(calls: &[Vec<OsString>]) -> Vec<Vec<String>> {
        calls
            .iter()
            .map(|arguments| {
                arguments
                    .iter()
                    .map(|argument| argument.to_string_lossy().into_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_changed_loaded_launch_agent_is_booted_out_before_the_new_plist_is_bootstrapped() {
        let mut calls = Vec::new();
        register_launch_agent(
            Path::new("/Users/example/Library/LaunchAgents/org.freevia.qcremote.resident.plist"),
            "gui/501",
            true,
            true,
            false,
            |arguments| {
                calls.push(arguments.to_vec());
                Ok(())
            },
            || Ok(()),
        )
        .expect("managed LaunchAgent refreshes");

        assert_eq!(
            rendered_calls(&calls),
            vec![
                vec![
                    "bootout".to_owned(),
                    "gui/501/org.freevia.qcremote.resident".to_owned()
                ],
                vec![
                    "bootstrap".to_owned(),
                    "gui/501".to_owned(),
                    "/Users/example/Library/LaunchAgents/org.freevia.qcremote.resident.plist"
                        .to_owned()
                ]
            ]
        );
    }

    #[test]
    fn unchanged_loaded_launch_agent_is_left_registered() {
        let mut calls = Vec::new();
        register_launch_agent(
            Path::new("/tmp/agent.plist"),
            "gui/501",
            true,
            false,
            true,
            |arguments| {
                calls.push(arguments.to_vec());
                Ok(())
            },
            || Ok(()),
        )
        .expect("unchanged LaunchAgent remains loaded");

        assert!(calls.is_empty());
    }

    #[test]
    fn changed_loaded_launch_agent_is_not_booted_out_while_its_resident_broker_is_running() {
        let mut calls = Vec::new();
        register_launch_agent(
            Path::new("/tmp/agent.plist"),
            "gui/501",
            true,
            true,
            true,
            |arguments| {
                calls.push(arguments.to_vec());
                Ok(())
            },
            || panic!("recovery must not run when a running resident is preserved"),
        )
        .expect("the live resident broker is preserved");

        assert!(calls.is_empty());
    }

    #[test]
    fn failed_launch_agent_bootout_does_not_bootstrap_a_second_registration() {
        let mut calls = Vec::new();
        let result = register_launch_agent(
            Path::new("/tmp/agent.plist"),
            "gui/501",
            true,
            true,
            false,
            |arguments| {
                calls.push(arguments.to_vec());
                Err("launchctl status 1".into())
            },
            || panic!("recovery must not run when bootout fails"),
        );

        assert!(result.is_err());
        assert_eq!(
            rendered_calls(&calls),
            vec![vec![
                "bootout".to_owned(),
                "gui/501/org.freevia.qcremote.resident".to_owned()
            ]]
        );
    }

    #[test]
    fn failed_bootstrap_after_bootout_restarts_the_resident_broker() {
        let mut calls = Vec::new();
        let mut recovered = false;
        let result = register_launch_agent(
            Path::new("/tmp/agent.plist"),
            "gui/501",
            true,
            true,
            false,
            |arguments| {
                calls.push(arguments.to_vec());
                if calls.len() == 1 {
                    Ok(())
                } else {
                    Err("launchctl status 5".into())
                }
            },
            || {
                recovered = true;
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(recovered);
        assert_eq!(
            rendered_calls(&calls),
            vec![
                vec![
                    "bootout".to_owned(),
                    "gui/501/org.freevia.qcremote.resident".to_owned()
                ],
                vec![
                    "bootstrap".to_owned(),
                    "gui/501".to_owned(),
                    "/tmp/agent.plist".to_owned()
                ]
            ]
        );
    }

    #[test]
    fn launch_agent_is_valid_and_keeps_the_broker_arguments_as_distinct_values() {
        let executable = Path::new("/Applications/QC & Remote.app/Contents/MacOS/qc-device-broker");
        let directory = Path::new("/Users/example/Library/Application Support/QC <Remote>");
        let xml = launch_agent(executable, directory).expect("valid macOS paths");
        let value = Value::from_reader(std::io::Cursor::new(xml.as_bytes()))
            .expect("valid LaunchAgent plist");
        let dictionary = value.as_dictionary().expect("LaunchAgent dictionary");

        assert_eq!(
            dictionary.get("Label").and_then(Value::as_string),
            Some(LABEL)
        );
        assert_eq!(
            dictionary
                .get("LimitLoadToSessionType")
                .and_then(Value::as_string),
            Some("Aqua")
        );
        assert_eq!(
            dictionary.get("RunAtLoad").and_then(Value::as_boolean),
            Some(true)
        );
        let arguments = dictionary
            .get("ProgramArguments")
            .and_then(Value::as_array)
            .expect("LaunchAgent argument array");
        let arguments: Vec<_> = arguments
            .iter()
            .map(|argument| argument.as_string().expect("string argument"))
            .collect();
        assert_eq!(
            arguments,
            [
                executable.to_str().unwrap(),
                "--resident",
                "--directory",
                directory.to_str().unwrap(),
                "--keep-user-quit",
            ]
        );
        assert!(xml.contains(OWNERSHIP));
    }

    #[test]
    fn launch_agent_refuses_non_unicode_paths() {
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;

            let executable = Path::new(OsStr::from_bytes(b"/tmp/qc-\xff"));
            assert!(launch_agent(executable, Path::new("/tmp/qc-remote")).is_err());
        }
    }
}
