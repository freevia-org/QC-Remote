//! Native Linux subprocess coverage. Every broker starts disconnected: none of
//! these tests enumerates or opens hardware, including on a machine with a QC.
#![cfg(target_os = "linux")]

use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(10);
const IO_WAIT: Duration = Duration::from_secs(3);
const APP_DIRECTORY: &str = "org.freevia.qcremote";

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("qc-linux-broker-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path.join("state"))
            .unwrap();
        Self(path)
    }

    fn directory(&self) -> PathBuf {
        self.0.join("state").join(APP_DIRECTORY)
    }

    fn descriptor(&self) -> PathBuf {
        self.directory().join("device-broker.json")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_qc-device-broker"));
        command
            .env("XDG_RUNTIME_DIR", &self.0)
            .env("XDG_STATE_HOME", self.0.join("state"))
            .env("HOME", self.0.join("home"))
            .env("QC_DEVICE_BROKER_NO_TRAY", "1")
            .stdin(Stdio::null());
        command
    }

    fn spawn(&self) -> Process {
        Process::spawn(
            self.command()
                .arg("--broker-serve")
                .arg("--descriptor-path")
                .arg(self.descriptor())
                .arg("--no-tray"),
            &self.0,
        )
    }

    fn published(&self, process: &mut Process) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Ok(bytes) = fs::read(self.descriptor()) {
                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                    if value["processId"] == process.child.id() {
                        return value;
                    }
                }
            }
            assert!(
                process.child.try_wait().unwrap().is_none(),
                "broker exited before publication: {}",
                process.diagnostics()
            );
            assert!(
                Instant::now() < deadline,
                "descriptor publication timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_released(&self) {
        assert!(
            !self.descriptor().exists(),
            "descriptor remained after shutdown"
        );
        assert!(
            !qc_owner_only_file::claim_path(&self.descriptor()).exists(),
            "descriptor claim remained after shutdown"
        );
        assert!(
            fs::read_dir(self.directory()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            }),
            "staging file remained after shutdown"
        );
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Own the exact spawned child, never a process found by name. File-backed
/// output cannot fill a pipe; both successful and panicking tests reap children
/// before Scratch removes their private files.
struct Process {
    child: Child,
    stdout: PathBuf,
    stderr: PathBuf,
}

impl Process {
    fn spawn(command: &mut Command, root: &Path) -> Self {
        let label = uuid::Uuid::new_v4();
        let stdout = root.join(format!("{label}.stdout"));
        let stderr = root.join(format!("{label}.stderr"));
        let child = command
            .stdout(File::create(&stdout).unwrap())
            .stderr(File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        Self {
            child,
            stdout,
            stderr,
        }
    }

    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "owned process did not exit: {}",
                self.diagnostics()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn diagnostics(&self) -> String {
        fs::read_to_string(&self.stderr).unwrap_or_default()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn connect(descriptor: &Value) -> TcpStream {
    let port = u16::try_from(descriptor["port"].as_u64().unwrap()).unwrap();
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(IO_WAIT)).unwrap();
    stream.set_write_timeout(Some(IO_WAIT)).unwrap();
    stream
}

fn send(stream: &mut TcpStream, request: &Value) {
    let body = serde_json::to_vec(request).unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

fn receive(stream: &mut TcpStream) -> Value {
    let mut header = [0; 4];
    stream.read_exact(&mut header).unwrap();
    let size = u32::from_be_bytes(header) as usize;
    assert!(
        size > 0 && size <= 1024 * 1024,
        "invalid test response length"
    );
    let mut body = vec![0; size];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn exchange(stream: &mut TcpStream, request: Value) -> Value {
    send(stream, &request);
    loop {
        let reply = receive(stream);
        if reply.get("id") == request.get("id") {
            return reply;
        }
    }
}

fn authenticate(descriptor: &Value) -> TcpStream {
    let mut stream = connect(descriptor);
    let reply = exchange(
        &mut stream,
        json!({
            "method": "broker.authenticate", "params": {"token": descriptor["bearerToken"]}, "id": 0
        }),
    );
    assert_eq!(reply["result"]["instanceId"], descriptor["instanceId"]);
    stream
}

fn assert_closed(stream: &mut TcpStream) {
    let mut byte = [0];
    match stream.read(&mut byte) {
        Ok(0) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        result => panic!("unauthenticated connection was not closed: {result:?}"),
    }
}

#[test]
fn two_processes_share_one_election_and_the_last_client_releases_it() {
    let scratch = Scratch::new();
    let mut owner = scratch.spawn();
    let descriptor = scratch.published(&mut owner);
    let mut first = authenticate(&descriptor);
    let mut second = authenticate(&descriptor);
    let mut contender = scratch.spawn();
    assert!(contender.wait().success(), "{}", contender.diagnostics());
    let still_published: Value =
        serde_json::from_slice(&fs::read(scratch.descriptor()).unwrap()).unwrap();
    assert_eq!(still_published["instanceId"], descriptor["instanceId"]);
    assert!(owner.child.try_wait().unwrap().is_none());
    for stream in [&mut first, &mut second] {
        let reply = exchange(
            stream,
            json!({"jsonrpc":"2.0", "id":1, "method":"system.status", "params":{}}),
        );
        assert!(reply.get("error").is_none(), "{reply}");
        assert!(reply["result"].is_object());
    }
    drop(first);
    let reply = exchange(
        &mut second,
        json!({"jsonrpc":"2.0", "id":2, "method":"broker.clientCount", "params":{}}),
    );
    assert!(reply.get("error").is_none(), "{reply}");
    drop(second);
    assert!(owner.wait().success(), "{}", owner.diagnostics());
    scratch.assert_released();
}

#[test]
fn simultaneous_launches_elect_exactly_one_owner() {
    let scratch = Scratch::new();
    let mut first = scratch.spawn();
    let mut second = scratch.spawn();
    let deadline = Instant::now() + WAIT;
    let descriptor = loop {
        if let Ok(bytes) = fs::read(scratch.descriptor()) {
            if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                if value["processId"] == first.child.id() || value["processId"] == second.child.id()
                {
                    break value;
                }
            }
        }
        assert!(Instant::now() < deadline, "neither contender published");
        thread::sleep(Duration::from_millis(10));
    };
    let participant = authenticate(&descriptor);
    let (owner, loser) = if descriptor["processId"] == first.child.id() {
        (&mut first, &mut second)
    } else {
        (&mut second, &mut first)
    };
    assert!(loser.wait().success(), "{}", loser.diagnostics());
    assert!(owner.child.try_wait().unwrap().is_none());
    let current: Value = serde_json::from_slice(&fs::read(scratch.descriptor()).unwrap()).unwrap();
    assert_eq!(current["instanceId"], descriptor["instanceId"]);
    drop(participant);
    assert!(owner.wait().success());
    scratch.assert_released();
}

#[test]
fn unauthenticated_malformed_and_oversized_frames_never_enter_rpc() {
    let scratch = Scratch::new();
    let mut owner = scratch.spawn();
    let descriptor = scratch.published(&mut owner);
    let _participant = authenticate(&descriptor);
    for request in [
        json!({"method":"broker.authenticate", "params":{"token":"wrong"}, "id":0}),
        json!({"jsonrpc":"2.0", "method":"system.status", "params":{}, "id":1}),
    ] {
        let mut stream = connect(&descriptor);
        let reply = exchange(&mut stream, request);
        assert_eq!(reply["error"]["code"], -32001);
        assert_closed(&mut stream);
    }
    for body in [b"{".as_slice(), b"null".as_slice()] {
        let mut stream = connect(&descriptor);
        stream
            .write_all(&(body.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(body).unwrap();
        assert_eq!(receive(&mut stream)["error"]["code"], -32001);
        assert_closed(&mut stream);
    }
    let mut stream = connect(&descriptor);
    stream.write_all(&4097_u32.to_be_bytes()).unwrap();
    assert_eq!(receive(&mut stream)["error"]["code"], -32001);
    assert_closed(&mut stream);
    assert!(owner.child.try_wait().unwrap().is_none());
}

#[test]
fn descriptors_are_owner_only_and_sigterm_cleans_the_claim_without_recording_quit() {
    let scratch = Scratch::new();
    let mut owner = scratch.spawn();
    let descriptor = scratch.published(&mut owner);
    let mut participant = authenticate(&descriptor);
    let metadata = fs::symlink_metadata(scratch.descriptor()).unwrap();
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.uid(), fs::metadata(&scratch.0).unwrap().uid());
    assert_eq!(
        fs::metadata(scratch.directory()).unwrap().mode() & 0o777,
        0o700
    );
    assert!(!metadata.file_type().is_symlink());
    // SAFETY: pid came from this still-owned Child; it cannot be reused until
    // we reap it. The signal handler is the production shutdown path.
    assert_eq!(
        unsafe { libc::kill(owner.child.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let notice = receive(&mut participant);
    assert_eq!(notice["method"], "broker.shuttingDown");
    assert!(owner.wait().success(), "{}", owner.diagnostics());
    scratch.assert_released();
    assert!(!scratch
        .descriptor()
        .with_extension("user-quit.json")
        .exists());
}

#[test]
fn a_killed_owner_is_replaced_without_waiting_for_descriptor_age() {
    let scratch = Scratch::new();
    let mut owner = scratch.spawn();
    let descriptor = scratch.published(&mut owner);
    let _participant = authenticate(&descriptor);
    owner.child.kill().unwrap();
    assert!(!owner.wait().success());
    assert!(
        scratch.descriptor().exists(),
        "SIGKILL bypasses normal cleanup"
    );
    let mut replacement = scratch.spawn();
    let new_descriptor = scratch.published(&mut replacement);
    assert_ne!(new_descriptor["instanceId"], descriptor["instanceId"]);
    assert_ne!(new_descriptor["processId"], descriptor["processId"]);
    let participant = authenticate(&new_descriptor);
    drop(participant);
    assert!(replacement.wait().success());
    scratch.assert_released();
}

#[test]
fn control_quit_authenticates_and_releases_its_exact_broker() {
    let scratch = Scratch::new();
    let mut owner = scratch.spawn();
    let descriptor = scratch.published(&mut owner);
    let _participant = authenticate(&descriptor);
    let mut control = Process::spawn(
        scratch.command().args(["--control", "quit", "--confirm"]),
        &scratch.0,
    );
    assert!(control.wait().success(), "{}", control.diagnostics());
    let output = fs::read(&control.stdout).unwrap();
    let report: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["schema"], "qc-broker-control/v1");
    assert!(
        !String::from_utf8_lossy(&output).contains(descriptor["bearerToken"].as_str().unwrap()),
        "control report exposed its credential"
    );
    assert!(owner.wait().success());
    scratch.assert_released();
}

#[test]
fn a_world_readable_descriptor_is_refused_by_the_control_client() {
    let scratch = Scratch::new();
    let mut owner = scratch.spawn();
    let descriptor = scratch.published(&mut owner);
    let _participant = authenticate(&descriptor);
    // The live broker refreshes its own document atomically. Test a separate
    // descriptor snapshot so a refresh cannot repair the permissions during
    // the refusal check. Were this accepted, its token reaches the live owner.
    let untrusted = Scratch::new();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(untrusted.directory())
        .unwrap();
    fs::write(
        untrusted.descriptor(),
        serde_json::to_vec(&descriptor).unwrap(),
    )
    .unwrap();
    fs::set_permissions(untrusted.descriptor(), fs::Permissions::from_mode(0o644)).unwrap();
    let mut control = Process::spawn(
        untrusted.command().args(["--control", "quit", "--confirm"]),
        &untrusted.0,
    );
    assert_eq!(control.wait().code(), Some(2));
    assert!(owner.child.try_wait().unwrap().is_none());
}

#[test]
fn the_harness_reaps_owned_children_and_removes_staging_after_a_panic() {
    let scratch = Scratch::new();
    let root = scratch.0.clone();
    let child = std::sync::Mutex::new(None);
    let outcome = std::panic::catch_unwind(|| {
        let mut owner = scratch.spawn();
        let descriptor = scratch.published(&mut owner);
        let _participant = authenticate(&descriptor);
        *child.lock().unwrap() = Some(owner.child.id());
        panic!("exercise failure-path cleanup");
    });
    assert!(outcome.is_err());
    let pid = child.lock().unwrap().unwrap();
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "owned child survived unwind"
    );
    drop(scratch);
    assert!(!root.exists(), "private test directory survived cleanup");
}
