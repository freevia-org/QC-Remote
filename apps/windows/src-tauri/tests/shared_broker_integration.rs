//! Real two-process (in fact three-process) integration test for the shared
//! device broker (Task 5 of the shared-device-broker plan).
//!
//! Every assertion here is made by inspecting real OS processes with
//! `tasklist` and by making real requests over the real wire protocol through
//! the app's own attach-or-elect logic, each run in its OWN process rather
//! than in-process - that is the entire point of this file: proving
//! convergence at the OS-process level, not merely inside one Rust process's
//! data structures. Nothing here mocks a process boundary.
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
use std::sync::OnceLock;
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

/// The `tasklist`-based liveness check this repo's own `lib.rs` unit tests
/// already use (`process_is_alive` there) - asking the OS, not our own
/// bookkeeping, whether a process still exists. Duplicated here rather than
/// imported: it is a private test helper of a different crate.
fn process_is_alive(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .expect("tasklist runs");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
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
fn brokers_serving(descriptor_path: &Path) -> Vec<u32> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*--broker-serve*' } \
             | ForEach-Object { '{0} {1}' -f $_.ProcessId, $_.CommandLine }",
        ])
        .output()
        .expect("powershell runs");
    assert!(
        output.status.success(),
        "the process query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let descriptor_path = descriptor_path.to_string_lossy();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains(descriptor_path.as_ref()))
        .filter_map(|line| line.split_whitespace().next()?.parse().ok())
        .collect()
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
fn kill_pid_from_outside(pid: u32) {
    let output = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output()
        .expect("taskkill runs");
    assert!(
        output.status.success() || !process_is_alive(pid),
        "taskkill could not end pid {pid}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// One harness process: a real, separate OS process running this crate's own
/// `shared_broker_integration_harness` test, attached to (or hosting) the
/// shared broker at `descriptor_path`.
struct Harness {
    child: Child,
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
    fn spawn(harness_binary: &Path, root: &Path, name: &str, descriptor_path: &Path) -> Self {
        Self::spawn_with_role(harness_binary, root, name, descriptor_path, false)
    }

    fn spawn_with_role(
        harness_binary: &Path,
        root: &Path,
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
            child,
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
        HarnessStatus {
            hosting: value["hosting"].as_bool().expect("hosting is a bool"),
            broker_pid: value["brokerPid"].as_u64().expect("brokerPid is a number") as u32,
        }
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
            exited = self.child.try_wait().unwrap_or_else(|error| {
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
        let tail = |path: &Path| match fs::read(path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(16 * 1024)..])
                .into_owned(),
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
        let _ = self.child.wait();
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // A failed assertion must not leave a helper attached indefinitely.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spike_root(name: &str) -> PathBuf {
    let root = env::temp_dir().join(format!(
        "qc-broker-tray-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("spike scratch directory");
    root
}

#[test]
fn an_early_harness_exit_reports_its_captured_diagnostics() {
    let root = spike_root("diagnostic-failure");
    let stdout_path = root.join("stdout.log");
    let stderr_path = root.join("stderr.log");
    let child = Command::new("cmd")
        .args([
            "/D",
            "/C",
            "echo harness diagnostic sentinel 1>&2 & exit /B 7",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(fs::File::create(&stdout_path).unwrap()))
        .stderr(Stdio::from(fs::File::create(&stderr_path).unwrap()))
        .spawn()
        .unwrap();
    let mut harness = Harness {
        pid: child.id(),
        child,
        control_dir: root.clone(),
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

    let root = env::temp_dir().join(format!(
        "qc-shared-broker-integration-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("test root directory");
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
