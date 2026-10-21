#![cfg(test)]

//! `--control` against real in-process TCP brokers on scratch descriptor
//! directories. No test starts a process, reads the real profile, opens the
//! USB device or names a real pid other than its own: the machine is the
//! `TestEnv` below, and "the broker process exited" is the flag a served
//! broker sets where the shipped one calls `process::exit`.
use super::*;
use crate::rpc::test_support::{self, Served};
use crate::rpc::BrokerHandle;
use crate::tray_settings::IdlePolicy;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct Scratch(PathBuf, Arc<ScratchRoot>);
struct ScratchRoot(PathBuf);
impl Drop for ScratchRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Scratch {
    fn new(label: &str) -> Self {
        let directory = (0..64)
            .find_map(|_| {
                let path = std::env::temp_dir()
                    .canonicalize()
                    .expect("canonical fixture parent")
                    .join(format!(
                        "qc-broker-control-{label}-{:032x}",
                        rand::random::<u128>()
                    ));
                match fs::create_dir(&path) {
                    Ok(()) => Some(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => None,
                    Err(error) => panic!("reserve control fixture root: {error}"),
                }
            })
            .expect("exclusive control fixture root after at most 64 collisions");
        let owner = Arc::new(ScratchRoot(directory.clone()));
        crate::device_lease::testing::create_directory(&directory).unwrap();
        Self(directory, owner)
    }
    fn descriptor(&self) -> PathBuf {
        self.0.join(DESCRIPTOR_FILE)
    }
}

type RetainedOldBroker = (
    std::thread::JoinHandle<Vec<String>>,
    Arc<ScratchRoot>,
    Arc<OldBrokerPermit>,
);
static RETAINED_OLD_BROKERS: Mutex<Vec<RetainedOldBroker>> = Mutex::new(Vec::new());
static OLD_BROKER_OWNERS: AtomicUsize = AtomicUsize::new(0);
struct OldBrokerPermit;
impl OldBrokerPermit {
    #[allow(
        deprecated,
        reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
    )]
    fn acquire() -> Arc<Self> {
        let finished = {
            let mut retained = RETAINED_OLD_BROKERS
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let mut finished = Vec::new();
            let mut index = 0;
            while index < retained.len() {
                if crate::platform::process::thread_signalled(&retained[index].0) {
                    finished.push(retained.swap_remove(index));
                } else {
                    index += 1;
                }
            }
            finished
        };
        // Never join or release ownership under the registry mutex.
        for (worker, root, permit) in finished {
            let _ = worker.join();
            drop(root);
            drop(permit);
        }
        OLD_BROKER_OWNERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 4).then_some(count + 1)
            })
            .expect("old broker fixture capacity exhausted; exact prior owners remain retained");
        Arc::new(Self)
    }
}
impl Drop for OldBrokerPermit {
    fn drop(&mut self) {
        OLD_BROKER_OWNERS.fetch_sub(1, Ordering::AcqRel);
    }
}
struct OldBroker {
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<Vec<String>>>,
    root: Arc<ScratchRoot>,
    permit: Option<Arc<OldBrokerPermit>>,
}
impl OldBroker {
    fn start(listener: std::net::TcpListener, root: Arc<ScratchRoot>) -> Self {
        // Admit before any worker spawn; setup errors/panics release this.
        let permit = OldBrokerPermit::acquire();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requested = stop.clone();
        let owned_root = root.clone();
        let worker = std::thread::spawn(move || {
            let _root = owned_root;
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                if requested.load(Ordering::Acquire) || Instant::now() >= deadline {
                    return Vec::new();
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        // Windows may inherit the listener's nonblocking mode.
                        // Framing deadlines operate on blocking accepted sockets.
                        stream.set_nonblocking(false).unwrap();
                        break stream;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("accept old broker fixture: {error}"),
                }
            };
            let mut frames = Vec::new();
            while !requested.load(Ordering::Acquire) {
                let body = match qc_broker_framing::read_tcp_frame_before_deadline(
                    &mut stream,
                    MAX_FRAME_BYTES,
                    deadline,
                ) {
                    Ok(body) => body,
                    Err(_) => break,
                };
                let request: Value = serde_json::from_slice(&body).unwrap();
                if frames.is_empty() {
                    let reply =
                        serde_json::to_vec(&json!({"id":0,"result":{"instanceId":"old"}})).unwrap();
                    qc_broker_framing::write_tcp_frame_before_deadline(
                        &mut stream,
                        &reply,
                        MAX_FRAME_BYTES,
                        deadline,
                    )
                    .unwrap();
                }
                frames.push(request["method"].as_str().unwrap_or_default().to_owned());
            }
            frames
        });
        Self {
            stop,
            worker: Some(worker),
            root,
            permit: Some(permit),
        }
    }
    fn finish(mut self) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(6);
        while !crate::platform::process::thread_signalled(self.worker.as_ref().unwrap()) {
            assert!(
                Instant::now() < deadline,
                "old broker fixture finishes within its deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let frames = self.worker.take().unwrap().join().unwrap();
        self.permit.take();
        frames
    }
}
impl Drop for OldBroker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let Some(worker) = self.worker.take() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(6);
        while !crate::platform::process::thread_signalled(&worker) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if crate::platform::process::thread_signalled(&worker) {
            let _ = worker.join();
        } else {
            RETAINED_OLD_BROKERS
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push((
                    worker,
                    self.root.clone(),
                    self.permit
                        .take()
                        .expect("original worker owns its admission"),
                ));
            eprintln!("old broker fixture cleanup unacknowledged; worker and scratch retained");
        }
    }
}

#[cfg(target_os = "macos")]
thread_local! {
    static MAC_NATIVE_DIRECTORY: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(target_os = "macos")]
struct MacNativeDirectoryScope;
#[cfg(target_os = "macos")]
impl Drop for MacNativeDirectoryScope {
    fn drop(&mut self) {
        MAC_NATIVE_DIRECTORY.with(|directory| *directory.borrow_mut() = None);
    }
}

#[cfg(target_os = "macos")]
fn mac_native_directory() -> PathBuf {
    MAC_NATIVE_DIRECTORY.with(|directory| {
        directory
            .borrow()
            .as_ref()
            .expect("owned Mac fixture installed")
            .clone()
    })
}

#[cfg(target_os = "macos")]
#[test]
fn native_mac_marker_fixture_retires_on_assertion_unwind() {
    let mut original = None;
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let scratch = Scratch::new("macos-native-unwind");
        original = Some(scratch.0.clone());
        MAC_NATIVE_DIRECTORY.with(|held| *held.borrow_mut() = Some(scratch.0.clone()));
        let _scope = MacNativeDirectoryScope;
        let _marker = qc_native_platform::resident_marker::ResidentMarker::acquire(&scratch.0)
            .expect("acquire owned marker");
        panic!("injected marker fixture assertion");
    }));
    assert!(failed.is_err());
    assert!(!original.expect("exclusive root acquired").exists());
    MAC_NATIVE_DIRECTORY.with(|held| assert!(held.borrow().is_none()));
}

#[cfg(target_os = "macos")]
#[test]
fn native_control_observes_the_mac_resident_marker() {
    let scratch = Scratch::new("macos-native");
    let directory = scratch.0.clone();
    MAC_NATIVE_DIRECTORY.with(|held| *held.borrow_mut() = Some(directory.clone()));
    let _scope = MacNativeDirectoryScope;
    let environment = NativeEnv {
        default_directory: Some(mac_native_directory),
    };
    assert!(!environment.session_marker_present());
    assert_eq!(
        environment.own_directory().as_deref(),
        Some(directory.as_path())
    );
    let marker = qc_native_platform::resident_marker::ResidentMarker::acquire(&directory)
        .expect("acquire macOS resident marker");
    assert!(environment.session_marker_present());
    drop(marker);
    assert!(!environment.session_marker_present());
}

type Spawner = Box<dyn Fn(&Path) -> Result<Box<dyn Spawned>, String> + Send + Sync>;

#[test]
fn control_descriptor_reads_refuse_oversized_owner_private_documents_without_changing_them() {
    let scratch = Scratch::new("oversized-descriptor");
    qc_owner_only_file::write_staged(&scratch.descriptor(), b"{}").unwrap();
    let file = fs::OpenOptions::new()
        .write(true)
        .open(scratch.descriptor())
        .unwrap();
    file.set_len(128 * 1024 + 1).unwrap();
    drop(file);
    let error = read_descriptor(&scratch.descriptor()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(
        fs::metadata(scratch.descriptor()).unwrap().len(),
        128 * 1024 + 1
    );
}

#[test]
fn control_descriptor_reads_accept_a_verified_document_at_the_exact_budget() {
    let scratch = Scratch::new("exact-descriptor-budget");
    let mut bytes = b"{}".to_vec();
    bytes.resize(128 * 1024, b' ');
    qc_owner_only_file::write_staged(&scratch.descriptor(), &bytes).unwrap();
    assert_eq!(read_descriptor(&scratch.descriptor()).unwrap(), bytes);
}

struct TestEnv {
    directory: PathBuf,
    /// Brokers whose "process" ended: their pid is ours, so liveness is the
    /// served broker's exit flag, matched by creation time.
    exited: Mutex<Vec<Arc<AtomicBool>>>,
    marker: AtomicBool,
    interactive: bool,
    spawner: Option<Spawner>,
    spawns: AtomicUsize,
    owner_override: Option<qc_native_platform::owner_identity::Assessment>,
}

impl TestEnv {
    fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
            exited: Mutex::new(Vec::new()),
            marker: AtomicBool::new(false),
            interactive: true,
            spawner: None,
            spawns: AtomicUsize::new(0),
            owner_override: None,
        }
    }
    fn watching(self, served: &Served) -> Self {
        self.exited.lock().unwrap().push(Arc::clone(&served.exited));
        self
    }
}

impl Env for TestEnv {
    fn owner_assessment(
        &self,
        pid: u32,
        created: Option<u64>,
    ) -> qc_native_platform::owner_identity::Assessment {
        self.owner_override.unwrap_or_else(|| {
            if self.alive(pid, created) {
                qc_native_platform::owner_identity::Assessment::Same
            } else {
                qc_native_platform::owner_identity::Assessment::GoneOrDifferent
            }
        })
    }
    fn own_directory(&self) -> Option<PathBuf> {
        Some(self.directory.clone())
    }
    fn alive(&self, pid: u32, created_at: Option<u64>) -> bool {
        pid == std::process::id()
            && created_at.is_some()
            && created_at == test_support::own_created_at()
            && !self
                .exited
                .lock()
                .unwrap()
                .iter()
                .any(|exited| exited.load(Ordering::Acquire))
    }
    fn session_marker_present(&self) -> bool {
        self.marker.load(Ordering::Acquire)
    }
    fn interactive(&self) -> bool {
        self.interactive
    }
    fn spawn_resident(&self, directory: &Path) -> Result<Box<dyn Spawned>, String> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        match &self.spawner {
            Some(spawner) => spawner(directory),
            None => panic!("this test must never start a resident host"),
        }
    }
    fn client_pid(&self) -> u32 {
        4242
    }
    fn parent_pid(&self) -> Option<u32> {
        Some(4141)
    }
    fn timeouts(&self) -> Timeouts {
        Timeouts {
            connect: Duration::from_secs(2),
            authenticate: Duration::from_secs(3),
            status: Duration::from_secs(3),
            action: Duration::from_secs(5),
            quit_exit: Duration::from_secs(3),
            start: Duration::from_secs(3),
        }
    }
}

struct FakeChild {
    exit: Option<Option<i32>>,
}

impl Spawned for FakeChild {
    fn pid(&self) -> u32 {
        std::process::id()
    }
    fn exit_code(&mut self) -> Result<Option<Option<i32>>, String> {
        Ok(self.exit)
    }
}

fn invoke(command: Command, confirm: bool, env: &dyn Env) -> Outcome {
    run(
        &Invocation {
            command,
            directory: None,
            confirm,
        },
        "9.9.9",
        env,
    )
}

#[test]
fn owner_recovery_control_unknown_is_not_reported_dead_or_sent_actions() {
    let root = Scratch::new("owner-unknown");
    let mut claim = qc_owner_only_file::ClaimedFile::try_claim(&root.descriptor()).unwrap();
    let bytes = serde_json::to_vec(&serde_json::json!({"processId":42,"processCreatedAt":7,
        "port":1,"bearerToken":"synthetic","instanceId":"fixture","publishedAtUnixMs":1}))
    .unwrap();
    claim.write(&bytes).unwrap();
    let mut env = TestEnv::new(&root.0);
    env.owner_override = Some(qc_native_platform::owner_identity::Assessment::Unknown);
    let status = invoke(Command::Status, false, &env);
    assert_eq!(status.report["outcome"], "ownerIdentityUnavailable");
    assert!(status.report["running"].is_null());
    assert_eq!(
        status.report["descriptor"]["reason"],
        "processIdentityUnavailable"
    );
    assert!(status.report["descriptor"]["live"].is_null());
    let connect = invoke(Command::Connect, false, &env);
    assert_eq!(connect.exit_code, exit::REFUSED);
    assert_eq!(connect.report["outcome"], "ownerIdentityUnavailable");
    assert_eq!(fs::read(root.descriptor()).unwrap(), bytes);
    assert_eq!(env.spawns.load(Ordering::Acquire), 0);
    claim.release().unwrap();
}

/// A resident broker whose icon holds the device indefinitely: nothing reaps
/// it while a test runs.
fn resident(scratch: &Scratch) -> Served {
    test_support::serve(&scratch.descriptor(), scratch.1.clone(), |handle| {
        handle.set_resident(true);
        handle.set_tray_hold(Some(IdlePolicy::Never));
    })
}

fn token(scratch: &Scratch) -> String {
    let descriptor: Value =
        serde_json::from_slice(&fs::read(scratch.descriptor()).unwrap()).unwrap();
    descriptor["bearerToken"].as_str().unwrap().to_owned()
}

fn port(scratch: &Scratch) -> u16 {
    let descriptor: Value =
        serde_json::from_slice(&fs::read(scratch.descriptor()).unwrap()).unwrap();
    descriptor["port"].as_u64().unwrap() as u16
}

fn frame(stream: &mut TcpStream, value: Value) -> Value {
    let body = serde_json::to_vec(&value).unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
    let id = value["id"].clone();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    loop {
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).unwrap();
        let mut body = vec![0_u8; u32::from_be_bytes(header) as usize];
        stream.read_exact(&mut body).unwrap();
        let reply: Value = serde_json::from_slice(&body).unwrap();
        if reply.get("id") == Some(&id) {
            return reply;
        }
    }
}

/// A window: an ordinary participant, as every client before the control
/// role authenticated.
fn participant(scratch: &Scratch) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", port(scratch))).unwrap();
    let reply = frame(
        &mut stream,
        json!({"method": "broker.authenticate", "params": {"token": token(scratch)}, "id": 0}),
    );
    assert!(reply["result"]["instanceId"].is_string(), "{reply}");
    assert!(
        reply["result"].get("role").is_none(),
        "unchanged for participants"
    );
    stream
}

/// This process's own background MCP host, recognised by its token.
fn host(scratch: &Scratch) -> TcpStream {
    let mut stream = participant(scratch);
    let reply = frame(
        &mut stream,
        json!({"jsonrpc": "2.0", "id": 1, "method": "broker.hello", "params": {
            "provider": crate::rpc::IN_PROCESS_HOST_PROVIDER,
            "hostToken": crate::resident::host_token(),
        }}),
    );
    assert!(reply.get("error").is_none(), "{reply}");
    stream
}

fn wait_for(within: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    condition()
}

fn wait_for_participants(handle: &BrokerHandle, count: usize) {
    assert!(
        wait_for(Duration::from_secs(3), || handle.participants().len()
            == count),
        "expected {count} participants, have {:?}",
        handle.participants()
    );
}

fn audit_lines(scratch: &Scratch) -> Vec<Value> {
    fs::read_to_string(scratch.0.join(AUDIT_LOG))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn assert_no_token(scratch: &Scratch, secret: &str, outcome: &Outcome) {
    let printed = serde_json::to_string(&outcome.report).unwrap();
    assert!(!printed.contains(secret), "the report carries the token");
    let log = fs::read_to_string(scratch.0.join(AUDIT_LOG)).unwrap_or_default();
    assert!(!log.contains(secret), "the audit log carries the token");
}

fn keys(report: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = report
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn status_reports_a_live_broker_with_stable_keys_and_no_token() {
    let scratch = Scratch::new("status");
    let served = resident(&scratch);
    let _window = participant(&scratch);
    let _host = host(&scratch);
    wait_for_participants(&served.handle, 2);
    let env = TestEnv::new(&scratch.0).watching(&served);
    let outcome = invoke(Command::Status, false, &env);
    assert_eq!(outcome.exit_code, exit::OK, "{}", outcome.report);
    let report = &outcome.report;
    assert_eq!(
        keys(report),
        [
            "broker",
            "claim",
            "command",
            "controlVersion",
            "descriptor",
            "detail",
            "exitCode",
            "ok",
            "outcome",
            "releaseMarkers",
            "running",
            "schema",
            "sessionMarker",
            "trayEnabled"
        ]
    );
    assert_eq!(report["schema"], SCHEMA);
    assert_eq!(report["command"], "status");
    assert_eq!(report["outcome"], "running");
    assert_eq!(report["running"], true);
    assert_eq!(report["claim"], "heldByLiveOwner");
    assert_eq!(report["sessionMarker"], false);
    assert_eq!(report["trayEnabled"], true);
    assert_eq!(
        report["releaseMarkers"],
        json!({"count": 0, "newestUnixMs": null, "currentInstance": false})
    );
    assert_eq!(report["descriptor"]["live"], true);
    assert_eq!(report["descriptor"]["processId"], std::process::id());
    assert_eq!(
        keys(&report["descriptor"]),
        [
            "gatewayApiVersion",
            "live",
            "present",
            "processCreatedAt",
            "processId",
            "publishedAtUnixMs",
            "reason"
        ]
    );
    let broker = &report["broker"];
    assert_eq!(
        keys(broker),
        [
            "connected",
            "controlConnections",
            "gatewayApiVersion",
            "hostAttached",
            "hostClients",
            "idleReleased",
            "observers",
            "participants",
            "participantsOtherThanHost",
            "pendingAuthentications",
            "phase",
            "processCreatedAt",
            "processId",
            "reopensInFlight",
            "resident",
            "shuttingDown",
            "trayHold"
        ]
    );
    assert_eq!(broker["participants"], 2);
    assert_eq!(broker["participantsOtherThanHost"], 1);
    assert_eq!(broker["hostAttached"], true);
    assert_eq!(broker["observers"], 0);
    assert_eq!(broker["controlConnections"], 1, "only itself");
    assert_eq!(broker["resident"], true);
    assert_eq!(broker["trayHold"], json!({"policy": "never"}));
    assert_eq!(broker["connected"], false);
    assert_eq!(broker["hostClients"], Value::Null);
    assert_eq!(broker["shuttingDown"], false);
    assert!(audit_lines(&scratch).is_empty(), "status is not audited");
    assert_no_token(&scratch, &token(&scratch), &outcome);
    test_support::finish(&served.handle);
}

#[test]
fn status_without_a_broker_is_exit_zero_and_running_false() {
    let scratch = Scratch::new("status-none");
    let outcome = invoke(Command::Status, false, &TestEnv::new(&scratch.0));
    assert_eq!(outcome.exit_code, exit::OK);
    assert_eq!(outcome.report["running"], false);
    assert_eq!(outcome.report["broker"], Value::Null);
    assert_eq!(outcome.report["outcome"], "notRunning");
    assert_eq!(outcome.report["claim"], "absent");
    assert_eq!(
        outcome.report["descriptor"],
        json!({"present": false, "live": false, "reason": null})
    );
}

/// A resident started under a launcher's MSIX file-system virtualisation
/// publishes where this caller cannot see it, while its session marker (a
/// named mutex) is shared. That is not "nothing runs": status says so, and
/// start refuses instead of electing a second resident beside it.
#[test]
fn a_marker_without_a_visible_descriptor_is_neither_not_running_nor_started_over() {
    let scratch = Scratch::new("hidden-resident");
    let env = TestEnv::new(&scratch.0);
    env.marker.store(true, Ordering::Release);
    let outcome = invoke(Command::Status, false, &env);
    assert_eq!(outcome.exit_code, exit::OK);
    assert_eq!(outcome.report["outcome"], "residentNotVisible");
    assert_eq!(outcome.report["running"], false);
    assert_eq!(outcome.report["sessionMarker"], true);
    assert_eq!(outcome.report["broker"], Value::Null);
    let message = outcome.report["detail"].as_str().unwrap();
    assert!(message.contains("QC Remote tray menu"), "{message}");
    assert!(message.contains("same app context"), "{message}");
    let outcome = invoke(Command::Start, false, &env);
    assert_eq!(outcome.exit_code, exit::REFUSED, "{}", outcome.report);
    assert_eq!(outcome.report["outcome"], "residentNotVisible");
    assert_eq!(outcome.report["before"]["sessionMarker"], true);
    assert_eq!(env.spawns.load(Ordering::SeqCst), 0, "nothing was started");
}

/// The control role is decided in the handshake, so the broker never counts
/// it as a participant, never resets an idle origin, and its departure never
/// starts a grace that nothing else had pending.
#[test]
fn status_moves_no_count_no_idle_clock_and_arms_no_grace() {
    let scratch = Scratch::new("status-clock");
    let served = test_support::serve(&scratch.descriptor(), scratch.1.clone(), |handle| {
        handle.set_resident(true);
        handle.set_tray_hold(Some(IdlePolicy::Minutes(5)));
    });
    test_support::set_grace(&served.handle, Duration::from_millis(150));
    let env = TestEnv::new(&scratch.0).watching(&served);
    let before = test_support::idle_origins(&served.handle);
    assert!(before.0.is_some(), "the icon's idle origin is running");
    for _ in 0..3 {
        assert_eq!(invoke(Command::Status, false, &env).exit_code, exit::OK);
    }
    assert!(wait_for(Duration::from_secs(2), || {
        test_support::connection_count(&served.handle) == 0
    }));
    assert_eq!(test_support::idle_origins(&served.handle), before);
    assert!(served.handle.participants().is_empty());

    // An open device with nobody attached and no hold past the grace: only a
    // departure arms the grace that would release it. A status read is no
    // departure.
    served.handle.set_tray_hold(Some(IdlePolicy::Immediately));
    // Dropping the hold re-arms the grace once; let it release first.
    assert!(wait_for(Duration::from_secs(2), || {
        test_support::idle_released(&served.handle)
    }));
    served.handle.reconnect().unwrap();
    assert!(!test_support::idle_released(&served.handle));
    assert_eq!(invoke(Command::Status, false, &env).exit_code, exit::OK);
    std::thread::sleep(Duration::from_millis(600));
    assert!(
        !test_support::idle_released(&served.handle),
        "a status read armed an idle release"
    );

    // A participant leaving while the status connection is still counted
    // keeps its grace: the control connection's departure arms it instead.
    let window = participant(&scratch);
    wait_for_participants(&served.handle, 1);
    let mut control = TcpStream::connect(("127.0.0.1", port(&scratch))).unwrap();
    let reply = frame(
        &mut control,
        json!({"method": "broker.authenticate",
               "params": {"token": token(&scratch), "role": "control"}, "id": 0}),
    );
    assert_eq!(reply["result"]["role"], "control");
    drop(window);
    wait_for_participants(&served.handle, 0);
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        !test_support::idle_released(&served.handle),
        "still counted"
    );
    drop(control);
    assert!(
        wait_for(Duration::from_secs(2), || test_support::idle_released(
            &served.handle
        )),
        "the deferred grace was lost"
    );
    test_support::finish(&served.handle);
}

/// The backstop watcher parks with no deadline while any socket is still
/// authenticating. A control connection must wake it again when it
/// authenticates and when it leaves, or one status read leaves the device
/// held under the icon's idle policy until some unrelated client event.
#[test]
fn a_control_connection_never_parks_the_icons_idle_release() {
    let scratch = Scratch::new("backstop");
    let served = test_support::serve(&scratch.descriptor(), scratch.1.clone(), |handle| {
        handle.set_resident(true);
    });
    let minute = Duration::from_millis(600);
    test_support::set_policy_minute(&served.handle, minute);
    served.handle.set_tray_hold(Some(IdlePolicy::Minutes(1)));
    let idle_from = test_support::idle_origins(&served.handle)
        .0
        .expect("tray idle origin is armed");
    assert!(!test_support::idle_released(&served.handle));
    // Accepted and still authenticating: the watcher wakes and parks.
    let mut control = TcpStream::connect(("127.0.0.1", port(&scratch))).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    let reply = frame(
        &mut control,
        json!({"method": "broker.authenticate",
               "params": {"token": token(&scratch), "role": "control"}, "id": 0}),
    );
    assert_eq!(reply["result"]["role"], "control");
    let status = frame(
        &mut control,
        json!({"jsonrpc": "2.0", "id": 1, "method": "broker.controlStatus", "params": {}}),
    );
    assert_eq!(status["result"]["idleReleased"], false);
    drop(control);
    assert!(
        wait_for(minute * 4, || test_support::idle_released(&served.handle)),
        "the icon's idle release did not happen after a control connection"
    );
    assert!(idle_from.elapsed() >= minute, "not before the policy time");
    test_support::finish(&served.handle);
}

#[test]
fn a_control_connection_is_refused_every_other_method_and_an_unknown_role_is_refused() {
    let scratch = Scratch::new("control-methods");
    let served = resident(&scratch);
    let mut stream = TcpStream::connect(("127.0.0.1", port(&scratch))).unwrap();
    let refused = frame(
        &mut stream,
        json!({"method": "broker.authenticate",
               "params": {"token": token(&scratch), "role": "admin"}, "id": 0}),
    );
    assert_eq!(refused["error"]["message"], "Unsupported connection role");
    let mut control = TcpStream::connect(("127.0.0.1", port(&scratch))).unwrap();
    frame(
        &mut control,
        json!({"method": "broker.authenticate",
               "params": {"token": token(&scratch), "role": "control"}, "id": 0}),
    );
    for (id, method, params) in [
        (1, "system.status", json!({})),
        (2, "broker.hello", json!({"role": "participant"})),
        (3, "broker.clients", json!({})),
        (4, "broker.disconnectDevice", json!({})),
        (5, "device.reconnect", json!({})),
        (6, "broker.shutdown", json!({})),
        (7, "broker.shutdown", json!({"force": false})),
    ] {
        let reply = frame(
            &mut control,
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        );
        assert!(reply.get("error").is_some(), "{method}: {reply}");
    }
    assert!(!served.exited.load(Ordering::Acquire));
    assert!(served.handle.participants().is_empty());
    test_support::finish(&served.handle);
}

#[test]
fn release_is_the_icons_disconnect_without_its_exit_and_is_idempotent() {
    let scratch = Scratch::new("release");
    let served = resident(&scratch);
    let _host = host(&scratch);
    wait_for_participants(&served.handle, 1);
    let env = TestEnv::new(&scratch.0).watching(&served);
    for _ in 0..2 {
        let outcome = invoke(Command::Release, false, &env);
        assert_eq!(outcome.exit_code, exit::OK, "{}", outcome.report);
        assert_eq!(outcome.report["outcome"], "released");
        assert_eq!(outcome.report["result"]["released"], true);
        assert!(test_support::idle_released(&served.handle));
        assert!(!served.exited.load(Ordering::Acquire));
    }
    // A window attached: release still closes USB, not the resident or clients.
    served.handle.reconnect().unwrap();
    assert!(!test_support::idle_released(&served.handle));
    let _window = participant(&scratch);
    wait_for_participants(&served.handle, 2);
    let released = invoke(Command::Release, false, &env);
    assert_eq!(released.exit_code, exit::OK, "{}", released.report);
    assert_eq!(released.report["outcome"], "released");
    assert_eq!(released.report["result"]["released"], true);
    assert!(test_support::idle_released(&served.handle));
    assert!(!served.exited.load(Ordering::Acquire));
    assert!(!served.handle.shutting_down());
    assert_eq!(served.handle.participants().len(), 2);
    let lines = audit_lines(&scratch);
    assert_eq!(lines.len(), 3);
    for line in &lines {
        assert_eq!(
            keys(line),
            [
                "atUtc",
                "clientPid",
                "command",
                "exitCode",
                "outcome",
                "parentPid"
            ]
        );
        assert_eq!(line["command"], "release");
        assert_eq!(line["clientPid"], 4242);
        assert_eq!(line["parentPid"], 4141);
    }
    assert_eq!(
        lines
            .iter()
            .map(|line| (
                line["outcome"].as_str().unwrap(),
                line["exitCode"].as_i64().unwrap()
            ))
            .collect::<Vec<_>>(),
        [("released", 0), ("released", 0), ("released", 0)]
    );
    assert_no_token(&scratch, &token(&scratch), &released);
    test_support::finish(&served.handle);
}

#[test]
fn connect_is_the_icons_connect() {
    let scratch = Scratch::new("connect");
    let served = test_support::serve(&scratch.descriptor(), scratch.1.clone(), |handle| {
        handle.set_resident(true);
        handle.set_tray_hold(Some(IdlePolicy::Never));
        handle.mark_started_disconnected();
    });
    assert!(test_support::idle_released(&served.handle));
    let env = TestEnv::new(&scratch.0).watching(&served);
    let outcome = invoke(Command::Connect, false, &env);
    assert_eq!(outcome.exit_code, exit::OK, "{}", outcome.report);
    assert_eq!(outcome.report["outcome"], "connecting");
    assert_eq!(outcome.report["result"]["connecting"], true);
    assert!(!test_support::idle_released(&served.handle));
    assert_eq!(audit_lines(&scratch)[0]["outcome"], "connecting");
    test_support::finish(&served.handle);
}

#[test]
fn release_and_connect_without_a_broker_send_nothing_and_exit_three() {
    let scratch = Scratch::new("absent");
    for command in [Command::Release, Command::Connect, Command::Quit] {
        let outcome = invoke(command, true, &TestEnv::new(&scratch.0));
        assert_eq!(outcome.exit_code, exit::NOT_RUNNING, "{command:?}");
        assert_eq!(outcome.report["outcome"], "notRunning");
    }
    assert_eq!(audit_lines(&scratch).len(), 3);
}

#[test]
fn quit_without_confirm_refuses_with_no_effect_on_the_broker() {
    let scratch = Scratch::new("quit-refused");
    let served = resident(&scratch);
    let _window = participant(&scratch);
    wait_for_participants(&served.handle, 1);
    let env = TestEnv::new(&scratch.0).watching(&served);
    let outcome = invoke(Command::Quit, false, &env);
    assert_eq!(outcome.exit_code, exit::REFUSED);
    assert_eq!(outcome.report["outcome"], "confirmRequired");
    assert_eq!(
        test_support::connection_count(&served.handle),
        1,
        "never connected"
    );
    assert!(!served.exited.load(Ordering::Acquire));
    assert!(!served.handle.shutting_down());
    assert!(scratch.descriptor().exists());
    assert!(
        audit_lines(&scratch).is_empty(),
        "a refusal leaves no audit line"
    );
    assert!(!scratch.0.join(AUDIT_LOG).exists());
    assert!(outcome.report.get("auditLogged").is_none());
    test_support::finish(&served.handle);
}

#[test]
fn quit_with_clients_attached_releases_everything_and_waits_for_the_exit() {
    let scratch = Scratch::new("quit");
    let served = resident(&scratch);
    let secret = token(&scratch);
    let instance = test_support::instance_id(&served.handle);
    let _window = participant(&scratch);
    let _host = host(&scratch);
    wait_for_participants(&served.handle, 2);
    let env = TestEnv::new(&scratch.0).watching(&served);
    let outcome = invoke(Command::Quit, true, &env);
    assert_eq!(outcome.exit_code, exit::OK, "{}", outcome.report);
    assert_eq!(outcome.report["outcome"], "quit");
    assert_eq!(
        outcome.report["verified"],
        json!({
            "processExited": true,
            "descriptorReleased": true,
            "claimReleased": true,
            "claim": "absent",
            "sessionMarkerReleased": true,
            "releaseMarkerWritten": true,
            "userQuitRecorded": true,
            "successorProcessId": null,
        })
    );
    assert_eq!(
        crate::user_quit::read(&scratch.descriptor()),
        Some(Ok(crate::user_quit::UserQuitRecord {
            instance_id: instance.clone(),
            source: crate::user_quit::UserQuitSource::Control,
        })),
        "the user's Quit stands for this generation until a user action"
    );
    assert!(served.exited.load(Ordering::Acquire));
    assert!(!scratch.descriptor().exists());
    assert!(!qc_owner_only_file::claim_path(&scratch.descriptor()).exists());
    assert!(
        crate::generated_broker_surface::BrokerReleaseMarker::path_for(
            &scratch.descriptor(),
            &instance
        )
        .exists(),
        "the forced-release marker that keeps clients from restarting it"
    );
    assert_no_token(&scratch, &secret, &outcome);
    // Idempotent: nothing runs now, nothing is sent.
    let again = invoke(Command::Quit, true, &env);
    assert_eq!(again.exit_code, exit::NOT_RUNNING);
    assert_eq!(
        audit_lines(&scratch)
            .iter()
            .map(|line| line["outcome"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        ["quit", "notRunning"]
    );
}

/// A descriptor naming a dead pid, or a recycled one (creation time
/// mismatch), is never acted on, even when its port and token reach a live
/// broker: nothing is sent, so nothing is shut down.
#[test]
fn a_stale_descriptor_is_refused_and_never_shuts_anything_down() {
    let live = Scratch::new("stale-live");
    let served = resident(&live);
    for (label, pid, created) in [
        ("recycled", std::process::id(), Some(1_u64)),
        ("dead", u32::MAX - 7, test_support::own_created_at()),
    ] {
        let stale = Scratch::new(label);
        let descriptor = json!({
            "port": port(&live), "bearerToken": token(&live), "instanceId": "x",
            "processId": pid, "processCreatedAt": created,
            "publishedAtUnixMs": 1, "gatewayApiVersion": 3,
        });
        qc_owner_only_file::write_staged(&stale.descriptor(), descriptor.to_string().as_bytes())
            .unwrap();
        let env = TestEnv::new(&stale.0).watching(&served);
        for command in [Command::Quit, Command::Release, Command::Connect] {
            let outcome = invoke(command, true, &env);
            assert_eq!(outcome.exit_code, exit::NOT_RUNNING, "{label} {command:?}");
            assert_eq!(outcome.report["outcome"], "staleDescriptor");
            assert!(!served.exited.load(Ordering::Acquire));
            assert_eq!(test_support::connection_count(&served.handle), 0);
        }
        let status = invoke(Command::Status, false, &env);
        assert_eq!(status.report["running"], false);
        assert_eq!(
            status.report["descriptor"]["reason"],
            "processGoneOrRecycled"
        );
        assert!(stale.descriptor().exists(), "never deleted");
    }
    // A descriptor anyone could have written is not trusted at all.
    let foreign = Scratch::new("not-owner-only");
    fs::write(
        foreign.descriptor(),
        json!({"port": 1, "bearerToken": "t", "instanceId": "i", "processId": 7,
               "processCreatedAt": 1, "publishedAtUnixMs": 1})
        .to_string(),
    )
    .unwrap();
    #[cfg(windows)]
    {
        let outcome = invoke(Command::Quit, true, &TestEnv::new(&foreign.0));
        assert_eq!(outcome.exit_code, exit::REFUSED);
        assert_eq!(outcome.report["outcome"], "descriptorRefused");
    }
    test_support::finish(&served.handle);
}

#[test]
fn start_is_idempotent_while_a_broker_serves() {
    let scratch = Scratch::new("start-running");
    let served = resident(&scratch);
    let env = TestEnv::new(&scratch.0).watching(&served);
    let outcome = invoke(Command::Start, false, &env);
    assert_eq!(outcome.exit_code, exit::OK);
    assert_eq!(outcome.report["outcome"], "alreadyRunning");
    assert_eq!(outcome.report["started"], false);
    assert_eq!(outcome.report["processId"], std::process::id());
    assert_eq!(env.spawns.load(Ordering::SeqCst), 0, "nothing was started");
    test_support::finish(&served.handle);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_control_refuses_a_symlinked_descriptor_without_authenticating() {
    use std::os::unix::fs::symlink;
    let live = Scratch::new("linux-live-descriptor");
    let served = resident(&live);
    let redirected = Scratch::new("linux-symlink-descriptor");
    symlink(live.descriptor(), redirected.descriptor()).unwrap();
    let env = TestEnv::new(&redirected.0).watching(&served);
    for command in [Command::Release, Command::Connect, Command::Quit] {
        let outcome = invoke(command, true, &env);
        assert_eq!(outcome.exit_code, exit::REFUSED, "{}", outcome.report);
        assert!(!served.exited.load(Ordering::Acquire));
        assert_eq!(test_support::connection_count(&served.handle), 0);
    }
    test_support::finish(&served.handle);
}

/// `quit --confirm` records the user's Quit with the tray switch off too, and
/// with no client attached: the record is the broker's own, written before
/// the shutdown commits, bound to the generation that was quit.
#[test]
fn quit_records_the_users_quit_with_the_tray_switched_off() {
    let scratch = Scratch::new("quit-tray-off");
    fs::write(
        scratch.0.join(crate::tray_settings::SETTINGS_FILE),
        r#"{"trayEnabled": false}"#,
    )
    .unwrap();
    assert!(!crate::tray_settings::read_in(&scratch.0).tray_enabled);
    let served = resident(&scratch);
    let instance = test_support::instance_id(&served.handle);
    let env = TestEnv::new(&scratch.0).watching(&served);
    let outcome = invoke(Command::Quit, true, &env);
    assert_eq!(outcome.exit_code, exit::OK, "{}", outcome.report);
    assert_eq!(outcome.report["verified"]["userQuitRecorded"], true);
    assert!(matches!(
        crate::user_quit::read(&scratch.descriptor()),
        Some(Ok(record)) if record.instance_id == instance
    ));
}

/// `start` is a user action: it clears a recorded Quit when it starts and
/// when a broker already serves, says so, and keeps it when it refuses. The
/// record is gone before the resident start is spawned.
#[test]
fn start_clears_a_recorded_quit_and_a_refused_start_keeps_it() {
    let record = |scratch: &Scratch| {
        crate::user_quit::publish(
            &scratch.descriptor(),
            &crate::user_quit::UserQuitRecord {
                instance_id: "quit-generation".into(),
                source: crate::user_quit::UserQuitSource::Tray,
            },
        )
        .unwrap()
    };

    let refused = Scratch::new("start-quit-refused");
    record(&refused);
    let mut env = TestEnv::new(&refused.0);
    env.interactive = false;
    let outcome = invoke(Command::Start, false, &env);
    assert_eq!(outcome.report["outcome"], "notInteractive");
    assert!(
        crate::user_quit::read(&refused.descriptor()).is_some(),
        "nothing started, so the Quit still stands"
    );

    let spawning = Scratch::new("start-quit-spawn");
    record(&spawning);
    let mut env = TestEnv::new(&spawning.0);
    env.spawner = Some(Box::new(|directory| {
        assert!(
            crate::user_quit::read(&directory.join(DESCRIPTOR_FILE)).is_none(),
            "cleared before the resident start"
        );
        Ok(Box::new(FakeChild {
            exit: Some(Some(0)),
        }))
    }));
    let outcome = invoke(Command::Start, false, &env);
    assert_eq!(outcome.report["outcome"], "residentStartDeclined");
    assert_eq!(outcome.report["userQuitCleared"], true);
    assert_eq!(env.spawns.load(Ordering::SeqCst), 1);

    let running = Scratch::new("start-quit-running");
    let served = resident(&running);
    record(&running);
    let env = TestEnv::new(&running.0).watching(&served);
    let outcome = invoke(Command::Start, false, &env);
    assert_eq!(outcome.report["outcome"], "alreadyRunning");
    assert_eq!(outcome.report["userQuitCleared"], true);
    assert!(outcome.report["detail"]
        .as_str()
        .unwrap()
        .contains("recorded Quit was cleared"));
    assert!(crate::user_quit::read(&running.descriptor()).is_none());
    assert_eq!(env.spawns.load(Ordering::SeqCst), 0);
    test_support::finish(&served.handle);
}

/// One clear point per start. `start` clears the Quit standing when the user
/// ran it, then pauses in its spawn; meanwhile a later Quit (of a broker
/// another user start brought up) is recorded; the start resumes, and the
/// resident it started runs its entry with `--keep-user-quit`, exactly as the
/// spawned process would. The later Quit survives, and that resident does not
/// serve under it.
#[test]
fn a_start_paused_before_its_resident_never_erases_a_later_quit() {
    let scratch = Scratch::new("start-paused-later-quit");
    let record = |instance: &str| crate::user_quit::UserQuitRecord {
        instance_id: instance.into(),
        source: crate::user_quit::UserQuitSource::Tray,
    };
    crate::user_quit::publish(&scratch.descriptor(), &record("earlier-quit")).unwrap();
    let mut env = TestEnv::new(&scratch.0);
    env.spawner = Some(Box::new(move |directory| {
        let descriptor = directory.join(DESCRIPTOR_FILE);
        assert!(
            crate::user_quit::read(&descriptor).is_none(),
            "the start's own clear point ran before the spawn"
        );
        // Paused here: the later Quit is recorded.
        crate::user_quit::publish(&descriptor, &record("later-quit")).unwrap();
        // Resumed: the spawned resident's entry.
        assert_eq!(
            crate::cli::resident_entry(&descriptor, true),
            crate::cli::ResidentEntry::Decline
        );
        Ok(Box::new(FakeChild {
            exit: Some(Some(0)),
        }))
    }));
    let outcome = invoke(Command::Start, false, &env);
    assert_eq!(
        outcome.report["userQuitCleared"], true,
        "{}",
        outcome.report
    );
    assert_eq!(outcome.report["outcome"], "residentStartDeclined");
    assert_eq!(
        crate::user_quit::read(&scratch.descriptor()),
        Some(Ok(record("later-quit"))),
        "the later Quit survives the earlier start"
    );
}

/// A user's own resident entry (the sign-in Run value, the Settings switch)
/// is its start's one clear point; a `--keep-user-quit` entry with nothing
/// recorded serves; a clear that cannot take the election lock stops the
/// entry instead of serving, and the record stands.
#[test]
fn the_resident_entry_clears_once_and_stops_when_it_cannot() {
    let scratch = Scratch::new("resident-entry");
    let descriptor = scratch.descriptor();
    let record = crate::user_quit::UserQuitRecord {
        instance_id: "quit".into(),
        source: crate::user_quit::UserQuitSource::Control,
    };
    assert_eq!(
        crate::cli::resident_entry(&descriptor, true),
        crate::cli::ResidentEntry::Serve
    );
    crate::user_quit::publish(&descriptor, &record).unwrap();
    let held =
        qc_owner_only_file::ClaimedFile::try_claim(&crate::resident::election_path(&descriptor))
            .expect("this test holds the election");
    assert!(matches!(
        crate::cli::resident_entry(&descriptor, false),
        crate::cli::ResidentEntry::Fail(error) if error.contains("Could not clear")
    ));
    assert_eq!(crate::user_quit::read(&descriptor), Some(Ok(record)));
    held.release().unwrap();
    assert_eq!(
        crate::cli::resident_entry(&descriptor, false),
        crate::cli::ResidentEntry::Serve
    );
    assert_eq!(
        crate::user_quit::read(&descriptor),
        None,
        "cleared once, here"
    );
}

/// The entry is not the last word. On both paths (a plain
/// `--resident` that cleared, and `--keep-user-quit` with nothing recorded)
/// the entry says Serve; a later Quit is recorded before the election; the
/// admission run under the returned `ElectionGuard` refuses, nothing is
/// cleared, and the record survives.
#[test]
fn a_quit_recorded_after_the_entry_refuses_admission_under_the_election() {
    for keep_user_quit in [false, true] {
        let scratch = Scratch::new("admission-after-entry");
        let descriptor = scratch.descriptor();
        let later = crate::user_quit::UserQuitRecord {
            instance_id: "later-quit".into(),
            source: crate::user_quit::UserQuitSource::Tray,
        };
        assert_eq!(
            crate::cli::resident_entry(&descriptor, keep_user_quit),
            crate::cli::ResidentEntry::Serve
        );
        crate::user_quit::publish(&descriptor, &later).unwrap();
        let guard = match crate::resident::elect(&descriptor, Duration::from_secs(2)).unwrap() {
            crate::resident::Election::Serve(guard) => guard,
            crate::resident::Election::AlreadyServed => panic!("nothing serves here"),
        };
        assert!(
            !crate::cli::resident_admitted(&descriptor),
            "keep_user_quit={keep_user_quit}: the later Quit refuses admission"
        );
        drop(guard);
        assert_eq!(
            crate::user_quit::read(&descriptor),
            Some(Ok(later)),
            "keep_user_quit={keep_user_quit}: never cleared at admission"
        );
        assert!(!descriptor.exists(), "nothing was published");
    }
}

/// A killed broker's unlocked `.claim` is reported and left to the resident
/// start's own election, which clears it under its lock; the control client
/// never deletes it, and never hangs on it.
#[test]
fn start_reports_a_stale_claim_without_deleting_it() {
    let scratch = Scratch::new("start-stale");
    drop(qc_owner_only_file::ClaimedFile::try_claim(&scratch.descriptor()).unwrap());
    fs::remove_file(scratch.descriptor()).unwrap();
    assert!(qc_owner_only_file::claim_path(&scratch.descriptor()).exists());
    // A resident start that declines (exits 0, publishes nothing).
    let mut env = TestEnv::new(&scratch.0);
    env.spawner = Some(Box::new(|_| {
        Ok(Box::new(FakeChild {
            exit: Some(Some(0)),
        }))
    }));
    let started = Instant::now();
    let outcome = invoke(Command::Start, false, &env);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(outcome.exit_code, exit::BROKER_ERROR, "{}", outcome.report);
    assert_eq!(outcome.report["outcome"], "residentStartDeclined");
    assert_eq!(outcome.report["before"]["claim"], "staleUnlocked");
    assert_eq!(outcome.report["staleClaim"], true);
    assert!(
        qc_owner_only_file::claim_path(&scratch.descriptor()).exists(),
        "never deleted"
    );

    // The real election (what `--resident` runs) takes it over and serves.
    let served: Arc<Mutex<Option<Served>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&served);
    let owned_root = scratch.1.clone();
    env.spawner = Some(Box::new(move |directory| {
        let descriptor = directory.join(DESCRIPTOR_FILE);
        match crate::resident::elect(&descriptor, Duration::from_secs(2))? {
            crate::resident::Election::Serve(guard) => {
                let broker = test_support::serve(&descriptor, owned_root.clone(), move |handle| {
                    drop(guard);
                    handle.set_resident(true);
                    handle.set_tray_hold(Some(IdlePolicy::Never));
                });
                *slot.lock().unwrap() = Some(broker);
                Ok(Box::new(FakeChild { exit: None }) as Box<dyn Spawned>)
            }
            crate::resident::Election::AlreadyServed => Err("not elected".into()),
        }
    }));
    let outcome = invoke(Command::Start, false, &env);
    assert_eq!(outcome.exit_code, exit::OK, "{}", outcome.report);
    assert_eq!(outcome.report["outcome"], "started");
    assert_eq!(outcome.report["started"], true);
    assert_eq!(outcome.report["servedBySpawned"], true);
    assert_eq!(outcome.report["staleClaim"], true);
    let served = served.lock().unwrap().take().unwrap();
    assert_eq!(
        invoke(Command::Status, false, &env.watching(&served)).report["claim"],
        "heldByLiveOwner"
    );
    test_support::finish(&served.handle);
}

#[test]
fn start_refuses_where_resident_would_not_serve_and_reports_a_failed_start() {
    let scratch = Scratch::new("start-refused");
    let settings = scratch.0.join(crate::tray_settings::SETTINGS_FILE);
    fs::write(&settings, r#"{"trayEnabled": false}"#).unwrap();
    assert!(!crate::tray_settings::read_in(&scratch.0).tray_enabled);
    let env = TestEnv::new(&scratch.0);
    let outcome = invoke(Command::Start, false, &env);
    assert_eq!(outcome.exit_code, exit::REFUSED);
    assert_eq!(outcome.report["outcome"], "trayDisabled");
    fs::remove_file(&settings).unwrap();
    let mut env = TestEnv::new(&scratch.0);
    env.interactive = false;
    assert_eq!(
        invoke(Command::Start, false, &env).report["outcome"],
        "notInteractive"
    );
    let mut env = TestEnv::new(&scratch.0);
    env.spawner = Some(Box::new(|_| {
        Ok(Box::new(FakeChild {
            exit: Some(Some(1)),
        }))
    }));
    let outcome = invoke(Command::Start, false, &env);
    assert_eq!(outcome.exit_code, exit::BROKER_ERROR);
    assert_eq!(outcome.report["outcome"], "residentStartFailed");
    assert_eq!(outcome.report["spawnedExitCode"], 1);
    let mut env = TestEnv::new(&scratch.0);
    env.spawner = Some(Box::new(|_| Ok(Box::new(FakeChild { exit: None }))));
    let outcome = invoke(Command::Start, false, &env);
    assert_eq!(outcome.exit_code, exit::TIMEOUT);
    assert_eq!(outcome.report["outcome"], "startTimeout");
}

#[test]
fn only_the_callers_own_directory_is_accepted() {
    let scratch = Scratch::new("directory");
    let env = TestEnv::new(&scratch.0);
    let other = scratch.0.join("elsewhere");
    let outcome = run(
        &Invocation {
            command: Command::Quit,
            directory: Some(other),
            confirm: true,
        },
        "9.9.9",
        &env,
    );
    assert_eq!(outcome.exit_code, exit::REFUSED);
    assert_eq!(outcome.report["outcome"], "directoryRefused");
    assert!(
        audit_lines(&scratch).is_empty(),
        "refused before touching it"
    );
    let respelled = PathBuf::from(
        scratch
            .0
            .to_string_lossy()
            .to_uppercase()
            .replace('\\', "/")
            + "/",
    );
    let outcome = run(
        &Invocation {
            command: Command::Status,
            directory: Some(respelled),
            confirm: false,
        },
        "9.9.9",
        &env,
    );
    #[cfg(windows)]
    assert_eq!(outcome.exit_code, exit::OK, "{}", outcome.report);
    #[cfg(not(windows))]
    assert_eq!(outcome.exit_code, exit::REFUSED, "{}", outcome.report);
    struct NoDirectory;
    impl Env for NoDirectory {
        fn own_directory(&self) -> Option<PathBuf> {
            None
        }
        fn alive(&self, _: u32, _: Option<u64>) -> bool {
            unreachable!()
        }
        fn session_marker_present(&self) -> bool {
            unreachable!()
        }
        fn interactive(&self) -> bool {
            unreachable!()
        }
        fn spawn_resident(&self, _: &Path) -> Result<Box<dyn Spawned>, String> {
            unreachable!()
        }
        fn client_pid(&self) -> u32 {
            0
        }
        fn parent_pid(&self) -> Option<u32> {
            None
        }
        fn timeouts(&self) -> Timeouts {
            Timeouts::default()
        }
    }
    let outcome = invoke(Command::Start, false, &NoDirectory);
    assert_eq!(outcome.exit_code, exit::REFUSED);
    assert_eq!(outcome.report["outcome"], "noAppDataDirectory");
}

#[test]
fn usage_is_one_json_report_with_exit_two() {
    let outcome = usage("bad", "9.9.9");
    assert_eq!(outcome.exit_code, exit::REFUSED);
    assert_eq!(
        outcome.report,
        json!({"schema": SCHEMA, "command": null, "ok": false, "exitCode": 2,
               "outcome": "usage", "detail": "bad", "controlVersion": "9.9.9"})
    );
    let parse =
        |args: &[&str]| super::parse(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>());
    assert!(parse(&["status"]).is_err());
    assert!(parse(&["--control", "status", "--confirm"]).is_err());
    assert!(parse(&["--control", "quit", "--confirm"]).unwrap().confirm);
}

#[test]
fn the_audit_log_is_bounded_and_timestamps_are_utc() {
    let scratch = Scratch::new("audit");
    fs::write(
        scratch.0.join(AUDIT_LOG),
        vec![b'x'; AUDIT_LOG_MAX_BYTES as usize + 1],
    )
    .unwrap();
    assert!(audit(
        &scratch.0,
        Command::Start,
        3,
        "x",
        &TestEnv::new(&scratch.0)
    ));
    let log = fs::read_to_string(scratch.0.join(AUDIT_LOG)).unwrap();
    assert_eq!(log.lines().count(), 1, "truncated before the line");
    assert_eq!(
        utc_timestamp(UNIX_EPOCH + Duration::from_secs(1_791_331_200)),
        "2026-10-07T00:00:00Z"
    );
    assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    assert_eq!(
        utc_timestamp(UNIX_EPOCH + Duration::from_secs(951_782_400 + 3_661)),
        "2000-02-29T01:01:01Z"
    );
}

#[test]
fn the_token_never_appears_in_debug_output() {
    let descriptor: Descriptor = serde_json::from_value(json!({
        "port": 1, "bearerToken": "very-secret-token", "instanceId": "i", "processId": 7,
        "processCreatedAt": 1, "publishedAtUnixMs": 1
    }))
    .unwrap();
    assert!(!format!("{descriptor:?}").contains("very-secret-token"));
}

/// A broker from before the control role ignores it and answers as it always
/// did. The client must notice (no `role` in the reply), send nothing more,
/// and say truthfully that the handshake itself reached that broker.
#[test]
fn an_older_broker_gets_only_the_handshake_and_is_reported_unsupported() {
    let scratch = Scratch::new("older-broker");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let old = OldBroker::start(listener, scratch.1.clone());
    let descriptor = json!({
        "port": port, "bearerToken": "old-broker-token", "instanceId": "old",
        "processId": std::process::id(), "processCreatedAt": test_support::own_created_at(),
        "publishedAtUnixMs": 1, "gatewayApiVersion": 3,
    });
    qc_owner_only_file::write_staged(&scratch.descriptor(), descriptor.to_string().as_bytes())
        .unwrap();
    let outcome = invoke(Command::Status, false, &TestEnv::new(&scratch.0));
    assert_eq!(outcome.exit_code, exit::BROKER_ERROR, "{}", outcome.report);
    assert_eq!(
        outcome.report["outcome"], "unsupportedBroker",
        "{}",
        outcome.report
    );
    let detail = outcome.report["detail"].as_str().unwrap();
    assert!(detail.contains("handshake"), "{detail}");
    assert!(!detail.contains("nothing was sent"), "{detail}");
    assert_eq!(old.finish(), ["broker.authenticate"], "nothing after it");
    assert_no_token(&scratch, "old-broker-token", &outcome);
}

#[test]
fn old_broker_fixture_setup_unwind_cancels_accept_before_removing_root() {
    let mut root = None;
    let mut address = None;
    let mut original_permit = None;
    let fault = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let scratch = Scratch::new("old-broker-unwind");
        root = Some(scratch.0.clone());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        address = Some(listener.local_addr().unwrap());
        let _old = OldBroker::start(listener, scratch.1.clone());
        original_permit = Some(Arc::downgrade(_old.permit.as_ref().unwrap()));
        panic!("injected descriptor setup failure before connecting");
    }));
    assert!(fault.is_err());
    assert!(
        original_permit.unwrap().upgrade().is_none(),
        "actual joined fixture returned its own admission, independent of concurrent peers"
    );
    let rebound = std::net::TcpListener::bind(address.unwrap()).expect("owned listener retired");
    drop(rebound);
    assert!(
        !root.unwrap().exists(),
        "scratch retired only after worker acknowledgement"
    );
}

#[test]
fn the_broker_version_is_the_binarys_or_absent_never_a_stand_in() {
    let mut status = json!({"phase": "ready"});
    crate::rpc::with_broker_version(&mut status, None);
    assert!(status.get("brokerVersion").is_none());
    crate::rpc::with_broker_version(&mut status, Some("0.1.180"));
    assert_eq!(status["brokerVersion"], "0.1.180");
}

#[test]
fn an_action_outlasts_the_longest_request_lock_hold() {
    // A backup holds the broker's request lock for about 215 s; a shorter
    // client bound would leave the request queued to run after we give up.
    assert!(Timeouts::default().action >= Duration::from_secs(240));
}

/// The closest a unit test gets to "the resident host never inherits the
/// harness's pipe": the helper really clears the inherit flag of a handle
/// that has it. Whether `CreateProcessW` then leaves the pipe out needs a
/// real subprocess capture.
#[cfg(windows)]
#[test]
fn clearing_the_inherit_flag_really_clears_it() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{
        GetHandleInformation, SetHandleInformation, HANDLE_FLAG_INHERIT,
    };
    let scratch = Scratch::new("inherit");
    let file = fs::File::create(scratch.0.join("handle")).unwrap();
    let handle = file.as_raw_handle();
    let flags = || {
        let mut flags = 0_u32;
        assert_ne!(unsafe { GetHandleInformation(handle, &mut flags) }, 0);
        flags & HANDLE_FLAG_INHERIT
    };
    assert_ne!(
        unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) },
        0
    );
    assert_ne!(flags(), 0, "inheritable before");
    assert!(clear_inherit(handle));
    assert_eq!(flags(), 0, "not inheritable after");
    assert!(!clear_inherit(std::ptr::null_mut()));
}

#[cfg(target_os = "macos")]
#[test]
fn resident_launch_owns_a_process_group_independent_of_the_launcher() {
    let mut command = std::process::Command::new("/bin/sh");
    command.args(["-c", "ps -o pgid= -p $$"]);
    command.stdout(std::process::Stdio::piped());
    super::detach_resident(&mut command);
    let child = command.spawn().unwrap();
    let pid = child.id();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let group: u32 = String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        group, pid,
        "resident must not inherit its launcher's process group"
    );
}
