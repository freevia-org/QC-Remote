//! `qc-device-broker.exe --control <command>`: a short-lived client that
//! automates exactly what the broker's icon does, for experiment harnesses.
//! Exit codes are distinct and stable; see [`exit`].
//!
//! It is a pure client. It finds the running broker through the descriptor in
//! the caller's own app data directory, authenticates with the token that
//! descriptor holds (read from disk, never from an argument or the
//! environment, never printed), and declares the `control` role, so the broker
//! never counts it as a participant or an observer. Only `start` ever starts a
//! process, and only `--resident`, exactly as the app does. Nothing here kills
//! a process, deletes a claim or a descriptor, or opens the USB device itself;
//! `connect` asks the running broker to, as the icon's Connect does.
//!
//! Commands, each printing one JSON object on stdout:
//! - `status`: read-only.
//! - `release`: the icon's Disconnect, never its exit.
//! - `connect`: the icon's Connect.
//! - `quit --confirm`: the icon's forced Release and exit, then a bounded wait
//!   for the process to end and a check that it released what it published.
//! - `start`: `--resident --directory <dir> --keep-user-quit` when nothing
//!   serves, after clearing a recorded Quit (its one clear point).
//!
//! This proves only that the caller runs as the same Windows user (the
//! descriptor is owner-only). It is not a boundary against a hostile process
//! running as that user, which can read the same descriptor.
use crate::generated_broker_surface::DeviceBrokerDescriptor as Descriptor;
use crate::resident::DESCRIPTOR_FILE;
use serde_json::{json, Map, Value};
use std::fs;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The `schema` every report carries. A change to any key is a new version.
pub const SCHEMA: &str = "qc-broker-control/v1";
/// The audit log beside the descriptor.
pub const AUDIT_LOG: &str = "broker-control.log";
/// Past this the log is truncated before the next line is written.
pub const AUDIT_LOG_MAX_BYTES: u64 = 64 * 1024;
const SESSION_MARKER: &str = crate::resident::SESSION_MARKER;
/// The largest frame this client reads. Every control answer is tiny.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

pub const USAGE: &str = "Usage: qc-device-broker --control <status|release|connect|quit|start> \
     [--directory <app-data-dir>] [--confirm (quit only)]";

/// Process exit codes. Distinct and stable.
pub mod exit {
    /// Done; for `status`, also when no broker runs (`running: false`).
    pub const OK: i32 = 0;
    /// Bad arguments, or refused by a guard before anything was sent: quit
    /// without `--confirm`, a foreign `--directory`, an unverifiable or
    /// non-owner-only descriptor, `start` with the switch off.
    pub const REFUSED: i32 = 2;
    /// No live broker (absent or stale descriptor) for a command that needs
    /// one. Nothing was sent anywhere.
    pub const NOT_RUNNING: i32 = 3;
    /// The broker refused or failed the command, or the result could not be
    /// verified.
    pub const BROKER_ERROR: i32 = 4;
    /// A bounded wait ran out. The outcome is unknown; read `status`.
    pub const TIMEOUT: i32 = 5;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Status,
    Release,
    Connect,
    Quit,
    Start,
}

impl Command {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "status" => Self::Status,
            "release" => Self::Release,
            "connect" => Self::Connect,
            "quit" => Self::Quit,
            "start" => Self::Start,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Release => "release",
            Self::Connect => "connect",
            Self::Quit => "quit",
            Self::Start => "start",
        }
    }

    /// Everything but `status` is audited.
    fn changes_state(self) -> bool {
        self != Self::Status
    }
}

/// Parsed `--control` arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub directory: Option<PathBuf>,
    pub confirm: bool,
}

/// `args` is the whole command line after the program name. `--control
/// <command>` comes first; then `--directory <dir>` and `--confirm` (quit
/// only), each at most once, in any order. Anything else is refused, so a
/// typo can never reach another mode.
pub fn parse(args: &[String]) -> Result<Invocation, String> {
    let (Some(flag), Some(name)) = (args.first(), args.get(1)) else {
        return Err(USAGE.into());
    };
    if flag != "--control" {
        return Err(USAGE.into());
    }
    let command = Command::parse(name).ok_or_else(|| format!("Unknown command. {USAGE}"))?;
    let mut directory = None;
    let mut confirm = false;
    let mut rest = args[2..].iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--directory" if directory.is_none() => match rest.next() {
                Some(path) if !path.is_empty() && !path.starts_with("--") => {
                    directory = Some(PathBuf::from(path))
                }
                _ => return Err(format!("--directory needs a path. {USAGE}")),
            },
            "--confirm" if !confirm && command == Command::Quit => confirm = true,
            _ => return Err(format!("Unexpected argument. {USAGE}")),
        }
    }
    Ok(Invocation {
        command,
        directory,
        confirm,
    })
}

/// Every bounded wait. Production values are documented; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    pub connect: Duration,
    pub authenticate: Duration,
    pub status: Duration,
    /// release, connect and the shutdown request: each waits for the broker's
    /// request lock exactly as the icon does, behind a running backup too
    /// (about 215 s at most). A request that outlives this is not cancelled:
    /// it stays queued in the broker and still runs once the lock frees.
    pub action: Duration,
    /// After the shutdown was acknowledged, for the process to end.
    pub quit_exit: Duration,
    /// For a started resident host to publish its descriptor.
    pub start: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(2),
            authenticate: Duration::from_secs(5),
            status: Duration::from_secs(5),
            // Past the longest request-lock hold (a backup, about 215 s), so
            // a backup alone never leaves an action queued after we give up.
            action: Duration::from_secs(240),
            quit_exit: Duration::from_secs(10),
            start: Duration::from_secs(15),
        }
    }
}

/// A started `--resident` process, polled for an early exit.
pub trait Spawned {
    fn pid(&self) -> u32;
    /// `Ok(None)` while it runs, `Ok(Some(code))` once it exited.
    fn exit_code(&mut self) -> Result<Option<Option<i32>>, String>;
}

/// Everything the commands need from the machine, replaceable in tests so no
/// test ever starts a process, reads the real profile or names a real pid.
pub trait Env {
    /// The caller's own app data directory: the only one accepted.
    fn own_directory(&self) -> Option<PathBuf>;
    /// `pid` is alive and was created at `created_at`. A missing creation
    /// time is never enough.
    fn alive(&self, pid: u32, created_at: Option<u64>) -> bool;
    /// The resident host's session marker (a named mutex) exists.
    fn session_marker_present(&self) -> bool;
    /// An interactive desktop: `--resident` serves nowhere else.
    fn interactive(&self) -> bool;
    /// `--resident --directory <dir> --keep-user-quit`: `start` cleared any
    /// recorded Quit at its own one point first.
    fn spawn_resident(&self, directory: &Path) -> Result<Box<dyn Spawned>, String>;
    fn client_pid(&self) -> u32;
    fn parent_pid(&self) -> Option<u32>;
    fn timeouts(&self) -> Timeouts;
}

/// What a command produced: the exit code and the one JSON object.
#[derive(Debug)]
pub struct Outcome {
    pub exit_code: i32,
    pub report: Value,
}

/// The binary's entry point for `--control`.
pub fn main(args: &[String], version: &str, default_directory: Option<fn() -> PathBuf>) -> ! {
    let outcome = match parse(args) {
        Ok(invocation) => run(&invocation, version, &NativeEnv { default_directory }),
        Err(message) => usage(&message, version),
    };
    // Never `println!`: a harness that closed its pipe early would turn the
    // report's failed write into a panic and exit 101. The exit code stands.
    let mut stdout = io::stdout().lock();
    let _ = writeln!(
        stdout,
        "{}",
        serde_json::to_string(&outcome.report).expect("a report serializes")
    );
    let _ = stdout.flush();
    std::process::exit(outcome.exit_code)
}

/// The report for arguments that do not parse: nothing ran.
pub fn usage(message: &str, version: &str) -> Outcome {
    Outcome {
        exit_code: exit::REFUSED,
        report: json!({
            "schema": SCHEMA,
            "command": null,
            "ok": false,
            "exitCode": exit::REFUSED,
            "outcome": "usage",
            "detail": message,
            "controlVersion": version,
        }),
    }
}

/// Run one parsed command against `env`.
pub fn run(invocation: &Invocation, version: &str, env: &dyn Env) -> Outcome {
    let command = invocation.command;
    let mut report = Map::new();
    report.insert("schema".into(), json!(SCHEMA));
    report.insert("command".into(), json!(command.name()));
    report.insert("controlVersion".into(), json!(version));
    let directory = match guard_directory(invocation.directory.as_deref(), env) {
        Ok(directory) => directory,
        Err((outcome, detail)) => {
            // Refused before any directory is touched, so nothing is logged.
            return finish(report, exit::REFUSED, outcome, &detail, None);
        }
    };
    if command == Command::Quit && !invocation.confirm {
        // Refused before anything is read, sent or written, the audit log
        // included: a refusal changes nothing, so it leaves no trace.
        return finish(
            report,
            exit::REFUSED,
            "confirmRequired",
            "quit ends the broker even with clients attached; pass --confirm",
            None,
        );
    }
    let (code, outcome, detail) = match command {
        Command::Status => status(&directory, env, &mut report),
        Command::Release => action(&directory, env, &mut report, Action::Release),
        Command::Connect => action(&directory, env, &mut report, Action::Connect),
        Command::Quit => quit(&directory, env, &mut report),
        Command::Start => start(&directory, env, &mut report),
    };
    let audited = command
        .changes_state()
        .then(|| audit(&directory, command, code, outcome, env));
    finish(report, code, outcome, &detail, audited)
}

fn finish(
    mut report: Map<String, Value>,
    code: i32,
    outcome: &str,
    detail: &str,
    audited: Option<bool>,
) -> Outcome {
    report.insert("ok".into(), json!(code == exit::OK));
    report.insert("exitCode".into(), json!(code));
    report.insert("outcome".into(), json!(outcome));
    report.insert("detail".into(), json!(detail));
    if let Some(audited) = audited {
        report.insert("auditLogged".into(), json!(audited));
    }
    Outcome {
        exit_code: code,
        report: Value::Object(report),
    }
}

type Step = (i32, &'static str, String);

/// The caller's own app data directory, and only it. `--directory` may name
/// it again (spelled differently) but never another one.
fn guard_directory(
    requested: Option<&Path>,
    env: &dyn Env,
) -> Result<PathBuf, (&'static str, String)> {
    let Some(own) = env.own_directory() else {
        return Err((
            "noAppDataDirectory",
            "this build has no app data directory; use the installed qc-device-broker.exe".into(),
        ));
    };
    if let Some(requested) = requested {
        if !same_directory(requested, &own) {
            return Err((
                "directoryRefused",
                "--directory must be this user's own QC Remote app data directory".into(),
            ));
        }
    }
    Ok(own)
}

/// Windows paths compare without case and without a trailing separator.
fn same_directory(left: &Path, right: &Path) -> bool {
    let normal = |path: &Path| {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    normal(left) == normal(right)
}

// ---------------------------------------------------------------------------
// The generated reader type redacts the bearer token in Debug and accepts
// optional fields from older broker generations.

enum Found {
    Absent,
    /// Present but not usable: not owner-only, or not a descriptor.
    Unusable(&'static str),
    /// Names a process that is gone, or a recycled pid.
    Stale(Descriptor, &'static str),
    Live(Descriptor),
}

fn descriptor_path(directory: &Path) -> PathBuf {
    directory.join(DESCRIPTOR_FILE)
}

fn find(directory: &Path, env: &dyn Env) -> Found {
    let path = descriptor_path(directory);
    let read = || match fs::read(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(_) => Some(None),
        Ok(bytes) => Some(serde_json::from_slice::<Descriptor>(&bytes).ok()),
    };
    let parsed = match read() {
        None => return Found::Absent,
        Some(Some(parsed)) => parsed,
        // A live broker atomically republishes the complete document. Retry
        // once to cover a transient filesystem read/replace failure.
        Some(None) => {
            std::thread::sleep(Duration::from_millis(100));
            match read() {
                None => return Found::Absent,
                Some(Some(parsed)) => parsed,
                Some(None) => return Found::Unusable("unreadable"),
            }
        }
    };
    if !qc_owner_only_file::is_owner_only(&path).unwrap_or(false) {
        return Found::Unusable("notOwnerOnly");
    }
    if parsed.process_id == 0 || parsed.process_created_at.is_none() {
        return Found::Unusable("unverifiable");
    }
    if env.alive(parsed.process_id, parsed.process_created_at) {
        Found::Live(parsed)
    } else {
        Found::Stale(parsed, "processGoneOrRecycled")
    }
}

/// A resident sets its session marker only after it publishes, so a marker
/// with no live descriptor here means the resident published where this
/// process cannot see it (a launcher's MSIX file-system virtualisation
/// redirects `%LOCALAPPDATA%` writes into its package's LocalCache, while the
/// named mutex is shared), or that it is exiting. Neither is "not running".
fn resident_not_visible(found: &Found, env: &dyn Env) -> bool {
    matches!(found, Found::Absent | Found::Stale(..)) && env.session_marker_present()
}

/// The descriptor as reported: never its token, port or instance id.
fn describe(found: &Found) -> Value {
    let row = |descriptor: &Descriptor, live: bool, reason: Option<&str>| {
        json!({
            "present": true,
            "live": live,
            "reason": reason,
            "processId": descriptor.process_id,
            "processCreatedAt": descriptor.process_created_at,
            "publishedAtUnixMs": descriptor.published_at_unix_ms,
            "gatewayApiVersion": descriptor.gateway_api_version,
        })
    };
    match found {
        Found::Absent => json!({"present": false, "live": false, "reason": null}),
        Found::Unusable(reason) => json!({"present": true, "live": false, "reason": reason}),
        Found::Stale(descriptor, reason) => row(descriptor, false, Some(reason)),
        Found::Live(descriptor) => row(descriptor, true, None),
    }
}

/// The descriptor's `.claim` sidecar. A live owner's is reported from the
/// descriptor alone; otherwise a shared lock is taken and dropped at once,
/// which reads whether a process still holds it. Nothing is ever deleted.
fn claim_state(descriptor: &Path, live_owner: bool) -> &'static str {
    let claim = qc_owner_only_file::claim_path(descriptor);
    if !claim.exists() {
        return "absent";
    }
    if live_owner {
        return "heldByLiveOwner";
    }
    match open_for_probe(&claim) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => "absent",
        Err(_) => "unknown",
        Ok(file) => match file.try_lock_shared() {
            Ok(()) => {
                let _ = file.unlock();
                "staleUnlocked"
            }
            Err(fs::TryLockError::WouldBlock) => "heldUnpublished",
            Err(fs::TryLockError::Error(_)) => "unknown",
        },
    }
}

fn open_for_probe(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE: never
        // conflicts with the holder's handle, which shares only reading.
        options.share_mode(0x7);
    }
    options.open(path)
}

/// Forced-release markers are kept per broker generation, deliberately.
fn release_markers(directory: &Path, instance: Option<&str>) -> Value {
    let mut count = 0_usize;
    let mut newest: Option<u64> = None;
    if let Ok(entries) = fs::read_dir(directory) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("device-broker.released-") && name.ends_with(".json") {
                count += 1;
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|elapsed| elapsed.as_millis() as u64);
                newest = newest.max(modified);
            }
        }
    }
    let current = instance.map(|instance| {
        crate::generated_broker_surface::BrokerReleaseMarker::path_for(
            &descriptor_path(directory),
            instance,
        )
        .exists()
    });
    json!({"count": count, "newestUnixMs": newest, "currentInstance": current})
}

// ---------------------------------------------------------------------------
// The wire: length-prefixed JSON frames, exactly as the broker reads them.

enum Failure {
    Timeout(String),
    Refused(String),
    /// The broker refused because a client other than its host is attached.
    ParticipantsAttached(String),
    Io(String),
    Unsupported,
}

impl Failure {
    fn step(self) -> Step {
        match self {
            Self::Timeout(detail) => (exit::TIMEOUT, "timeout", detail),
            Self::Refused(detail) => (exit::BROKER_ERROR, "brokerRefused", detail),
            Self::ParticipantsAttached(detail) => {
                (exit::BROKER_ERROR, "participantsAttached", detail)
            }
            Self::Io(detail) => (exit::BROKER_ERROR, "brokerUnreachable", detail),
            Self::Unsupported => (
                exit::BROKER_ERROR,
                "unsupportedBroker",
                "the running broker predates the control role: it took the authentication \
                 handshake as an ordinary client (resetting its idle clocks), and the \
                 connection was closed at once without sending a command"
                    .into(),
            ),
        }
    }

    /// The broker's error object, by its stable `data.code` when it has one.
    fn from_error(error: &Value) -> Option<Self> {
        let message = error["message"].as_str()?.to_owned();
        Some(
            if error["data"]["code"] == crate::rpc::CONTROL_PARTICIPANTS_ATTACHED {
                Self::ParticipantsAttached(message)
            } else {
                Self::Refused(message)
            },
        )
    }
}

fn io_failure(error: io::Error, doing: &str) -> Failure {
    if matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        Failure::Timeout(format!("timed out {doing}"))
    } else {
        Failure::Io(format!("{doing}: {error}"))
    }
}

struct Session {
    stream: TcpStream,
    next_id: u64,
}

impl Session {
    /// Connect to the live broker the descriptor names, authenticate in the
    /// control role, and confirm it is that very broker generation.
    fn open(descriptor: &Descriptor, timeouts: &Timeouts) -> Result<Self, Failure> {
        let address = SocketAddr::from(([127, 0, 0, 1], descriptor.port));
        let stream = TcpStream::connect_timeout(&address, timeouts.connect)
            .map_err(|error| io_failure(error, "connecting to the broker"))?;
        let _ = stream.set_nodelay(true);
        let mut session = Self { stream, next_id: 1 };
        let frame = json!({
            "method": "broker.authenticate",
            "params": {"token": descriptor.bearer_token.as_str(), "role": crate::rpc::CONTROL_ROLE},
            "id": 0,
        });
        let deadline = Instant::now() + timeouts.authenticate;
        session.write(&frame, deadline)?;
        let reply = session.read(deadline)?;
        if let Some(message) = reply["error"]["message"].as_str() {
            return Err(Failure::Refused(message.to_owned()));
        }
        if reply["result"]["role"] != crate::rpc::CONTROL_ROLE {
            // An older broker took it as a participant: leave at once.
            let _ = session.stream.shutdown(std::net::Shutdown::Both);
            return Err(Failure::Unsupported);
        }
        if reply["result"]["instanceId"] != descriptor.instance_id.as_str() {
            return Err(Failure::Refused(
                "the broker answering is not the one the descriptor names".into(),
            ));
        }
        Ok(session)
    }

    /// One request, answered within `within`. Notifications are skipped.
    fn call(&mut self, method: &str, params: Value, within: Duration) -> Result<Value, Failure> {
        let id = self.next_id;
        self.next_id += 1;
        let deadline = Instant::now() + within;
        self.write(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
            deadline,
        )?;
        loop {
            let frame = self.read(deadline)?;
            if frame.get("id") != Some(&json!(id)) {
                continue;
            }
            if let Some(failure) = Failure::from_error(&frame["error"]) {
                return Err(failure);
            }
            return Ok(frame["result"].clone());
        }
    }

    fn write(&mut self, value: &Value, deadline: Instant) -> Result<(), Failure> {
        let body = serde_json::to_vec(value).expect("a frame serializes");
        qc_broker_framing::write_tcp_frame_before_deadline(
            &mut self.stream,
            &body,
            MAX_FRAME_BYTES,
            deadline,
        )
        .map_err(|error| io_failure(error, "writing to the broker"))
    }

    fn read(&mut self, deadline: Instant) -> Result<Value, Failure> {
        let body = qc_broker_framing::read_tcp_frame_before_deadline(
            &mut self.stream,
            MAX_FRAME_BYTES,
            deadline,
        )
        .map_err(|error| io_failure(error, "reading from the broker"))?;
        serde_json::from_slice(&body)
            .map_err(|_| Failure::Io("the broker sent a frame that is not JSON".into()))
    }
}

// ---------------------------------------------------------------------------
// The commands.

/// The parts of a report every command shares: what is on disk and in the
/// session, read without touching the broker.
fn local_state(directory: &Path, env: &dyn Env, found: &Found) -> Value {
    let live = matches!(found, Found::Live(_));
    let instance = match found {
        Found::Live(descriptor) | Found::Stale(descriptor, _) => {
            Some(descriptor.instance_id.as_str())
        }
        _ => None,
    };
    json!({
        "descriptor": describe(found),
        "claim": claim_state(&descriptor_path(directory), live),
        "sessionMarker": env.session_marker_present(),
        "trayEnabled": crate::tray_settings::read_in(directory).tray_enabled,
        "releaseMarkers": release_markers(directory, instance),
    })
}

fn merge(report: &mut Map<String, Value>, value: Value) {
    if let Value::Object(fields) = value {
        report.extend(fields);
    }
}

fn status(directory: &Path, env: &dyn Env, report: &mut Map<String, Value>) -> Step {
    let found = find(directory, env);
    merge(report, local_state(directory, env, &found));
    let Found::Live(descriptor) = &found else {
        report.insert("running".into(), json!(false));
        report.insert("broker".into(), Value::Null);
        if resident_not_visible(&found, env) {
            return (
                exit::OK,
                "residentNotVisible",
                "the resident host's session marker exists but its descriptor is not visible here"
                    .into(),
            );
        }
        return (
            exit::OK,
            "notRunning",
            "no live broker serves this directory".into(),
        );
    };
    report.insert("running".into(), json!(true));
    report.insert("broker".into(), Value::Null);
    let timeouts = env.timeouts();
    let answer = Session::open(descriptor, &timeouts).and_then(|mut session| {
        session.call(
            crate::rpc::BROKER_CONTROL_STATUS_METHOD,
            json!({}),
            timeouts.status,
        )
    });
    match answer {
        Ok(mut broker) => {
            broker["processId"] = json!(descriptor.process_id);
            broker["processCreatedAt"] = json!(descriptor.process_created_at);
            report.insert("broker".into(), broker);
            (exit::OK, "running", "a live broker answered".into())
        }
        Err(failure) => failure.step(),
    }
}

#[derive(Clone, Copy)]
enum Action {
    Release,
    Connect,
}

/// The live descriptor, or the step that says why there is none. Nothing is
/// sent to a stale or unusable descriptor's process, ever.
fn require_live(
    directory: &Path,
    env: &dyn Env,
    report: &mut Map<String, Value>,
) -> Result<Descriptor, Step> {
    let found = find(directory, env);
    report.insert("descriptor".into(), describe(&found));
    match found {
        Found::Live(descriptor) => Ok(descriptor),
        Found::Absent => Err((
            exit::NOT_RUNNING,
            "notRunning",
            "no broker is running; nothing was sent".into(),
        )),
        Found::Stale(..) => Err((
            exit::NOT_RUNNING,
            "staleDescriptor",
            "the descriptor names a process that is gone or a recycled pid; nothing was sent"
                .into(),
        )),
        Found::Unusable(reason) => Err((
            exit::REFUSED,
            "descriptorRefused",
            format!("the descriptor cannot be trusted ({reason}); nothing was sent"),
        )),
    }
}

fn action(
    directory: &Path,
    env: &dyn Env,
    report: &mut Map<String, Value>,
    action: Action,
) -> Step {
    let descriptor = match require_live(directory, env, report) {
        Ok(descriptor) => descriptor,
        Err(step) => return step,
    };
    let (method, done) = match action {
        Action::Release => (crate::rpc::BROKER_CONTROL_RELEASE_METHOD, "released"),
        Action::Connect => (crate::rpc::BROKER_CONTROL_CONNECT_METHOD, "connecting"),
    };
    let timeouts = env.timeouts();
    let answer = Session::open(&descriptor, &timeouts)
        .and_then(|mut session| session.call(method, json!({}), timeouts.action));
    match answer {
        Ok(result) => {
            let detail = result["detail"].as_str().unwrap_or_default().to_owned();
            report.insert("result".into(), result);
            (exit::OK, done, detail)
        }
        Err(failure) => failure.step(),
    }
}

fn quit(directory: &Path, env: &dyn Env, report: &mut Map<String, Value>) -> Step {
    let descriptor = match require_live(directory, env, report) {
        Ok(descriptor) => descriptor,
        Err(step) => return step,
    };
    let timeouts = env.timeouts();
    let answer = Session::open(&descriptor, &timeouts).and_then(|mut session| {
        session.call("broker.shutdown", json!({"force": true}), timeouts.action)
    });
    if let Err(failure) = answer {
        return failure.step();
    }
    // Acknowledged: the release is committed. Wait for the process itself.
    let deadline = Instant::now() + timeouts.quit_exit;
    let mut exited = false;
    while Instant::now() < deadline {
        if !env.alive(descriptor.process_id, descriptor.process_created_at) {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let path = descriptor_path(directory);
    // A descriptor naming another, live process is a successor's, not ours.
    let successor = match find(directory, env) {
        Found::Live(next) if next.process_id != descriptor.process_id => Some(next.process_id),
        _ => None,
    };
    let descriptor_released = successor.is_some() || !path.exists();
    let claim = claim_state(&path, successor.is_some());
    let claim_released = claim == "absent" || successor.is_some();
    let marker_released = !env.session_marker_present();
    // The broker records the Quit for its own generation before committing
    // (`crate::user_quit`); an older broker does not.
    let user_quit_recorded = matches!(
        crate::user_quit::read(&path),
        Some(Ok(record)) if record.instance_id == descriptor.instance_id
    );
    let verified = json!({
        "processExited": exited,
        "descriptorReleased": descriptor_released,
        "claimReleased": claim_released,
        "claim": claim,
        "sessionMarkerReleased": marker_released,
        "releaseMarkerWritten": crate::generated_broker_surface::BrokerReleaseMarker::path_for(
            &path,
            &descriptor.instance_id,
        )
        .exists(),
        "userQuitRecorded": user_quit_recorded,
        "successorProcessId": successor,
    });
    report.insert("verified".into(), verified);
    if !exited {
        return (
            exit::TIMEOUT,
            "exitTimeout",
            "the broker acknowledged the forced release but had not exited in time".into(),
        );
    }
    if !(descriptor_released && claim_released) {
        return (
            exit::BROKER_ERROR,
            "notReleased",
            "the broker exited but left its descriptor or claim behind".into(),
        );
    }
    if !user_quit_recorded {
        // Released and exited, but nothing stands for the Quit: an AI
        // client's tool call or an open window may start a broker again.
        return (
            exit::BROKER_ERROR,
            "quitNotRecorded",
            "the broker exited but did not record the Quit; an AI client or an open window may \
             start it again"
                .into(),
        );
    }
    (
        exit::OK,
        "quit",
        "the device was released and the broker exited; only a user action or start brings it back"
            .into(),
    )
}

fn start(directory: &Path, env: &dyn Env, report: &mut Map<String, Value>) -> Step {
    let found = find(directory, env);
    let before = local_state(directory, env, &found);
    report.insert("before".into(), before.clone());
    if let Found::Live(descriptor) = &found {
        report.insert("started".into(), json!(false));
        report.insert("processId".into(), json!(descriptor.process_id));
        let cleared = match clear_user_quit(directory, report) {
            Ok(cleared) => cleared,
            Err(step) => return step,
        };
        return (
            exit::OK,
            "alreadyRunning",
            if cleared {
                "a live broker already serves; the recorded Quit was cleared".into()
            } else {
                "a live broker already serves; nothing was changed".into()
            },
        );
    }
    if let Found::Unusable(reason) = found {
        return (
            exit::REFUSED,
            "descriptorRefused",
            format!("the descriptor cannot be trusted ({reason}); nothing was started"),
        );
    }
    // Its election reads only this directory, so a second resident would
    // serve beside the hidden one.
    if resident_not_visible(&found, env) {
        return (
            exit::REFUSED,
            "residentNotVisible",
            "the resident host's session marker exists but its descriptor is not visible here; \
             nothing was started"
                .into(),
        );
    }
    if before["trayEnabled"] != true {
        return (
            exit::REFUSED,
            "trayDisabled",
            "the resident host is switched off in QC Remote's settings; --resident would not serve"
                .into(),
        );
    }
    if !env.interactive() {
        return (
            exit::REFUSED,
            "notInteractive",
            "no interactive desktop; --resident would not serve here".into(),
        );
    }
    // A stale unlocked claim (left by a killed broker) is reported and left
    // alone: the resident start's own election clears it under its lock.
    report.insert(
        "staleClaim".into(),
        json!(before["claim"] == "staleUnlocked"),
    );
    // This start's one clear point, under the election lock. The resident it
    // spawns runs with `--keep-user-quit` and never clears again: a Quit
    // recorded after this point (another start's broker, quit while this one
    // waits) stands, and that resident declines to serve under it.
    if let Err(step) = clear_user_quit(directory, report) {
        return step;
    }
    let mut child = match env.spawn_resident(directory) {
        Ok(child) => child,
        Err(detail) => return (exit::BROKER_ERROR, "spawnFailed", detail),
    };
    report.insert("spawnedProcessId".into(), json!(child.pid()));
    let deadline = Instant::now() + env.timeouts().start;
    let mut child_exit: Option<Option<i32>> = None;
    loop {
        if let Found::Live(descriptor) = find(directory, env) {
            report.insert("started".into(), json!(true));
            report.insert("processId".into(), json!(descriptor.process_id));
            report.insert(
                "servedBySpawned".into(),
                json!(descriptor.process_id == child.pid()),
            );
            return (exit::OK, "started", "a broker serves this directory".into());
        }
        if child_exit.is_none() {
            match child.exit_code() {
                Ok(Some(code)) if code != Some(0) => {
                    report.insert("started".into(), json!(false));
                    report.insert("spawnedExitCode".into(), json!(code));
                    return (
                        exit::BROKER_ERROR,
                        "residentStartFailed",
                        "the resident start exited with an error; see broker-stderr.log".into(),
                    );
                }
                Ok(exited) => child_exit = exited,
                Err(detail) => return (exit::BROKER_ERROR, "spawnFailed", detail),
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    report.insert("started".into(), json!(false));
    report.insert(
        "claim".into(),
        json!(claim_state(&descriptor_path(directory), false)),
    );
    if child_exit.is_some() {
        report.insert("spawnedExitCode".into(), json!(0));
        (
            exit::BROKER_ERROR,
            "residentStartDeclined",
            "the resident start exited without serving and no broker published".into(),
        )
    } else {
        (
            exit::TIMEOUT,
            "startTimeout",
            "the resident start did not publish in time".into(),
        )
    }
}

/// `start` is a user action: it supersedes a recorded Quit
/// (`crate::user_quit`). Reported as `userQuitCleared`; a record that cannot
/// be removed refuses the start rather than starting under it.
fn clear_user_quit(directory: &Path, report: &mut Map<String, Value>) -> Result<bool, Step> {
    match crate::user_quit::clear(&descriptor_path(directory)) {
        Ok(cleared) => {
            report.insert("userQuitCleared".into(), json!(cleared));
            Ok(cleared)
        }
        Err(detail) => Err((exit::BROKER_ERROR, "userQuitNotCleared", detail)),
    }
}

// ---------------------------------------------------------------------------
// The audit log.

/// One short line per state-changing command, in a single write. Best-effort:
/// concurrent appends from several processes can lose a line on Windows, and
/// a log over `AUDIT_LOG_MAX_BYTES` is truncated first. It never holds a
/// token: only the time, the command, its outcome and the caller's pids.
fn audit(directory: &Path, command: Command, code: i32, outcome: &str, env: &dyn Env) -> bool {
    let path = directory.join(AUDIT_LOG);
    let line = format!(
        "{}\n",
        json!({
            "atUtc": utc_timestamp(SystemTime::now()),
            "command": command.name(),
            "exitCode": code,
            "outcome": outcome,
            "clientPid": env.client_pid(),
            "parentPid": env.parent_pid(),
        })
    );
    if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > AUDIT_LOG_MAX_BYTES) {
        let _ = fs::remove_file(&path);
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()))
        .is_ok()
}

/// `YYYY-MM-DDTHH:MM:SSZ`, without a calendar crate.
fn utc_timestamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

// ---------------------------------------------------------------------------
// The machine.

struct NativeEnv {
    default_directory: Option<fn() -> PathBuf>,
}

/// The app writes its brokers' stderr here (apps/windows/src-tauri lib.rs
/// `broker_stderr_path`), bounded the same way.
fn broker_stderr_log() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|base| base.join("QC Remote").join("broker-stderr.log"))
}
const BROKER_STDERR_MAX_BYTES: u64 = 4 * 1024 * 1024;

struct NativeChild(std::process::Child);

impl Spawned for NativeChild {
    fn pid(&self) -> u32 {
        self.0.id()
    }
    fn exit_code(&mut self) -> Result<Option<Option<i32>>, String> {
        self.0
            .try_wait()
            .map(|status| status.map(|status| status.code()))
            .map_err(|error| format!("Check the resident start: {error}"))
    }
}

impl Env for NativeEnv {
    fn own_directory(&self) -> Option<PathBuf> {
        self.default_directory.map(|directory| directory())
    }

    fn alive(&self, pid: u32, created_at: Option<u64>) -> bool {
        created_at.is_some()
            && qc_windows_process::os_process_is_alive(pid)
            && qc_windows_process::os_process_created_at(pid) == created_at
    }

    fn session_marker_present(&self) -> bool {
        named_mutex_exists(SESSION_MARKER)
    }

    fn interactive(&self) -> bool {
        crate::tray::interactive_session()
    }

    /// As `NativeTrayEffects::spawn` in apps/windows/src-tauri: no window,
    /// stdin and stdout on NUL, stderr to the app's bounded broker log. Two
    /// things the app never needs, because it holds no inherited pipes:
    /// - this process's own standard handles (a harness's capture pipes) are
    ///   made non-inheritable first. `CreateProcessW` runs with
    ///   `bInheritHandles`, so otherwise the long-lived host would hold the
    ///   harness's pipe open and a harness waiting for EOF would hang until
    ///   the broker exits;
    /// - it asks to break away from the harness's job object, so a job that
    ///   kills on close does not take the host with it, and falls back to an
    ///   ordinary start when the job forbids breakaway. Its working
    ///   directory is the executable's, never the harness's.
    fn spawn_resident(&self, directory: &Path) -> Result<Box<dyn Spawned>, String> {
        let executable =
            std::env::current_exe().map_err(|error| format!("Locate this executable: {error}"))?;
        disinherit_std_handles();
        let start = |breakaway: bool| {
            let mut command = std::process::Command::new(&executable);
            command
                .arg("--resident")
                .arg("--directory")
                .arg(directory)
                .arg(crate::cli::KEEP_USER_QUIT_FLAG)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(stderr_sink());
            if let Some(home) = executable.parent() {
                command.current_dir(home);
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
                command.creation_flags(if breakaway {
                    CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB
                } else {
                    CREATE_NO_WINDOW
                });
            }
            #[cfg(not(windows))]
            let _ = breakaway;
            command.spawn()
        };
        start(true)
            .or_else(|_| start(false))
            .map(|child| Box::new(NativeChild(child)) as Box<dyn Spawned>)
            .map_err(|error| format!("Start the QC Remote background host: {error}"))
    }

    fn client_pid(&self) -> u32 {
        std::process::id()
    }

    fn parent_pid(&self) -> Option<u32> {
        crate::orphan_guard::parent_process_id()
    }

    fn timeouts(&self) -> Timeouts {
        Timeouts::default()
    }
}

fn stderr_sink() -> std::process::Stdio {
    let Some(path) = broker_stderr_log() else {
        return std::process::Stdio::null();
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > BROKER_STDERR_MAX_BYTES) {
        let _ = fs::remove_file(&path);
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map(std::process::Stdio::from)
        .unwrap_or_else(|_| std::process::Stdio::null())
}

/// Make this process's standard handles non-inheritable. They are still this
/// process's own to write the report to; only children no longer get them.
fn disinherit_std_handles() {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        for handle in [
            io::stdin().as_raw_handle(),
            io::stdout().as_raw_handle(),
            io::stderr().as_raw_handle(),
        ] {
            let _ = clear_inherit(handle);
        }
    }
}

/// Clear `HANDLE_FLAG_INHERIT` on one handle; `false` for no handle or a
/// handle that refused it.
#[cfg(windows)]
fn clear_inherit(handle: std::os::windows::io::RawHandle) -> bool {
    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
    if handle.is_null() || handle as isize == -1 {
        return false;
    }
    // SAFETY: only a flag of a handle this process holds is changed.
    unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) != 0 }
}

#[cfg(windows)]
fn named_mutex_exists(name: &str) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a terminated name read only during the call; the handle, if
    // any, is closed at once so the marker is never kept alive by this client.
    let handle = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide.as_ptr()) };
    if handle.is_null() {
        return false;
    }
    unsafe { CloseHandle(handle) };
    true
}

#[cfg(not(windows))]
fn named_mutex_exists(_name: &str) -> bool {
    false
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
