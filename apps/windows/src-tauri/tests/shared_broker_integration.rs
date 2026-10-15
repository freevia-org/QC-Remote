//! Real two-process (in fact three-process) integration test for the shared
//! device broker (Task 5 of the shared-device-broker plan).
//!
//! The broker-convergence assertions inspect real OS processes with native
//! process identity APIs and by making real requests over the real wire protocol through
//! the app's own attach-or-elect logic, each run in its OWN process rather
//! than in-process - that is the entire point of this file: proving
//! convergence at the OS-process level, not merely inside one Rust process's
//! data structures. Cleanup-only fault tests additionally inject observation
//! errors; those synthetic seams do not substitute for the real-process cases.
//!
//! # Harness processes
//!
//! A `#[test]` function cannot take constructor arguments, and
//! `GatewayProcess`/`start_with_descriptor_path` are private to
//! `qc_remote_lib`'s own `lib.rs` - unreachable from this separate
//! integration-test crate. So each harness instance here is one invocation of
//! the crate's own compiled `--lib` unit-test binary, selecting the test
//! `shared_broker_integration_harness` by name (see the doc comment on that
//! test, at the bottom of `apps/windows/src-tauri/src/lib.rs`'s `mod tests`).
//! That test *is* the real app-side code path: it calls
//! `GatewayProcess::start_with_descriptor_path` exactly as a real window
//! would, and then answers file-based commands from this file so we can
//! drive request/response rounds and observe pids across the real process
//! boundary, without reimplementing the broker's wire protocol here.
//!
//! The harness binary is supplied by the test runner through
//! `QC_REMOTE_LIB_TEST_EXECUTABLE`; the runner captures its exact path from
//! Cargo's compiler-artifact output before launching tests.
//!
//! # Build preparation and the broker binary
//!
//! The parent builds both binaries once before spawning any harness. The exact
//! lib-test artifact comes from `QC_REMOTE_LIB_TEST_EXECUTABLE`; the broker is
//! supplied through `QC_GATEWAY_EXECUTABLE` or the active target directory.
//! Missing binaries fail with setup guidance; tests never invoke Cargo.
//! This file finds the brokers it started by the --descriptor-path on their
//! command line, which names this test's own fresh directory, so another
//! checkout's or the installed app's brokers are never counted.
//!
//! The broker publishes before its MIDI warm-up, which runs in the
//! background, so a machine whose Windows MIDI API (`midiOutGetNumDevs`)
//! never returns no longer keeps a real, unpatched broker from publishing.

use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

// The settings file the app's Settings window writes. Brokers started here
// draw no icon (QC_DEVICE_BROKER_NO_TRAY), which is exactly the state that
// must keep the shipped grace whatever the file says.
#[allow(dead_code)]
#[path = "../src/broker_tray_settings.rs"]
pub mod broker_tray_settings;

/// How long a freshly spawned harness may take to attach to or host the
/// shared broker and write its status file. Build preparation is completed
/// before spawning it, so this measures runtime startup only.
const HARNESS_READY_TIMEOUT: Duration = Duration::from_secs(30);
/// How long one request/response round through a harness's control directory
/// may take.
const HARNESS_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The broker's own zero-client grace period is 5s; this is that plus margin,
/// per the brief.
const BROKER_GRACE_PERIOD_MARGIN: Duration = Duration::from_secs(6);
const PROCESS_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

// Software diagnostic envelope, not a device/firmware limit. Select and read
// the tail from the same held file; growth after the seek cannot increase I/O
// or retained allocation. Concurrent growth may not be included in this sample.
const DIAGNOSTIC_TAIL_BYTES: usize = 16 * 1024;
fn read_diagnostic_tail(
    input: &mut (impl std::io::Read + std::io::Seek),
) -> std::io::Result<Vec<u8>> {
    use std::io::SeekFrom;
    let end = std::io::Seek::seek(input, SeekFrom::End(0))?;
    std::io::Seek::seek(
        input,
        SeekFrom::Start(end.saturating_sub(DIAGNOSTIC_TAIL_BYTES as u64)),
    )?;
    let mut bytes = Vec::new();
    let mut bounded = std::io::Read::take(input, DIAGNOSTIC_TAIL_BYTES as u64);
    std::io::Read::read_to_end(&mut bounded, &mut bytes)?;
    Ok(bytes)
}

// Reuse the production suspended/job-contained helper, including its bounded
// pipe pumps and descendant cleanup. Integration probes must not introduce an
// unbounded PATH-resolved PowerShell/taskkill process of their own.
#[cfg(windows)]
#[path = "../src/client_packages"]
mod client_packages {
    #[path = "owned_tool.rs"]
    mod owned_tool;
    pub(super) fn run(
        command: &mut std::process::Command,
        timeout: std::time::Duration,
    ) -> std::io::Result<std::process::Output> {
        owned_tool::run(command, timeout)
    }
}
#[cfg(windows)]
#[path = "../src/system_tools.rs"]
mod system_tools;

fn reap_child_within(child: &mut Child, timeout: Duration) -> std::io::Result<bool> {
    observe_exit_within(timeout, || child.try_wait().map(|status| status.is_some()))
}

fn observe_exit_within(
    timeout: Duration,
    mut exited: impl FnMut() -> std::io::Result<bool>,
) -> std::io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if exited()? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// Never silently drop the sole original Child after a failed termination.
// Failed owners remain attributed until the test executable exits; the root is
// retained too, and the test fails rather than claiming successful cleanup.
fn retain_failed_child(child: Child, root: &Path) {
    static RETAINED: OnceLock<Mutex<Vec<(Child, PathBuf)>>> = OnceLock::new();
    RETAINED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((child, root.to_path_buf()));
}

#[derive(Clone)]
struct TestRoot(Arc<TestRootOwner>);
struct TestRootOwner {
    path: PathBuf,
    brokers: Mutex<Vec<(u32, u64)>>,
    failed_child: std::sync::atomic::AtomicBool,
}
impl std::ops::Deref for TestRoot {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0.path
    }
}
impl AsRef<Path> for TestRoot {
    fn as_ref(&self) -> &Path {
        self
    }
}
impl TestRoot {
    fn record_broker(&self, pid: u32) {
        // Status names a PID, not its generation. Adopt only the publisher's
        // own descriptor identity, never a fresh PID lookup after readiness.
        let value: Value = serde_json::from_slice(
            &fs::read(self.join("device-broker.json")).expect("ready broker descriptor"),
        )
        .expect("ready descriptor JSON");
        assert_eq!(
            value["processId"].as_u64(),
            Some(u64::from(pid)),
            "ready descriptor names the same publisher"
        );
        let created = value["processCreatedAt"]
            .as_u64()
            .expect("publisher creation identity");
        let mut brokers = self.0.brokers.lock().unwrap_or_else(|e| e.into_inner());
        if !brokers.contains(&(pid, created)) {
            brokers.push((pid, created));
        }
    }
}

#[cfg(windows)]
fn retire_broker(pid: u32, created: u64) -> std::io::Result<()> {
    use windows_sys::Win32::{Foundation::*, System::Threading::*};
    // A retained HANDLE, not a second lookup by PID, is the termination target.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return if !process_is_alive(pid) {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        };
    }
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    let handle = Handle(handle);
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut start, mut end, mut kernel, mut user) = (zero, zero, zero, zero);
    if unsafe { GetProcessTimes(handle.0, &mut start, &mut end, &mut kernel, &mut user) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let actual = (u64::from(start.dwHighDateTime) << 32) | u64::from(start.dwLowDateTime);
    if actual != created {
        return Ok(());
    } // PID reused: never terminate replacement.
    if unsafe { WaitForSingleObject(handle.0, 0) } == WAIT_OBJECT_0 {
        return Ok(());
    }
    if unsafe { TerminateProcess(handle.0, 1) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { WaitForSingleObject(handle.0, PROCESS_CLEANUP_TIMEOUT.as_millis() as u32) }
        != WAIT_OBJECT_0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "owned broker did not terminate",
        ));
    }
    Ok(())
}
#[cfg(unix)]
fn retire_broker(pid: u32, created: u64) -> std::io::Result<()> {
    if !process_is_alive(pid) {
        return Ok(());
    }
    match qc_native_platform::process::os_process_created_at(pid) {
        Some(actual) if actual != created => return Ok(()),
        None => {
            return Err(std::io::Error::other(
                "owned broker creation identity is unavailable",
            ))
        }
        _ => {}
    }
    // Signal only the fresh private descriptor's recorded generation.
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) } != 0 && process_is_alive(pid) {
        return Err(std::io::Error::last_os_error());
    }
    if !wait_until(PROCESS_CLEANUP_TIMEOUT, || {
        !process_is_alive(pid)
            || qc_native_platform::process::os_process_created_at(pid)
                .is_some_and(|actual| actual != created)
    }) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "owned broker did not terminate",
        ));
    }
    Ok(())
}
impl Drop for TestRootOwner {
    fn drop(&mut self) {
        // Harnesses retain this Arc, so their original Children are retired
        // before the last root owner reaches this destructor.
        let cleanup = (|| -> Result<(), String> {
            let mut problems = Vec::new();
            if self.failed_child.load(std::sync::atomic::Ordering::Acquire) {
                problems.push("an original harness Child could not be reaped".to_owned());
            }
            // A harness can be killed before either readiness or descriptor
            // publication. Enumerate only this fresh exact descriptor namespace
            // before deleting bytes; an unavailable query refuses cleanup.
            let discovered = std::panic::catch_unwind(|| {
                discover_brokers(&self.path.join("device-broker.json"))
            });
            match discovered {
                Ok(discovered) => self
                    .brokers
                    .get_mut()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend(discovered),
                Err(_) => problems.push("owned broker process inspection failed".to_owned()),
            }
            // Startup can fail before status.json. The fresh private descriptor
            // still records the publisher's creation identity in that case.
            match fs::read(self.path.join("device-broker.json")) {
                Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                    Ok(value) => {
                        if let (Some(pid), Some(created)) = (
                            value["processId"]
                                .as_u64()
                                .and_then(|p| u32::try_from(p).ok()),
                            value["processCreatedAt"].as_u64(),
                        ) {
                            self.brokers
                                .get_mut()
                                .unwrap_or_else(|e| e.into_inner())
                                .push((pid, created));
                        } else {
                            problems
                                .push("descriptor publisher identity is unavailable".to_owned());
                        }
                    }
                    Err(error) => {
                        problems.push(format!("descriptor publisher JSON cannot be read: {error}"))
                    }
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    problems.push(format!("descriptor publisher cannot be read: {error}"))
                }
            }
            let brokers = self.brokers.get_mut().unwrap_or_else(|e| e.into_inner());
            if brokers.iter().any(|(pid, created)| {
                process_is_alive(*pid)
                    && !qc_native_platform::process::os_process_created_at(*pid)
                        .is_some_and(|actual| actual != *created)
            }) {
                wait_until(BROKER_GRACE_PERIOD_MARGIN, || {
                    brokers.iter().all(|(pid, created)| {
                        !process_is_alive(*pid)
                            || qc_native_platform::process::os_process_created_at(*pid)
                                .is_some_and(|actual| actual != *created)
                    })
                });
            }
            for &(pid, created) in brokers.iter() {
                if let Err(error) = retire_broker(pid, created) {
                    problems.push(format!("owned broker {pid} cleanup failed: {error}"));
                }
            }
            // A discovery fault never skips independently known owners. It
            // still forbids deleting bytes an unidentified owner may be using.
            if !problems.is_empty() {
                return Err(problems.join("; "));
            }
            match fs::remove_dir_all(&self.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        })();
        if let Err(error) = cleanup {
            let message = format!(
                "owned broker fixture cleanup failed; retained {}: {error}",
                self.path.display()
            );
            if std::thread::panicking() {
                eprintln!("{message}");
            } else {
                panic!("{message}");
            }
        }
    }
}

struct PreparedExecutables {
    harness: PathBuf,
    broker: PathBuf,
}

/// All parallel cases share this lookup; each child receives these exact
/// executables without changing the integration test process's environment.
fn prepared_executables() -> &'static PreparedExecutables {
    static PREPARED: OnceLock<Result<PreparedExecutables, String>> = OnceLock::new();
    PREPARED
        .get_or_init(|| {
            let harness = env::var_os("QC_REMOTE_LIB_TEST_EXECUTABLE")
                .map(PathBuf::from)
                .ok_or_else(|| {
                    "QC_REMOTE_LIB_TEST_EXECUTABLE is unset; build the qc_remote_lib test target and pass its exact executable path before running this integration test.".to_string()
                })?;
            if !harness.is_file() {
                return Err(format!(
                    "QC_REMOTE_LIB_TEST_EXECUTABLE does not name a file: {}",
                    harness.display()
                ));
            }
            let target = env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("..")
                        .join("..")
                        .join("..")
                        .join("target")
                });
            let broker = env::var_os("QC_GATEWAY_EXECUTABLE")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    target
                        .join("debug")
                        .join(format!("qc-device-broker{}", std::env::consts::EXE_SUFFIX))
                });
            if !broker.is_file() {
                return Err(format!(
                    "The prebuilt QC device broker is missing at {}; build it before running this integration test or set QC_GATEWAY_EXECUTABLE to its path.",
                    broker.display()
                ));
            }
            Ok(PreparedExecutables { harness, broker })
        })
        .as_ref()
        .unwrap_or_else(|error| panic!("shared broker test setup failed: {error}"))
}

fn locate_harness_binary() -> PathBuf {
    prepared_executables().harness.clone()
}

/// Ask the OS whether the process is alive, rather than trusting harness
/// bookkeeping. This uses the same fail-closed platform implementation as the
/// app and broker.
fn process_is_alive(pid: u32) -> bool {
    qc_native_platform::process::os_process_is_alive(pid)
}

/// The process ids of every `--broker-serve` broker the OS reports serving
/// this test's own descriptor path - the process-list-level assertion the
/// brief asks for ("exactly one qc-device-broker --broker-serve process").
///
/// Selected by command line, never by image name: the installed QC Remote
/// app's own broker and any concurrent test brokers run under the
/// same `qc-device-broker.exe` name, so a name count read "a stray broker"
/// on any machine where the app was open. Every broker this test starts is
/// given `--descriptor-path` under this test's own fresh directory, which no
/// other process on the machine can share.
#[cfg(windows)]
fn discover_brokers(descriptor_path: &Path) -> Vec<(u32, u64)> {
    // Capture each generation before reading its command line, and keep that
    // identity through the query and retirement. A replacement PID is refused.
    let native = qc_native_platform::process::processes_by_image("qc-device-broker.exe");
    let output = client_packages::run(Command::new(system_tools::powershell_exe())
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); ConvertTo-Json -Compress -InputObject @(Get-CimInstance Win32_Process -ErrorAction Stop | Where-Object { $_.CommandLine -like '*--broker-serve*' } | ForEach-Object { @{ pid = $_.ProcessId; commandLine = $_.CommandLine } })",
        ]).stderr(Stdio::piped()), PROCESS_CLEANUP_TIMEOUT)
        .expect("powershell runs");
    assert!(
        output.status.success(),
        "the process query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let values: Vec<Value> =
        serde_json::from_slice(&output.stdout).expect("process query JSON array");
    let mut matches = Vec::new();
    for value in values {
        if !windows_command_serves_descriptor(
            value["commandLine"].as_str().expect("process command line"),
            descriptor_path,
        ) {
            continue;
        }
        let pid =
            u32::try_from(value["pid"].as_u64().expect("process PID")).expect("process PID range");
        let Some(original) = native.iter().find(|process| process.pid == pid) else {
            assert!(!process_is_alive(pid), "publisher appeared during discovery; retain scratch rather than adopt a new generation");
            continue;
        };
        let created = original
            .created_at
            .expect("discovered publisher creation identity");
        assert!(
            original
                .command_line
                .as_deref()
                .is_some_and(|line| windows_command_serves_descriptor(line, descriptor_path)),
            "native discovered publisher command line must match the exact descriptor"
        );
        if !process_is_alive(pid) {
            continue;
        }
        let now = qc_native_platform::process::os_process_created_at(pid)
            .expect("discovered publisher identity remains readable");
        if now == created {
            matches.push((pid, created));
        }
    }
    matches
}

fn brokers_serving(descriptor_path: &Path) -> Vec<u32> {
    discover_brokers(descriptor_path)
        .into_iter()
        .map(|(pid, _)| pid)
        .collect()
}

#[cfg(windows)]
fn windows_command_serves_descriptor(command: &str, descriptor_path: &Path) -> bool {
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn CommandLineToArgvW(command: *const u16, count: *mut i32) -> *mut *mut u16;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }
    let command: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
    let mut count = 0;
    // SAFETY: command is NUL terminated; count is writable. The allocated
    // argument array remains alive until every argument is copied below.
    let arguments = unsafe { CommandLineToArgvW(command.as_ptr(), &mut count) };
    assert!(
        !arguments.is_null(),
        "process command line cannot be parsed"
    );
    struct Args(*mut *mut u16);
    impl Drop for Args {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0.cast());
            }
        }
    }
    let owned = Args(arguments);
    let mut copied = Vec::new();
    for index in 0..count as usize {
        let pointer = unsafe { *owned.0.add(index) };
        let mut len = 0;
        while unsafe { *pointer.add(len) } != 0 {
            len += 1;
        }
        copied.push(std::ffi::OsString::from_wide(unsafe {
            std::slice::from_raw_parts(pointer, len)
        }));
    }
    copied.iter().any(|argument| argument == "--broker-serve")
        && copied
            .windows(2)
            .any(|pair| pair[0] == "--descriptor-path" && Path::new(&pair[1]) == descriptor_path)
}

#[cfg(windows)]
#[test]
fn windows_broker_query_matches_exact_descriptor_argument_not_a_substring() {
    let path = Path::new(r"C:\owned fixture\device-broker.json");
    assert!(windows_command_serves_descriptor(
        r#"broker.exe --broker-serve --descriptor-path "C:\owned fixture\device-broker.json""#,
        path
    ));
    assert!(!windows_command_serves_descriptor(
        r#"broker.exe --broker-serve --descriptor-path "C:\owned fixture\device-broker.json.backup""#,
        path
    ));
    assert!(!windows_command_serves_descriptor(
        r#"broker.exe prefix--broker-serve --descriptor-path "C:\owned fixture\device-broker.json""#,
        path
    ));
    assert!(!windows_command_serves_descriptor(
        r#"broker.exe --broker-serve --other "C:\owned fixture\device-broker.json""#,
        path
    ));
}

#[cfg(unix)]
fn command_serves_descriptor(arguments: &[&[u8]], descriptor_path: &[u8]) -> bool {
    arguments
        .iter()
        .any(|argument| *argument == b"--broker-serve")
        && arguments
            .windows(2)
            .any(|pair| pair[0] == b"--descriptor-path" && pair[1] == descriptor_path)
}

#[cfg(unix)]
fn discover_brokers(descriptor_path: &Path) -> Vec<(u32, u64)> {
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};

    let own_uid = fs::metadata("/proc/self")
        .expect("read current process owner")
        .uid();
    let expected_path = descriptor_path.as_os_str().as_bytes();
    let entries = fs::read_dir("/proc").expect("read Linux process table");
    let mut pids = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let process_dir = entry.path();
        let Ok(process_metadata) = fs::metadata(&process_dir) else {
            continue;
        };
        if process_metadata.uid() != own_uid {
            continue;
        }
        let Some(identity) = qc_native_platform::process::os_process_created_at(pid) else {
            continue;
        };
        let Ok(executable) = fs::read_link(process_dir.join("exe")) else {
            continue;
        };
        if executable
            .file_name()
            .is_none_or(|name| name != "qc-device-broker")
        {
            continue;
        }
        let Ok(command_line) = fs::read(process_dir.join("cmdline")) else {
            continue;
        };
        let arguments = command_line
            .split(|byte| *byte == 0)
            .filter(|argument| !argument.is_empty())
            .collect::<Vec<_>>();
        if command_serves_descriptor(&arguments, expected_path)
            && qc_native_platform::process::os_process_is_alive(pid)
            && qc_native_platform::process::os_process_created_at(pid) == Some(identity)
        {
            pids.push((pid, identity));
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(unix)]
#[test]
fn broker_command_match_requires_exact_arguments_and_descriptor_path() {
    use std::os::unix::ffi::OsStrExt;

    let expected = Path::new("/tmp/qc-test/device-broker.json");
    let expected = expected.as_os_str().as_bytes();
    assert!(command_serves_descriptor(
        &[
            b"qc-device-broker",
            b"--broker-serve",
            b"--descriptor-path",
            expected
        ],
        expected
    ));
    assert!(!command_serves_descriptor(
        &[
            b"qc-device-broker",
            b"--broker-serve",
            b"--descriptor-path",
            b"/tmp/qc-test/device-broker.json.backup"
        ],
        expected
    ));
    assert!(!command_serves_descriptor(
        &[
            b"qc-device-broker",
            b"prefix--broker-serve",
            b"--descriptor-path",
            expected
        ],
        expected
    ));
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return condition();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Kills a process the hard way, from outside - the way a crash or the user
/// closing a window ends a real QC Remote process, as opposed to this file's
/// own `Child` handles being dropped (which the app-under-test must not rely
/// on: a real crash runs no destructors either).
#[cfg(windows)]
fn kill_pid_from_outside(pid: u32) {
    let output = client_packages::run(
        Command::new(system_tools::system32_directory().join("taskkill.exe"))
            .args(["/F", "/PID", &pid.to_string()])
            .stderr(Stdio::piped()),
        PROCESS_CLEANUP_TIMEOUT,
    )
    .expect("taskkill runs");
    assert!(
        output.status.success() || !process_is_alive(pid),
        "taskkill could not end pid {pid}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn kill_pid_from_outside(pid: u32) {
    use qc_native_platform::process::{os_process_created_at, os_process_is_alive};

    let identity = os_process_created_at(pid).expect("owned harness process has an identity");
    assert!(
        os_process_is_alive(pid),
        "owned harness pid {pid} is alive before kill"
    );
    assert_eq!(
        os_process_created_at(pid),
        Some(identity),
        "owned harness identity is stable before kill"
    );
    // No helper process or pipe wait is needed for a signal. The caller still
    // retains the original unreaped Child throughout this crash simulation.
    let sent = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) } == 0;
    assert!(
        sent || !os_process_is_alive(pid),
        "SIGKILL could not end owned harness pid {pid}: {}",
        std::io::Error::last_os_error()
    );
}

/// One harness process: a real, separate OS process running this crate's own
/// `shared_broker_integration_harness` test, attached to (or hosting) the
/// shared broker at `descriptor_path`.
struct Harness {
    child: Option<Child>,
    root: TestRoot,
    pid: u32,
    control_dir: PathBuf,
    status_path: PathBuf,
    request_counter: u64,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

/// What the harness wrote to its status file once
/// `start_with_descriptor_path` returned.
struct HarnessStatus {
    hosting: bool,
    broker_pid: u32,
}

impl Harness {
    fn spawn(harness_binary: &Path, root: &TestRoot, name: &str, descriptor_path: &Path) -> Self {
        Self::spawn_with_role(harness_binary, root, name, descriptor_path, false)
    }

    fn spawn_with_role(
        harness_binary: &Path,
        root: &TestRoot,
        name: &str,
        descriptor_path: &Path,
        observer: bool,
    ) -> Self {
        let work_dir = root.join(name);
        let control_dir = work_dir.join("control");
        fs::create_dir_all(&control_dir).expect("harness control directory");
        let status_path = work_dir.join("status.json");

        // Even callers that bypass locate_harness_binary finish preparation
        // before the child is spawned and any readiness clock can begin.
        let broker = &prepared_executables().broker;
        let stdout_path = work_dir.join("stdout.log");
        let stderr_path = work_dir.join("stderr.log");
        let stdout = fs::File::create(&stdout_path).expect("harness stdout log");
        let stderr = fs::File::create(&stderr_path).expect("harness stderr log");
        let mut command = Command::new(harness_binary);
        command
            .args([
                "tests::shared_broker_integration_harness",
                "--exact",
                "--nocapture",
            ])
            .env("QC_GATEWAY_EXECUTABLE", broker)
            // Inherited by the broker the harness spawns: no desktop icon
            // during tests, and so none of the icon's hold either.
            .env("QC_DEVICE_BROKER_NO_TRAY", "1")
            .env("QC_HARNESS_DESCRIPTOR_PATH", descriptor_path)
            .env("QC_HARNESS_STATUS_PATH", &status_path)
            .env("QC_HARNESS_CONTROL_DIR", &control_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        if observer {
            command.env("QC_HARNESS_OBSERVER", "1");
        } else {
            command.env_remove("QC_HARNESS_OBSERVER");
        }
        let child = command.spawn().expect("harness process starts");
        let pid = child.id();

        Self {
            child: Some(child),
            root: root.clone(),
            pid,
            control_dir,
            status_path,
            request_counter: 0,
            stdout_path,
            stderr_path,
        }
    }

    /// Blocks until the harness has written its status file (i.e. its own
    /// `start_with_descriptor_path` call returned), and reads it back.
    fn wait_ready(&mut self) -> HarnessStatus {
        self.wait_for_file(
            &self.status_path.clone(),
            HARNESS_READY_TIMEOUT,
            "readiness status",
        );
        let contents = fs::read_to_string(&self.status_path).expect("status file is readable");
        let value: Value = serde_json::from_str(&contents).expect("status file is valid JSON");
        let status = HarnessStatus {
            hosting: value["hosting"].as_bool().expect("hosting is a bool"),
            broker_pid: value["brokerPid"].as_u64().expect("brokerPid is a number") as u32,
        };
        self.root.record_broker(status.broker_pid);
        status
    }

    /// Drops a trigger file into this harness's control directory, waits for
    /// the matching result, and returns it ("ok" or an "err:..." message the
    /// harness formatted from a real `GatewayRequestFailure`).
    fn request(&mut self) -> String {
        self.request_bytes(b"")
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        let request = serde_json::json!({"method":method,"params":params}).to_string();
        let response = self.request_bytes(request.as_bytes());
        serde_json::from_str(&response).unwrap_or_else(|error| {
            panic!("harness {method} did not return a result: {response}: {error}")
        })
    }

    fn request_bytes(&mut self, contents: &[u8]) -> String {
        self.request_counter += 1;
        let trigger = self
            .control_dir
            .join(format!("req-{}.cmd", self.request_counter));
        let result_path = trigger.with_extension("result");
        let staging = trigger.with_extension("tmp");
        fs::write(&staging, contents).expect("trigger file is writable");
        fs::rename(staging, &trigger).expect("complete trigger is published");
        self.wait_for_file(&result_path, HARNESS_REQUEST_TIMEOUT, "request response");
        let result = fs::read_to_string(&result_path).expect("result file is readable");
        let _ = fs::remove_file(&result_path);
        result
    }

    fn wait_for_file(&mut self, path: &Path, timeout: Duration, stage: &str) {
        let mut exited = None;
        wait_until(timeout, || {
            if path.exists() {
                return true;
            }
            exited = self
                .child
                .as_mut()
                .expect("owned Child")
                .try_wait()
                .unwrap_or_else(|error| {
                    panic!(
                        "harness pid {} could not be queried: {error}\n{}",
                        self.pid,
                        self.diagnostics()
                    )
                });
            exited.is_some()
        });
        assert!(
            path.exists(),
            "harness pid {} failed waiting for {stage} at {} (exit: {exited:?})\n{}",
            self.pid,
            path.display(),
            self.diagnostics(),
        );
    }

    fn diagnostics(&self) -> String {
        let tail = |path: &Path| match fs::File::open(path)
            .and_then(|mut input| read_diagnostic_tail(&mut input))
        {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) => format!("<could not read {}: {error}>", path.display()),
        };
        format!(
            "stdout ({}):\n{}\nstderr ({}):\n{}",
            self.stdout_path.display(),
            tail(&self.stdout_path),
            self.stderr_path.display(),
            tail(&self.stderr_path),
        )
    }

    /// Kills this harness process from outside, the way a crashed or closed
    /// QC Remote window ends - not by dropping this `Child`, which this test
    /// deliberately avoids relying on.
    fn kill(mut self) {
        kill_pid_from_outside(self.pid);
        assert!(
            reap_child_within(
                self.child.as_mut().expect("owned Child"),
                PROCESS_CLEANUP_TIMEOUT
            )
            .expect("query original harness Child"),
            "original harness Child did not exit within cleanup deadline"
        );
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // A failed assertion must not leave a helper attached indefinitely.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            if !matches!(
                reap_child_within(&mut child, PROCESS_CLEANUP_TIMEOUT),
                Ok(true)
            ) {
                self.root
                    .0
                    .failed_child
                    .store(true, std::sync::atomic::Ordering::Release);
                retain_failed_child(child, &self.root);
                let message = format!(
                    "original harness Child {} could not be reaped; retained {}",
                    self.pid,
                    self.root.display()
                );
                if std::thread::panicking() {
                    eprintln!("{message}");
                } else {
                    panic!("{message}");
                }
            }
        }
    }
}

fn spike_root(name: &str) -> TestRoot {
    let root = env::temp_dir().join(format!(
        "qc-broker-tray-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    create_private_test_directory(&root).expect("private spike scratch directory");
    TestRoot(Arc::new(TestRootOwner {
        path: root,
        brokers: Mutex::new(Vec::new()),
        failed_child: std::sync::atomic::AtomicBool::new(false),
    }))
}

fn create_private_test_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path)
    }
}

#[cfg(unix)]
#[test]
fn broker_test_roots_are_private_for_the_linux_directory_guard() {
    use std::os::unix::fs::PermissionsExt;

    let root = spike_root("owner-mode");
    let result = fs::metadata(&root)
        .map(|metadata| metadata.permissions().mode() & 0o777)
        .expect("test root metadata");
    fs::remove_dir(&root).expect("remove owned empty test root");
    assert_eq!(result, 0o700, "broker descriptor roots must be private");
}

#[test]
fn integration_scratch_is_removed_on_assertion_unwind() {
    let mut path = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let root = spike_root("unwind-root");
        path = Some(root.to_path_buf());
        fs::write(root.join("owned-log"), b"temporary diagnostics").unwrap();
        panic!("injected primary assertion");
    }));
    assert_eq!(
        result.unwrap_err().downcast_ref::<&str>(),
        Some(&"injected primary assertion")
    );
    assert!(
        !path.unwrap().exists(),
        "unwind removes the attributed scratch tree"
    );
}

#[test]
fn diagnostic_tail_reads_only_its_held_envelope_even_when_an_open_log_grows() {
    use std::io::{Read, Seek, SeekFrom, Write};
    struct Meter {
        file: fs::File,
        read: usize,
        grow_after_end: bool,
    }
    impl Read for Meter {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let count = self.file.read(output)?;
            self.read += count;
            Ok(count)
        }
    }
    impl Seek for Meter {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            let offset = self.file.seek(position)?;
            if matches!(position, SeekFrom::End(0)) && self.grow_after_end {
                self.grow_after_end = false;
                self.file.set_len(offset + 1024 * 1024)?;
            }
            Ok(offset)
        }
    }
    let root = spike_root("diagnostic-held-tail");
    let path = root.join("owned.log");
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    // Sparse local fixture: no harness/broker/device session is started. Two MiB is
    // sufficient to expose the old whole-file read through an actual meter.
    file.set_len(2 * 1024 * 1024).unwrap();
    let sentinel = b"owned diagnostic tail";
    file.seek(SeekFrom::End(-(sentinel.len() as i64))).unwrap();
    file.write_all(sentinel).unwrap();
    let mut input = Meter {
        file,
        read: 0,
        grow_after_end: false,
    };
    let bytes = read_diagnostic_tail(&mut input).unwrap();
    assert_eq!(bytes.len(), 16 * 1024);
    assert!(bytes.ends_with(sentinel));
    assert_eq!(
        input.read,
        16 * 1024,
        "a bounded displayed tail must not hide an unbounded underlying read"
    );
    input.read = 0;
    input.grow_after_end = true;
    let bytes = read_diagnostic_tail(&mut input).unwrap();
    assert_eq!(bytes.len(), 16 * 1024);
    assert!(
        bytes.ends_with(sentinel),
        "sample remains bound to the held end before concurrent growth"
    );
    assert_eq!(
        input.read,
        16 * 1024,
        "growth after seek cannot expand the retained envelope"
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), 3 * 1024 * 1024);
    drop(input);
}

#[test]
fn integration_root_waits_for_its_last_harness_owner() {
    let root = spike_root("root-owner-order");
    let path = root.to_path_buf();
    let last_owner = root.clone();
    drop(root);
    assert!(
        path.is_dir(),
        "a harness owner retains its live scratch bytes"
    );
    drop(last_owner);
    assert!(!path.exists());
}

#[test]
fn unacknowledged_harness_cleanup_refuses_root_removal_and_reports_recovery_path() {
    let root = spike_root("retained-cleanup-failure");
    let path = root.to_path_buf();
    fs::write(root.join("owned-log"), b"recovery diagnostics").unwrap();
    // Inject only the cleanup acknowledgement seam; no process is launched.
    root.0
        .failed_child
        .store(true, std::sync::atomic::Ordering::Release);
    let failure =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(root))).unwrap_err();
    let message = failure
        .downcast_ref::<String>()
        .expect("named cleanup failure");
    let kept = fs::read(path.join("owned-log"));
    fs::remove_dir_all(&path).unwrap(); // test owns the injected, process-free bytes
    assert!(
        message.contains("original harness Child could not be reaped"),
        "{message}"
    );
    assert!(
        message.contains(path.to_string_lossy().as_ref()),
        "{message}"
    );
    assert_eq!(kept.unwrap(), b"recovery diagnostics");
}

#[test]
fn original_child_poll_is_bounded_and_wait_errors_do_not_prove_exit() {
    let started = Instant::now();
    let mut calls = 0;
    assert!(!observe_exit_within(Duration::from_millis(20), || {
        calls += 1;
        Ok(false)
    })
    .unwrap());
    assert!(calls >= 2, "a running child is polled until the bound");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "a stalled child cannot hang cleanup"
    );
    let error = observe_exit_within(Duration::from_secs(5), || {
        Err(std::io::Error::other("injected observation error"))
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "injected observation error");
    assert!(
        observe_exit_within(Duration::ZERO, || Ok(true)).unwrap(),
        "an acknowledged exit succeeds even at the deadline"
    );
}

#[test]
fn harness_assertion_unwind_reaps_original_child_before_removing_scratch() {
    let mut path = None;
    let mut pid = None;
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let root = spike_root("harness-unwind");
        path = Some(root.to_path_buf());
        #[cfg(windows)]
        let mut command = {
            let mut c = Command::new(system_tools::system32_directory().join("ping.exe"));
            c.args(["-n", "60", "127.0.0.1"]);
            c
        };
        #[cfg(unix)]
        let mut command = {
            let mut c = Command::new("/bin/sleep");
            c.arg("60");
            c
        };
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let child_pid = child.id();
        let harness = Harness {
            child: Some(child),
            root: root.clone(),
            pid: child_pid,
            control_dir: root.to_path_buf(),
            status_path: root.join("never-ready.json"),
            request_counter: 0,
            stdout_path: root.join("stdout.log"),
            stderr_path: root.join("stderr.log"),
        };
        pid = Some(child_pid);
        fs::write(root.join("owned-log"), b"temporary").unwrap();
        assert!(harness.child.is_some(), "original Child remains owned");
        panic!("injected harness assertion");
    }));
    assert_eq!(
        failure.unwrap_err().downcast_ref::<&str>(),
        Some(&"injected harness assertion")
    );
    assert!(
        !process_is_alive(pid.unwrap()),
        "the original harness process was ended before root removal"
    );
    assert!(!path.unwrap().exists());
}

#[test]
fn broker_assertion_unwind_retires_owned_descendant_before_removing_scratch() {
    let binary = locate_harness_binary();
    let mut path = None;
    let mut broker = None;
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let root = spike_root("broker-unwind");
        path = Some(root.to_path_buf());
        let mut harness = Harness::spawn(
            &binary,
            &root,
            "participant",
            &root.join("device-broker.json"),
        );
        broker = Some(harness.wait_ready().broker_pid);
        assert_eq!(harness.request(), "ok");
        panic!("injected broker assertion");
    }));
    assert_eq!(
        failure.unwrap_err().downcast_ref::<&str>(),
        Some(&"injected broker assertion")
    );
    assert!(
        !process_is_alive(broker.unwrap()),
        "the owned descendant is gone before scratch is removed"
    );
    assert!(!path.unwrap().exists());
}

#[test]
fn malformed_publisher_diagnostics_do_not_skip_a_previously_owned_broker_cleanup() {
    let binary = locate_harness_binary();
    let mut path = None;
    let mut broker = None;
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let root = spike_root("malformed-publisher-unwind");
        path = Some(root.to_path_buf());
        let descriptor = root.join("device-broker.json");
        let mut harness = Harness::spawn(&binary, &root, "participant", &descriptor);
        broker = Some(harness.wait_ready().broker_pid);
        fs::write(&descriptor, b"injected unreadable publisher JSON").unwrap();
        fs::write(root.join("retained-log"), b"recovery diagnostics").unwrap();
        panic!("injected primary after publisher corruption");
    }));
    let path = path.expect("fixture acquired");
    let gone = !process_is_alive(broker.expect("owned generation recorded"));
    let retained = fs::read(path.join("retained-log"));
    // The injection deliberately forbids automatic root removal; this test can
    // remove its own bytes only after proving the independent owner is gone.
    if gone {
        fs::remove_dir_all(&path).unwrap();
    }
    assert_eq!(
        failure.unwrap_err().downcast_ref::<&str>(),
        Some(&"injected primary after publisher corruption")
    );
    assert!(
        gone,
        "a malformed descriptor cannot bypass known broker cleanup"
    );
    assert_eq!(
        retained.unwrap(),
        b"recovery diagnostics",
        "discovery faults retain attributed recovery bytes"
    );
}

#[test]
fn an_early_harness_exit_reports_its_captured_diagnostics() {
    let root = spike_root("diagnostic-failure");
    let stdout_path = root.join("stdout.log");
    let stderr_path = root.join("stderr.log");
    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args([
            "/D",
            "/C",
            "echo harness diagnostic sentinel 1>&2 & exit /B 7",
        ]);
        command
    } else {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo 'harness diagnostic sentinel' >&2; exit 7"]);
        command
    };
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(fs::File::create(&stdout_path).unwrap()))
        .stderr(Stdio::from(fs::File::create(&stderr_path).unwrap()))
        .spawn()
        .unwrap();
    let mut harness = Harness {
        pid: child.id(),
        child: Some(child),
        root: root.clone(),
        control_dir: root.to_path_buf(),
        status_path: root.join("never-ready.json"),
        request_counter: 0,
        stdout_path,
        stderr_path,
    };
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| harness.wait_ready()))
        .err()
        .expect("an exited child cannot satisfy readiness");
    let message = failure.downcast_ref::<String>().unwrap();
    assert!(message.contains("readiness status"), "{message}");
    assert!(message.contains("exit: Some"), "{message}");
    assert!(message.contains("harness diagnostic sentinel"), "{message}");
    drop(harness);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_broker_without_its_icon_keeps_the_shipped_grace_whatever_the_idle_setting() {
    // The one sanctioned broker without an icon: here no desktop icon may be
    // drawn, so a 24-hour or Never setting must not hold the device at all.
    for minutes in [Some(1440), None] {
        let harness_binary = locate_harness_binary();
        let root = spike_root("no-icon-no-hold");
        let descriptor = root.join("device-broker.json");
        broker_tray_settings::set_tray_enabled_in(&root, true).unwrap();
        broker_tray_settings::set_idle_minutes_in(&root, minutes).unwrap();
        let mut participant = Harness::spawn(&harness_binary, &root, "participant", &descriptor);
        let status = participant.wait_ready();
        participant.kill();
        assert!(
            wait_until(BROKER_GRACE_PERIOD_MARGIN, || !process_is_alive(
                status.broker_pid
            )),
            "with no icon the five-second grace reaps the broker ({minutes:?})"
        );
        assert!(!descriptor.exists());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn the_crash_path_reaps_the_broker_when_the_only_connection_is_killed_outright() {
    let harness_binary = locate_harness_binary();
    let root = spike_root("observer-crash");
    let descriptor = root.join("device-broker.json");
    let mut observer =
        Harness::spawn_with_role(&harness_binary, &root, "observer", &descriptor, true);
    let status = observer.wait_ready();
    assert_eq!(
        observer.rpc("broker.clientCount", serde_json::json!({}))["clients"],
        0
    );
    assert_eq!(
        observer.rpc("broker.clients", serde_json::json!({}))["clients"][0]["role"],
        "observer"
    );
    observer.kill();
    assert!(
        wait_until(BROKER_GRACE_PERIOD_MARGIN, || !process_is_alive(
            status.broker_pid
        )),
        "OS socket close must reap the broker after the observer process is killed"
    );
    assert!(!descriptor.exists(), "reaping releases the descriptor slot");
    fs::remove_dir_all(root).expect("clean spike directory");
}

#[test]
fn spike_step_3_an_observer_preserves_raw_liveness_until_it_detaches() {
    let harness_binary = locate_harness_binary();
    let root = spike_root("observer-liveness");
    let descriptor = root.join("device-broker.json");
    let mut participant = Harness::spawn(&harness_binary, &root, "participant", &descriptor);
    let status = participant.wait_ready();
    let mut observer =
        Harness::spawn_with_role(&harness_binary, &root, "observer", &descriptor, true);
    assert_eq!(observer.wait_ready().broker_pid, status.broker_pid);
    assert_eq!(
        observer.rpc("broker.clientCount", serde_json::json!({}))["clients"],
        1
    );
    participant.kill();
    assert!(wait_until(Duration::from_secs(2), || {
        observer.rpc("broker.clientCount", serde_json::json!({}))["clients"] == 0
    }));
    std::thread::sleep(BROKER_GRACE_PERIOD_MARGIN);
    assert_eq!(
        observer.request(),
        "ok",
        "observer keeps raw liveness past the grace"
    );
    assert!(process_is_alive(status.broker_pid));
    observer.kill();
    assert!(wait_until(
        BROKER_GRACE_PERIOD_MARGIN,
        || !process_is_alive(status.broker_pid)
    ));
    assert!(!descriptor.exists());
    fs::remove_dir_all(root).expect("clean spike directory");
}

#[test]
fn no_observer_ever_preserves_the_pre_change_count_guard_and_reap_behavior() {
    let harness_binary = locate_harness_binary();
    let root = spike_root("legacy-participants");
    let descriptor = root.join("device-broker.json");
    let mut first = Harness::spawn(&harness_binary, &root, "first", &descriptor);
    let status = first.wait_ready();
    let mut second = Harness::spawn(&harness_binary, &root, "second", &descriptor);
    assert_eq!(second.wait_ready().broker_pid, status.broker_pid);
    assert_eq!(
        first.rpc("broker.clientCount", serde_json::json!({}))["clients"],
        2
    );
    let refused = first.rpc("device.disconnect", serde_json::json!({}));
    assert!(
        refused["detail"].as_str().unwrap().contains("left open"),
        "{refused}"
    );
    second.kill();
    assert!(wait_until(Duration::from_secs(2), || {
        first.rpc("broker.clientCount", serde_json::json!({}))["clients"] == 1
    }));
    let disconnected = first.rpc("device.disconnect", serde_json::json!({}));
    assert!(
        !disconnected["detail"]
            .as_str()
            .unwrap()
            .contains("left open"),
        "{disconnected}"
    );
    first.kill();
    assert!(wait_until(
        BROKER_GRACE_PERIOD_MARGIN,
        || !process_is_alive(status.broker_pid)
    ));
    assert!(!descriptor.exists());
    fs::remove_dir_all(root).expect("clean spike directory");
}

#[test]
fn two_real_processes_converge_on_one_shared_broker_and_survive_host_handoff() {
    let harness_binary = locate_harness_binary();

    let root = spike_root("host-handoff");
    // Next to `device-broker.json`, exactly the pairing
    // `device_broker_election_path` (`descriptor_path.with_extension
    // ("election")`) produces in the app itself.
    let descriptor_path = root.join("device-broker.json");

    assert_eq!(
        brokers_serving(&descriptor_path),
        Vec::<u32>::new(),
        "a broker serving this test's own fresh descriptor path cannot exist yet"
    );

    // --- Step 1 & 2: two harnesses started against the same fresh descriptor
    // path converge on exactly one broker process. ---
    let mut first = Harness::spawn(&harness_binary, &root, "first", &descriptor_path);
    let first_status = first.wait_ready();
    assert!(
        first_status.hosting,
        "the only contender so far must be the one hosting the broker"
    );
    let broker_pid = first_status.broker_pid;

    let mut second = Harness::spawn(&harness_binary, &root, "second", &descriptor_path);
    let second_status = second.wait_ready();
    assert!(
        !second_status.hosting,
        "the second process must attach to the first's broker, not spawn its own"
    );
    assert_eq!(
        second_status.broker_pid, broker_pid,
        "both harnesses must agree on the same broker process"
    );

    assert_eq!(
        brokers_serving(&descriptor_path),
        vec![broker_pid],
        "exactly one broker - the one both harnesses named - must serve this descriptor \
         once both harnesses have started"
    );

    // --- Step 3: both can independently issue a request and get a valid
    // response, against the same broker. ---
    assert_eq!(first.request(), "ok");
    assert_eq!(second.request(), "ok");

    // --- Step 4: kill the harness that is NOT hosting the broker; the broker
    // survives, and the remaining (hosting) harness still works. ---
    second.kill();
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        process_is_alive(broker_pid),
        "the broker must survive an attached-only client's process being killed"
    );
    assert_eq!(
        first.request(),
        "ok",
        "the hosting harness must still be served after the other harness was killed"
    );

    // --- Step 5: kill the harness that IS hosting the broker; the broker
    // exits within its grace period, and a third fresh harness against the
    // same descriptor path becomes the new host rather than seeing a stale
    // live descriptor. ---
    first.kill();
    assert!(
        wait_until(BROKER_GRACE_PERIOD_MARGIN, || !process_is_alive(broker_pid)),
        "with every client's process gone, the broker must exit through its own grace period \
         within {BROKER_GRACE_PERIOD_MARGIN:?}"
    );

    let mut third = Harness::spawn(&harness_binary, &root, "third", &descriptor_path);
    let third_status = third.wait_ready();
    assert!(
        third_status.hosting,
        "with the old broker gone, a fresh harness against the same descriptor path must \
         become the new host, not see a stale live descriptor"
    );
    assert_ne!(
        third_status.broker_pid, broker_pid,
        "the new host's broker must be a genuinely new process, not the old (now dead) pid"
    );
    assert_eq!(third.request(), "ok");

    third.kill();
    assert!(
        wait_until(BROKER_GRACE_PERIOD_MARGIN, || !process_is_alive(
            third_status.broker_pid
        )),
        "the final harness's broker must exit before the test releases its staged executable"
    );
    assert!(
        !descriptor_path.exists(),
        "reaping releases the descriptor slot"
    );
    fs::remove_dir_all(&root).expect("clean integration test directory");
}
