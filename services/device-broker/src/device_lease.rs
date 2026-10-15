//! Who currently holds the Quad Cortex, written down where the next opener can
//! read it - and consulted *before* the device is opened.
//!
//! The HID open does not reliably refuse a second opener: a live test on the
//! real unit had the app's broker open the device while a `--stdio` hardware
//! broker held it, and the two fought for ~45 s (rebuild loops, rejected
//! initialisations) while the lease sat unread, because it used to be consulted
//! only to word the status line *after* an open had already failed. So the
//! lease now gates the open:
//!
//! * [`LeaseHolder::acquire`] runs before every connect attempt. It creates the
//!   record atomically (a hard link of a fully written temp file, so a reader
//!   never sees half a record and two openers cannot both create it). When a
//!   live holder other than this process exists it returns that holder and the
//!   caller must not open the device; it reports the wait on its normal retry
//!   cadence and tries again, taking the unit over by itself once the holder
//!   exits or its record goes stale.
//! * A record is stale when its process is gone or its creation time differs
//!   (pid reuse). Only then may a non-owner replace it. A live holder's record
//!   is never overwritten or removed by anyone else - the previous `claim`
//!   overwrote it unconditionally after a contested connect, and the loser's
//!   later `release` then deleted what had become its own copy, which is how a
//!   live owner's record disappeared.
//! * An owner removes only its own record (pid and creation time match), on a
//!   disconnect or on its own clean shutdown (`Drop`). While connected it
//!   refreshes the record every [`REFRESH_INTERVAL`] and re-creates it if
//!   something deleted it, so the file's mtime shows a live owner and a crashed
//!   one is recognisable.

mod cleanup_lifetime;
use cleanup_lifetime::{CleanupLimiter, CleanupPermit};

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Holders of this process that currently hold the claim. One process can run
/// a worker per device while the record names the process, so the file is
/// removed only when the last of them lets go.
static HELD_IN_PROCESS: Mutex<Vec<(PathBuf, usize)>> = Mutex::new(Vec::new());
static LAST_LEASE_GENERATION_MS: AtomicU64 = AtomicU64::new(0);

fn next_generation_after(now: u64, previous: u64) -> u64 {
    now.max(previous.saturating_add(1))
}

/// A lease starts at wall-clock milliseconds, with a strictly increasing
/// process-local sequence when acquisitions share a millisecond or the clock
/// moves backwards. It remains an approximate acquisition timestamp; the
/// sequence component prevents delayed release jobs matching a newer lease.
fn next_lease_generation_ms() -> u64 {
    loop {
        let previous = LAST_LEASE_GENERATION_MS.load(Ordering::Relaxed);
        let next = next_generation_after(unix_ms(), previous);
        if LAST_LEASE_GENERATION_MS
            .compare_exchange_weak(previous, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return next;
        }
    }
}

/// Adjusts this process's holder count for `path` and returns the new count.
fn adjust_held(path: &Path, up: bool) -> usize {
    let mut held = HELD_IN_PROCESS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let index = match held.iter().position(|(held_path, _)| held_path == path) {
        Some(index) => index,
        None => {
            held.push((path.to_path_buf(), 0));
            held.len() - 1
        }
    };
    let count = &mut held[index].1;
    *count = if up {
        *count + 1
    } else {
        count.saturating_sub(1)
    };
    let now = *count;
    if now == 0 {
        held.swap_remove(index);
    }
    now
}

fn held_count(path: &Path) -> usize {
    HELD_IN_PROCESS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .find_map(|(held_path, count)| (held_path == path).then_some(*count))
        .unwrap_or(0)
}

/// How often a connected owner rewrites its record.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// Concurrent recovery must settle quickly without blocking reconnect forever.
const TRANSITION_LOCK_WAIT: Duration = Duration::from_secs(2);
const TRANSITION_LOCK_RETRY: Duration = Duration::from_millis(20);
const RELEASE_RETRY_LOCK_WAIT: Duration = Duration::from_millis(100);
const RELEASE_RETRY_BACKOFF: Duration = Duration::from_millis(50);

#[derive(Clone)]
struct PendingLeaseRelease {
    paths: Vec<PathBuf>,
    generation: LeaseRecord,
    _cleanup: std::sync::Arc<CleanupPermit>,
}

static PENDING_LEASE_RELEASES: Mutex<Vec<PendingLeaseRelease>> = Mutex::new(Vec::new());
static PENDING_LEASE_RELEASES_READY: Condvar = Condvar::new();
static LEASE_RELEASE_WORKER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

/// Ensure a single process-wide worker retries releases that could not take
/// the transition lock. The worker owns no device handles and exits with the
/// process; the normal stale-owner recovery handles that case.
fn ensure_release_worker() {
    if PENDING_LEASE_RELEASES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_empty()
    {
        return;
    }
    let completed = {
        let mut owner = LEASE_RELEASE_WORKER
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if owner
            .as_ref()
            .is_some_and(crate::platform::process::thread_signalled)
        {
            owner.take()
        } else {
            None
        }
    };
    if let Some(handle) = completed {
        let _ = handle.join();
    }
    let mut owner = LEASE_RELEASE_WORKER
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if owner.is_some() {
        return;
    }
    if let Ok(handle) = std::thread::Builder::new()
        .name("qc-lease-release".into())
        .spawn(retry_pending_lease_releases)
    {
        // One intentional process-lifetime manager. Started OS calls are not
        // forcibly cancellable; its exact original remains owned until exit.
        *owner = Some(handle);
    }
}

fn enqueue_lease_release(job: PendingLeaseRelease) {
    {
        let mut pending = PENDING_LEASE_RELEASES
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !pending.iter().any(|existing| same_release(existing, &job)) {
            pending.push(job);
        }
    }
    ensure_release_worker();
    PENDING_LEASE_RELEASES_READY.notify_one();
}

fn same_generation(record: &LeaseRecord, generation: &LeaseRecord) -> bool {
    record.process_id == generation.process_id
        && record.process_created_at == generation.process_created_at
        && record.since_unix_ms == generation.since_unix_ms
}

fn same_release(left: &PendingLeaseRelease, right: &PendingLeaseRelease) -> bool {
    left.paths == right.paths && same_generation(&left.generation, &right.generation)
}

/// Delete only this captured lease generation while its normal transition
/// locks are held. A later lease from this same process has a new `since`
/// value and therefore cannot be removed by delayed cleanup.
fn remove_release_generation(job: &PendingLeaseRelease) -> bool {
    let Ok(_transition) = LeaseTransitionGuard::acquire(&job.paths, RELEASE_RETRY_LOCK_WAIT) else {
        return false;
    };
    remove_release_generation_locked(job)
}

fn remove_release_generation_locked(job: &PendingLeaseRelease) -> bool {
    job.paths.iter().all(|path| match read_record(path) {
        Some(record) if same_generation(&record, &job.generation) => {
            if std::fs::remove_file(path).is_err() {
                return !path.try_exists().unwrap_or(true);
            }
            !path.try_exists().unwrap_or(true)
        }
        Some(_) => true,
        None => path.try_exists().map(|exists| !exists).unwrap_or(false),
    })
}

fn attempt_release(
    job: PendingLeaseRelease,
    remove: impl FnOnce(&PendingLeaseRelease) -> bool,
) -> Option<PendingLeaseRelease> {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| remove(&job))).unwrap_or(false) {
        None
    } else {
        Some(job)
    }
}

fn retry_pending_lease_releases() {
    loop {
        let job = {
            let mut pending = PENDING_LEASE_RELEASES
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            while pending.is_empty() {
                pending = PENDING_LEASE_RELEASES_READY
                    .wait(pending)
                    .unwrap_or_else(|poison| poison.into_inner());
            }
            pending.remove(0)
        };
        let Some(job) = attempt_release(job, remove_release_generation) else {
            continue;
        };
        let mut pending = PENDING_LEASE_RELEASES
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !pending.iter().any(|existing| same_release(existing, &job)) {
            pending.push(job);
        }
        let _ = PENDING_LEASE_RELEASES_READY
            .wait_timeout(pending, RELEASE_RETRY_BACKOFF)
            .unwrap_or_else(|poison| poison.into_inner());
    }
}

/// Serializes every mutation of a primary lease and its mirrors across broker
/// processes. The lock document is separate from the lease record; its
/// `.claim` sidecar is held by the OS and can be reaped after a process crash.
struct LeaseTransitionGuard(Vec<qc_owner_only_file::ClaimedFile>);

impl LeaseTransitionGuard {
    fn acquire(lease_paths: &[PathBuf], budget: Duration) -> std::io::Result<Self> {
        let mut lock_paths: Vec<PathBuf> = lease_paths
            .iter()
            .map(|lease_path| lease_path.with_extension("transition"))
            .collect();
        lock_paths.sort_by_key(|path| transition_path_key(path));
        lock_paths.dedup_by(|left, right| transition_path_key(left) == transition_path_key(right));
        let deadline = Instant::now() + budget;
        let mut claims = Vec::with_capacity(lock_paths.len());
        for lock_path in lock_paths {
            if let Some(parent) = lock_path.parent() {
                #[cfg(target_os = "linux")]
                crate::platform::paths::prepare_directory(parent).map_err(std::io::Error::other)?;
                #[cfg(target_os = "macos")]
                crate::platform::paths_macos::prepare_directory(parent)
                    .map_err(std::io::Error::other)?;
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                std::fs::create_dir_all(parent)?;
            }
            loop {
                match qc_owner_only_file::ClaimedFile::try_claim(&lock_path) {
                    Ok(claim) => {
                        claims.push(claim);
                        break;
                    }
                    Err(_) => {
                        // ClaimedFile uses create-new for its sidecar. If the
                        // previous owner crashed, its OS lock is gone but the
                        // names remain; release_if_unclaimed safely removes only
                        // that abandoned generation while refusing a live owner.
                        let _ = qc_owner_only_file::release_if_unclaimed(&lock_path);
                    }
                }
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Timed out waiting for the device lease transition lock",
                    ));
                }
                std::thread::sleep(
                    TRANSITION_LOCK_RETRY.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
        Ok(Self(claims))
    }
}

impl Drop for LeaseTransitionGuard {
    fn drop(&mut self) {
        while let Some(claim) = self.0.pop() {
            let _ = claim.release();
        }
    }
}

fn transition_path_key(path: &Path) -> String {
    let value = path.to_string_lossy().to_string();
    #[cfg(windows)]
    {
        value.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        value
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseRecord {
    pub process_id: u32,
    pub process_created_at: Option<u64>,
    pub role: String,
    /// Approximate acquisition time and generation: process-local allocation
    /// advances by 1 ms when wall-clock reads collide or move backwards.
    pub since_unix_ms: u64,
    /// Rewritten by the owner every [`REFRESH_INTERVAL`]; diagnostic only.
    /// Liveness is always the process, never this timestamp, because a worker
    /// blocked inside a long handshake legitimately stops refreshing.
    #[serde(default)]
    pub refreshed_unix_ms: u64,
}

const APP_DIRECTORY: &str = "org.freevia.qcremote";
const RECORD_NAME: &str = "device-owner.json";
const SHARED_DIRECTORY: &str = "QC Remote";
/// The broker whose `--stdio` instances a scan treats as device holders.
#[cfg(windows)]
const BROKER_IMAGE: &str = "qc-device-broker.exe";
#[cfg(any(target_os = "linux", target_os = "macos"))]
const BROKER_IMAGE: &str = "qc-device-broker";

/// `QC_DEVICE_LEASE_PATH` overrides, for tests and unusual installs.
pub fn default_path() -> PathBuf {
    if let Some(path) = std::env::var_os("QC_DEVICE_LEASE_PATH") {
        return PathBuf::from(path);
    }
    Locations::from_environment().primary
}

/// Every place an owner record can live.
///
/// A process launched from a packaged (MSIX) host such as Codex is run under
/// file-system virtualisation: its writes under `%LOCALAPPDATA%` are
/// redirected into `%LOCALAPPDATA%\Packages\<family>\LocalCache\Local\...`
/// and an unpackaged process never sees them, while the packaged one still
/// reads the real directory. A record kept only there is therefore visible
/// one way round - which is how an app started from a normal shell hard-killed
/// a hardware run launched from Codex. Virtualisation does not apply under
/// `%ProgramData%`, so the record is *written* there (the one place both kinds
/// of process resolve identically), mirrored best-effort into the legacy
/// `%LOCALAPPDATA%` directory for older builds, and *read* from all of them
/// plus every package's LocalCache copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locations {
    /// Authoritative record, shared by packaged and unpackaged processes.
    pub primary: PathBuf,
    /// Written alongside the primary and removed with it (own record only).
    pub mirrors: Vec<PathBuf>,
    /// Read-only: a live other owner recorded here is authoritative too.
    pub peers: Vec<PathBuf>,
}

impl Locations {
    pub fn from_environment() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self {
                primary: crate::platform::paths::default_directory().join(RECORD_NAME),
                mirrors: Vec::new(),
                peers: Vec::new(),
            }
        }
        #[cfg(target_os = "macos")]
        {
            Self {
                primary: crate::platform::paths_macos::default_directory().join(RECORD_NAME),
                mirrors: Vec::new(),
                peers: Vec::new(),
            }
        }
        #[cfg(windows)]
        Self::resolve(
            std::env::var_os("PROGRAMDATA")
                .map(PathBuf::from)
                .as_deref(),
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .as_deref(),
        )
    }

    /// Pure resolution from the two roots (no file is created).
    pub fn resolve(program_data: Option<&Path>, local_app_data: Option<&Path>) -> Self {
        let legacy = local_app_data
            .map(|root| root.join(APP_DIRECTORY).join(RECORD_NAME))
            .unwrap_or_else(|| std::env::temp_dir().join(APP_DIRECTORY).join(RECORD_NAME));
        let shared = program_data.map(|root| root.join(SHARED_DIRECTORY).join(RECORD_NAME));
        match shared {
            Some(primary) => Self {
                primary,
                mirrors: vec![legacy],
                peers: package_copies(local_app_data),
            },
            // No ProgramData (never on Windows): the legacy file is primary.
            None => Self {
                primary: legacy,
                mirrors: Vec::new(),
                peers: package_copies(local_app_data),
            },
        }
    }
}

/// The virtualised copy of the record under every installed package's
/// LocalCache that has one.
fn package_copies(local_app_data: Option<&Path>) -> Vec<PathBuf> {
    let Some(root) = local_app_data else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root.join("Packages")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| {
            entry
                .path()
                .join("LocalCache")
                .join("Local")
                .join(APP_DIRECTORY)
                .join(RECORD_NAME)
        })
        .filter(|path| path.is_file())
        .collect()
}

/// A process found by the fallback scan, reduced to what the decision needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedProcess {
    pub pid: u32,
    pub created_at: Option<u64>,
    pub command_line: Option<String>,
}

/// Whether `command_line` names a `--stdio` broker: a whole `--stdio`
/// argument, not a substring of some other flag or path.
fn is_stdio_command_line(command_line: &str) -> bool {
    command_line
        .split_whitespace()
        .any(|argument| argument.trim_matches('"') == "--stdio")
}

/// Pure fallback decision: the first `qc-device-broker` process, other than
/// `own_pid`, that is demonstrably a `--stdio` run. Conservative by
/// construction: a process whose command line or creation time could not be
/// read is never a match, and an `own_is_stdio` caller never defers to a scan
/// (two hardware runs would each wait for the other forever).
pub fn stdio_owner_from_scan(
    processes: &[ScannedProcess],
    own_pid: u32,
    own_is_stdio: bool,
    is_alive: impl Fn(u32) -> bool,
) -> Option<LeaseRecord> {
    if own_is_stdio {
        return None;
    }
    processes
        .iter()
        .filter(|process| process.pid != own_pid && process.created_at.is_some())
        .filter(|process| {
            process
                .command_line
                .as_deref()
                .is_some_and(is_stdio_command_line)
        })
        .find(|process| is_alive(process.pid))
        .map(|process| LeaseRecord {
            process_id: process.pid,
            process_created_at: process.created_at,
            role: "a hardware or conformance run (device broker --stdio)".into(),
            since_unix_ms: 0,
            refreshed_unix_ms: 0,
        })
}

fn scan_for_stdio_owner() -> Option<LeaseRecord> {
    if std::env::var_os("QC_DEVICE_LEASE_SCAN").is_some_and(|value| value == "0") {
        return None;
    }
    let processes: Vec<ScannedProcess> = crate::platform::process::processes_by_image(BROKER_IMAGE)
        .into_iter()
        .map(|process| ScannedProcess {
            pid: process.pid,
            created_at: process.created_at,
            command_line: process.command_line,
        })
        .collect();
    stdio_owner_from_scan(
        &processes,
        std::process::id(),
        std::env::args().any(|argument| argument == "--stdio"),
        crate::platform::process::os_process_is_alive,
    )
}

/// Creates the shared directory and lets every local user's processes use it:
/// a directory first created by an elevated process is owned by
/// Administrators and, under the default `%ProgramData%` ACL, closed to the
/// ordinary token of the same person. Best effort; failing leaves the mirror
/// in the legacy directory to carry the record.
#[cfg(windows)]
fn prepare_shared_directory(directory: &Path) {
    if directory.is_dir() {
        return;
    }
    if std::fs::create_dir_all(directory).is_err() {
        return;
    }
    #[cfg(windows)]
    {
        let _ = grant_shared_directory_with(directory, |command| {
            qc_native_platform::process::owned_tool::run(command, Duration::from_secs(3))
        });
    }
}

#[cfg(windows)]
fn grant_shared_directory_with(
    directory: &Path,
    run: impl FnOnce(&mut std::process::Command) -> std::io::Result<std::process::Output>,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    }
    let mut buffer = vec![0_u16; 32_768];
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(std::io::Error::other(
            "Could not resolve trusted Windows system directory",
        ));
    }
    buffer.truncate(length);
    let system = PathBuf::from(std::ffi::OsString::from_wide(&buffer));
    if !system.is_absolute() {
        return Err(std::io::Error::other(
            "Windows system directory is not absolute",
        ));
    }
    let mut command = std::process::Command::new(system.join("icacls.exe"));
    // BUILTIN\Users (S-1-5-32-545): modify, inherited by files. No PATH/current-dir search.
    command
        .arg(directory)
        .args(["/grant", "*S-1-5-32-545:(OI)(CI)M"])
        .stderr(std::process::Stdio::piped());
    let output = run(&mut command)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("Shared directory ACL tool failed"))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn prepare_shared_directory(directory: &Path) {
    #[cfg(target_os = "linux")]
    let _ = crate::platform::paths::prepare_directory(directory);
    #[cfg(target_os = "macos")]
    let _ = crate::platform::paths_macos::prepare_directory(directory);
}

/// What this process is, in the words a user would recognise.
pub fn own_role() -> String {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|argument| argument == "--stdio") {
        "a hardware or conformance run (device broker --stdio)".into()
    } else if args.iter().any(|argument| argument == "--broker-serve") {
        "QC Remote's shared device broker".into()
    } else {
        "another device broker".into()
    }
}

fn unix_ms() -> u64 {
    quadcortex::time::unix_time_ms()
}

fn own_record(role: &str, since_unix_ms: u64) -> LeaseRecord {
    LeaseRecord {
        process_id: std::process::id(),
        process_created_at: crate::platform::process::os_process_created_at(std::process::id()),
        role: role.to_owned(),
        since_unix_ms,
        refreshed_unix_ms: unix_ms(),
    }
}

/// A fail-closed deferral when the lease transition could not be serialized or
/// settled. PID zero marks this as a coordination condition, not a named owner.
fn transition_wait_record() -> LeaseRecord {
    LeaseRecord {
        process_id: 0,
        process_created_at: None,
        role: "another device broker is updating the ownership record".into(),
        since_unix_ms: unix_ms(),
        refreshed_unix_ms: unix_ms(),
    }
}

const UNAVAILABLE_CLAIM_ROLE: &str = "existing ownership claim could not be read or validated";
const CLEANUP_CAPACITY_ROLE: &str = "native lease cleanup lifetime capacity exhausted";
fn unavailable_claim_record() -> LeaseRecord {
    let mut record = transition_wait_record();
    record.role = UNAVAILABLE_CLAIM_ROLE.into();
    record
}
fn cleanup_capacity_record() -> LeaseRecord {
    let mut record = transition_wait_record();
    record.role = CLEANUP_CAPACITY_ROLE.into();
    record
}

/// Whether `record` names this very process (same pid and creation time).
fn is_own(record: &LeaseRecord) -> bool {
    record.process_id == std::process::id()
        && match (
            record.process_created_at,
            crate::platform::process::os_process_created_at(record.process_id),
        ) {
            (Some(recorded), Some(current)) => recorded == current,
            // A Windows lookup that succeeds cannot authenticate an absent
            // recorded creation time. Keep Linux/OS-unknown compatibility;
            // the shared record is still advisory metadata, not authentication.
            (None, Some(_)) if cfg!(windows) => false,
            _ => true,
        }
}

/// Whether the process a record names is still the one that wrote it.
fn holder_is_live(record: &LeaseRecord) -> bool {
    if !crate::platform::process::os_process_is_alive(record.process_id) {
        return false;
    }
    match (
        record.process_created_at,
        crate::platform::process::os_process_created_at(record.process_id),
    ) {
        (Some(recorded), Some(current)) => recorded == current,
        _ => true,
    }
}

// Software diagnostic-record budget, not a firmware or device limit. The shared
// lease directory is writable by other users, so metadata length is not a read
// bound: a record may grow after open or while it is being consumed.
const MAX_LEASE_RECORD_BYTES: usize = 64 * 1024;

fn read_record_from(reader: impl std::io::Read) -> std::io::Result<LeaseRecord> {
    use std::io::Read;
    let mut bytes = Vec::new();
    reader
        .take((MAX_LEASE_RECORD_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_LEASE_RECORD_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Device lease record exceeds its byte budget",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn read_record(path: &Path) -> Option<LeaseRecord> {
    read_record_from(std::fs::File::open(path).ok()?).ok()
}

fn encode_record(record: &LeaseRecord) -> std::io::Result<Vec<u8>> {
    struct BoundedRecord(Vec<u8>);
    impl std::io::Write for BoundedRecord {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_LEASE_RECORD_BYTES.saturating_sub(self.0.len()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Device lease record exceeds its byte budget",
                ));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = BoundedRecord(Vec::new());
    serde_json::to_writer(&mut writer, record).map_err(|error| {
        std::io::Error::new(
            error
                .io_error_kind()
                .unwrap_or(std::io::ErrorKind::InvalidData),
            error,
        )
    })?;
    Ok(writer.0)
}

/// The lease of a live process other than `own_pid`, if any.
pub fn read_live_other(path: &Path, own_pid: u32) -> Option<LeaseRecord> {
    let record = read_record(path)?;
    if record.process_id == own_pid || !holder_is_live(&record) {
        return None;
    }
    Some(record)
}

/// Creates `path` holding `record`, failing if it already exists. The content
/// is written to a private temp file first and linked into place, so the file
/// is never visible half written.
fn create_exclusive(path: &Path, record: &LeaseRecord) -> std::io::Result<()> {
    let bytes = encode_record(record)?;
    if let Some(parent) = path.parent() {
        #[cfg(target_os = "linux")]
        crate::platform::paths::prepare_directory(parent).map_err(std::io::Error::other)?;
        #[cfg(target_os = "macos")]
        crate::platform::paths_macos::prepare_directory(parent).map_err(std::io::Error::other)?;
        #[cfg(windows)]
        let _ = std::fs::create_dir_all(parent);
    }
    let mut stage = LeaseStage::reserve(path)?;
    stage.write_with(&bytes, std::io::Write::write_all)?;
    let linked = std::fs::hard_link(&stage.path, path);
    match linked {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(error),
        Err(_) => {
            // A volume without hard links: fall back to an exclusive create.
            let mut target = LeaseStage::at(path.to_owned())?;
            target.write_with(&bytes, std::io::Write::write_all)?;
            target.armed = false;
            Ok(())
        }
    }
}

/// Replaces the record with this process's own, atomically.
fn replace_own(path: &Path, record: &LeaseRecord) -> std::io::Result<()> {
    let bytes = encode_record(record)?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        qc_owner_only_file::publish_atomically(path, &bytes).map_err(std::io::Error::other)
    }
    #[cfg(windows)]
    {
        let mut stage = LeaseStage::reserve(path)?;
        stage.write_with(&bytes, std::io::Write::write_all)?;
        std::fs::rename(&stage.path, path)?;
        stage.armed = false; // Ownership transferred; never delete a new former-path occupant.
        Ok(())
    }
}

/// A fresh diagnostic stage, acquired before the first byte can be written.
struct LeaseStage {
    path: PathBuf,
    file: std::fs::File,
    armed: bool,
}
impl LeaseStage {
    fn reserve(destination: &Path) -> std::io::Result<Self> {
        for _ in 0..64 {
            let path = destination.with_extension(format!(
                "json.{}.{}.tmp",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            match Self::at(path) {
                Ok(stage) => return Ok(stage),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::other(
            "Could not reserve fresh device lease stage",
        ))
    }
    fn at(path: PathBuf) -> std::io::Result<Self> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::{
                Foundation::GENERIC_WRITE,
                Storage::FileSystem::{DELETE, FILE_SHARE_DELETE, FILE_SHARE_READ},
            };
            options
                .access_mode(GENERIC_WRITE | DELETE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let file = options.open(&path)?;
        Ok(Self {
            path,
            file,
            armed: true,
        })
    }
    fn write_with(
        &mut self,
        bytes: &[u8],
        write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        write(&mut self.file, bytes)?;
        self.file.sync_all()
    }
}
impl Drop for LeaseStage {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
            };
            let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
            unsafe {
                SetFileInformationByHandle(
                    self.file.as_raw_handle(),
                    FileDispositionInfo,
                    (&raw const disposition).cast(),
                    std::mem::size_of_val(&disposition) as u32,
                );
            }
        }
        #[cfg(not(windows))]
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// One process's claim on the device. Dropping it releases the claim, which is
/// the owner's clean-shutdown path.
pub struct LeaseHolder {
    path: PathBuf,
    role: String,
    held: bool,
    since_unix_ms: u64,
    last_refresh: Instant,
    /// Other copies of the record written and removed together with the primary.
    mirrors: Vec<PathBuf>,
    /// Other places a live owner may have recorded itself (read-only).
    peers: Vec<PathBuf>,
    /// Whether the process-scan fallback applies.
    scan: bool,
    /// The shared directory to prepare before the first write.
    shared_directory: Option<PathBuf>,
    cleanup_limiter: std::sync::Arc<CleanupLimiter>,
    cleanup_permit: Option<std::sync::Arc<CleanupPermit>>,
    #[cfg(test)]
    transition_test_barrier: Option<PathBuf>,
}

impl LeaseHolder {
    pub fn new(path: PathBuf, role: String) -> Self {
        Self {
            path,
            role,
            held: false,
            since_unix_ms: 0,
            last_refresh: Instant::now(),
            mirrors: Vec::new(),
            peers: Vec::new(),
            scan: false,
            shared_directory: None,
            cleanup_limiter: CleanupLimiter::production(),
            cleanup_permit: None,
            #[cfg(test)]
            transition_test_barrier: None,
        }
    }

    /// A holder over every location of the shared policy, with the process
    /// scan as the last resort. Production workers use this when no explicit
    /// path was given; tests use [`Self::new`] and stay on their own files.
    pub fn with_locations(locations: Locations, role: String, scan: bool) -> Self {
        let mut holder = Self::new(locations.primary.clone(), role);
        if !locations.mirrors.is_empty() {
            holder.shared_directory = locations.primary.parent().map(Path::to_path_buf);
        }
        holder.mirrors = locations.mirrors;
        holder.peers = locations.peers;
        holder.scan = scan;
        holder
    }

    /// A live owner recorded anywhere other than the primary, or found by the
    /// scan. Never our own pid.
    pub fn foreign_holder(&self) -> Option<LeaseRecord> {
        let own = std::process::id();
        self.mirrors
            .iter()
            .chain(self.peers.iter())
            .find_map(|path| read_live_other(path, own))
            .or_else(|| self.scan.then(scan_for_stdio_owner).flatten())
    }

    /// The status line for a failed open, naming any owner we can see.
    pub fn busy_detail(&self) -> String {
        match read_live_other(&self.path, std::process::id()).or_else(|| self.foreign_holder()) {
            Some(holder) => waiting_detail(&holder),
            None => busy_detail(&self.path),
        }
    }

    // Peers are authoritative observations, never write/lock targets. This
    // guards the known location set; it cannot serialize noncooperating old
    // writers or hostile path changes after the final ownership observation.
    fn unknown_recorded_claim(&self) -> Option<LeaseRecord> {
        self.mirrors
            .iter()
            .chain(self.peers.iter())
            .find_map(|path| {
                (read_record(path).is_none() && path.try_exists().unwrap_or(true))
                    .then(unavailable_claim_record)
            })
    }

    fn write_mirrors(&self) {
        for mirror in &self.mirrors {
            match read_record(mirror) {
                Some(existing) if is_own(&existing) => {
                    let _ = replace_own(mirror, &own_record(&self.role, self.since_unix_ms));
                }
                Some(existing) if holder_is_live(&existing) => {}
                Some(stale) => {
                    // Attribute stale recovery, just as for the primary. An
                    // older broker may use this mirror as its primary claim.
                    if read_record(mirror).as_ref() == Some(&stale) {
                        let _ = std::fs::remove_file(mirror);
                        let _ =
                            create_exclusive(mirror, &own_record(&self.role, self.since_unix_ms));
                    }
                }
                None => {
                    // Unknown existing bytes or denied metadata are not proof
                    // that the old owner's native session has retired.
                    if matches!(mirror.try_exists(), Ok(false)) {
                        let _ =
                            create_exclusive(mirror, &own_record(&self.role, self.since_unix_ms));
                    }
                }
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Lock every path this holder may mutate. A new broker's shared primary
    /// and legacy mirror must contend with an older broker that still treats
    /// that legacy file as its primary lease.
    fn transition_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.path.clone()];
        paths.extend(self.mirrors.iter().cloned());
        paths
    }

    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Retain this already-acquired generation while a native HID handle may
    /// outlive its worker. This never acquires or substitutes a new lease.
    pub fn io_owner(&self) -> Result<std::sync::Arc<dyn Send + Sync>, LeaseRecord> {
        if !self.held {
            return Err(transition_wait_record());
        }
        let Some(cleanup) = self
            .cleanup_permit
            .as_ref()
            .filter(|permit| permit.covers_paths(&self.transition_paths()))
        else {
            return Err(transition_wait_record());
        };
        let mut retained = Self::new(self.path.clone(), self.role.clone());
        retained.cleanup_limiter = std::sync::Arc::clone(&self.cleanup_limiter);
        retained.cleanup_permit = Some(std::sync::Arc::clone(cleanup));
        retained.mirrors = self.mirrors.clone();
        retained.since_unix_ms = self.since_unix_ms;
        retained.held = true;
        adjust_held(&retained.path, true);
        Ok(std::sync::Arc::new(retained))
    }

    /// Takes the claim before the device is opened. `Err(holder)` names a live
    /// process that owns the unit; the caller must not open it.
    pub fn acquire(&mut self) -> Result<(), LeaseRecord> {
        self.acquire_with_replacer(replace_own)
    }

    fn acquire_with_replacer(
        &mut self,
        replace: impl FnMut(&Path, &LeaseRecord) -> std::io::Result<()>,
    ) -> Result<(), LeaseRecord> {
        let previous_generation = self.since_unix_ms;
        let result = self.acquire_with_cleanup_admission(replace);
        if result.is_err() && self.held {
            self.since_unix_ms = previous_generation;
        }
        result
    }

    fn acquire_with_cleanup_admission(
        &mut self,
        mut replace: impl FnMut(&Path, &LeaseRecord) -> std::io::Result<()>,
    ) -> Result<(), LeaseRecord> {
        let reservation = if self.held {
            None
        } else {
            Some(
                self.cleanup_limiter
                    .reserve(self.transition_paths())
                    .map_err(|_| {
                        ensure_release_worker();
                        cleanup_capacity_record()
                    })?,
            )
        };
        if let Some(directory) = self.shared_directory.take() {
            prepare_shared_directory(&directory);
        }
        if let Some(refused) = self.unknown_recorded_claim() {
            return Err(refused);
        }
        if let Some(holder) = self.foreign_holder() {
            return Err(holder);
        }
        #[cfg(test)]
        if let Some(barrier) = &self.transition_test_barrier {
            wait_for_transition_test_peers(barrier);
        }
        let transition_paths = self.transition_paths();
        let _transition = LeaseTransitionGuard::acquire(&transition_paths, TRANSITION_LOCK_WAIT)
            .map_err(|_| transition_wait_record())?;
        // The first scan is an inexpensive early exit. Repeat it after taking
        // the inter-process guard so two stale recoveries cannot both pass a
        // pre-lock scan and open the HID device.
        if let Some(refused) = self.unknown_recorded_claim() {
            return Err(refused);
        }
        if let Some(holder) = self.foreign_holder() {
            return Err(holder);
        }
        self.since_unix_ms = next_lease_generation_ms();
        let record = own_record(&self.role, self.since_unix_ms);
        // Bounded: each pass either settles or removes one stale record.
        for _ in 0..4 {
            match read_record(&self.path) {
                Some(existing) if is_own(&existing) => {
                    // Share the active generation when another holder in this
                    // process already owns this path. With no local holder,
                    // this may be an old deferred release, so keep our newly
                    // allocated generation to protect it from delayed cleanup.
                    if held_count(&self.path) > 0 {
                        self.since_unix_ms = existing.since_unix_ms;
                    }
                    let current = own_record(&self.role, self.since_unix_ms);
                    replace(&self.path, &current).map_err(|_| transition_wait_record())?;
                    if !read_record(&self.path)
                        .is_some_and(|landed| same_generation(&landed, &current))
                    {
                        return Err(transition_wait_record());
                    }
                    self.mark_held(reservation);
                    return Ok(());
                }
                Some(existing) if holder_is_live(&existing) => return Err(existing),
                Some(stale) => {
                    // Dead pid or reused pid. Remove only if it is still the
                    // same stale record, so a successor's fresh one survives.
                    if read_record(&self.path).as_ref() == Some(&stale) {
                        let _ = std::fs::remove_file(&self.path);
                    }
                }
                None if self.path.exists() => {
                    // Corruption or temporary read denial is not evidence that
                    // a recorded owner's native session is gone. Never replace
                    // an existing claim without an attributable stale owner.
                    return Err(unavailable_claim_record());
                }
                None => {}
            }
            if create_exclusive(&self.path, &record).is_ok() {
                // Verify we are the one that landed (a racing opener's remove
                // can interleave with ours).
                match read_record(&self.path) {
                    Some(landed) if same_generation(&landed, &record) => {
                        self.mark_held(reservation);
                        return Ok(());
                    }
                    Some(landed) if holder_is_live(&landed) => return Err(landed),
                    _ => {}
                }
            }
        }
        // Could not settle the record (permissions or malformed concurrent
        // state). Never open the device without a recorded, serialized claim.
        match read_live_other(&self.path, std::process::id()) {
            Some(holder) => Err(holder),
            None => Err(transition_wait_record()),
        }
    }

    fn mark_held(&mut self, cleanup: Option<std::sync::Arc<CleanupPermit>>) {
        if !self.held {
            self.cleanup_permit = Some(cleanup.expect("lease cleanup admission precedes effects"));
            self.held = true;
            adjust_held(&self.path, true);
        }
        self.last_refresh = Instant::now();
        self.write_mirrors();
    }

    /// Called while connected: keeps the record fresh, and puts it back if
    /// something removed it. Never touches a live other holder's record.
    pub fn tick(&mut self) {
        if !self.held || self.last_refresh.elapsed() < REFRESH_INTERVAL {
            return;
        }
        let transition_paths = self.transition_paths();
        let Ok(_transition) =
            LeaseTransitionGuard::acquire(&transition_paths, Duration::from_millis(100))
        else {
            return;
        };
        self.last_refresh = Instant::now();
        match read_record(&self.path) {
            Some(existing) if is_own(&existing) => {
                let _ = replace_own(&self.path, &own_record(&self.role, self.since_unix_ms));
            }
            Some(existing) if holder_is_live(&existing) => {}
            _ => {
                let _ = create_exclusive(&self.path, &own_record(&self.role, self.since_unix_ms));
            }
        }
        self.write_mirrors();
    }

    /// Removes the record if, and only if, it is this process's own.
    pub fn release(&mut self) {
        self.release_with(remove_release_generation_locked, enqueue_lease_release);
    }

    fn release_with(
        &mut self,
        remove: impl FnOnce(&PendingLeaseRelease) -> bool,
        enqueue: impl FnOnce(PendingLeaseRelease),
    ) {
        if self.held {
            let cleanup = self
                .cleanup_permit
                .take()
                .expect("held lease owns cleanup admission");
            let transition_paths = self.transition_paths();
            let transition = LeaseTransitionGuard::acquire(&transition_paths, TRANSITION_LOCK_WAIT);
            self.held = false;
            if adjust_held(&self.path, false) == 0 {
                let pending = PendingLeaseRelease {
                    paths: transition_paths,
                    generation: own_record(&self.role, self.since_unix_ms),
                    _cleanup: cleanup,
                };
                let acknowledged = if let Ok(guard) = transition {
                    let acknowledged =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| remove(&pending)))
                            .unwrap_or(false);
                    // Transition handles and their cleanup retire before the
                    // final lifetime permit can be released or transferred.
                    drop(guard);
                    acknowledged
                } else {
                    false
                };
                if !acknowledged {
                    enqueue(pending);
                }
            }
        }
    }
}

impl Drop for LeaseHolder {
    fn drop(&mut self) {
        self.release();
    }
}

/// Removes the lease only if it is this process's own.
pub fn release(path: &Path) {
    if read_record(path).is_some_and(|record| is_own(&record)) {
        let _ = std::fs::remove_file(path);
    }
}

/// The status line for an open that found the device busy or deferred.
pub fn busy_detail(path: &Path) -> String {
    match read_live_other(path, std::process::id()) {
        Some(holder) => format!(
            "The Quad Cortex is in use by {} (process {}); waiting for it to let go - \
             QC Remote connects by itself as soon as it does",
            holder.role, holder.process_id
        ),
        None => "Quad Cortex is not attached or is owned by another application".into(),
    }
}

/// The status line naming a specific holder, for a deferred open.
pub fn waiting_detail(holder: &LeaseRecord) -> String {
    if holder.process_id == 0 {
        if holder.role == UNAVAILABLE_CLAIM_ROLE {
            return "The existing device ownership claim could not be read or validated and was preserved. Opening is refused; inspect the ownership record and its owner's status before explicitly retrying.".into();
        }
        if holder.role == CLEANUP_CAPACITY_ROLE {
            return "Native lease cleanup capacity is exhausted; waiting for exact active or deferred cleanup to retire before reopening the Quad Cortex.".into();
        }
        return "Device ownership is being updated; retrying before opening the Quad Cortex".into();
    }
    format!(
        "The Quad Cortex is in use by {} (process {}); waiting for it to let go - \
         QC Remote connects by itself as soon as it does",
        holder.role, holder.process_id
    )
}

#[cfg(test)]
fn wait_for_transition_test_peers(directory: &Path) {
    let marker = directory.join(format!("ready-{}", std::process::id()));
    std::fs::write(marker, b"ready").expect("test barrier marker is writable");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let ready = std::fs::read_dir(directory)
            .expect("test barrier directory is readable")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("ready-"))
            .count();
        if ready >= 2 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "both lease processes reach the stale-scan barrier"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Exact cleanup admission spans active/native references plus queued and
    /// in-flight generation cleanup, including mirrors and transition handles.
    pub(crate) fn root_has_lease_activity(root: &Path) -> bool {
        cleanup_lifetime::root_has_activity(root)
    }

    /// Match the production Linux directory contract for explicit test leases
    /// and descriptors. This changes only each test's own scratch directory.
    pub(crate) fn create_directory(path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// A live process that is not this one, killed on drop.
    pub(crate) struct FixtureRoot(pub(crate) PathBuf);
    impl Drop for FixtureRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub(crate) fn fresh_root(label: &str) -> std::sync::Arc<FixtureRoot> {
        for _ in 0..64 {
            let path = std::env::temp_dir().join(format!(
                "qc-lease-{label}-{:032x}",
                uuid::Uuid::new_v4().as_u128()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    let root = std::sync::Arc::new(FixtureRoot(path));
                    create_directory(&root.0).unwrap();
                    return root;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("reserve lease fixture root: {error}"),
            }
        }
        panic!("could not reserve lease fixture after 64 collisions");
    }

    pub(crate) struct BoundedChild {
        child: Option<std::process::Child>,
        root: Option<std::sync::Arc<FixtureRoot>>,
    }
    impl BoundedChild {
        pub(crate) fn new(
            child: std::process::Child,
            root: Option<std::sync::Arc<FixtureRoot>>,
        ) -> Self {
            Self {
                child: Some(child),
                root,
            }
        }
        pub(crate) fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
            let deadline = Instant::now() + Duration::from_secs(6);
            loop {
                if let Some(status) = self.child.as_mut().unwrap().try_wait()? {
                    return Ok(status);
                }
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "owned lease fixture child did not exit",
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    impl std::ops::Deref for BoundedChild {
        type Target = std::process::Child;
        fn deref(&self) -> &Self::Target {
            self.child.as_ref().unwrap()
        }
    }
    impl std::ops::DerefMut for BoundedChild {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.child.as_mut().unwrap()
        }
    }
    impl Drop for BoundedChild {
        fn drop(&mut self) {
            let Some(mut child) = self.child.take() else {
                return;
            };
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            let _ = child.kill();
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            static RETAINED: Mutex<
                Vec<(std::process::Child, Option<std::sync::Arc<FixtureRoot>>)>,
            > = Mutex::new(Vec::new());
            RETAINED
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push((child, self.root.clone()));
            eprintln!(
                "lease fixture cleanup unacknowledged; original child and owned root retained"
            );
        }
    }

    pub(crate) struct Sleeper(BoundedChild);
    impl Sleeper {
        pub(crate) fn start() -> Self {
            let root = fresh_root("sleeper");
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "device_lease::tests::transition_claim_crash_child_helper",
                ])
                .env(
                    "QC_DEVICE_LEASE_TEST_TRANSITION_LOCK_PATH",
                    root.0.join("sleeper.transition"),
                )
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000);
            }
            Self(BoundedChild::new(
                command.spawn().expect("a helper process starts"),
                Some(root),
            ))
        }
        pub(crate) fn pid(&self) -> u32 {
            self.0.id()
        }
        #[cfg(windows)]
        pub(crate) fn root_path(&self) -> &Path {
            &self.0.root.as_ref().unwrap().0
        }
        #[cfg(windows)]
        pub(crate) fn process_handle(&self) -> std::os::windows::io::RawHandle {
            use std::os::windows::io::AsRawHandle;
            self.0.as_raw_handle()
        }
        pub(crate) fn record(&self) -> LeaseRecord {
            LeaseRecord {
                process_id: self.pid(),
                process_created_at: crate::platform::process::os_process_created_at(self.pid()),
                role: "a hardware or conformance run (device broker --stdio)".into(),
                since_unix_ms: 1,
                refreshed_unix_ms: 1,
            }
        }
        pub(crate) fn kill(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    pub(crate) fn write_record(path: &Path, record: &LeaseRecord) {
        std::fs::write(path, serde_json::to_vec(record).unwrap()).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn create_test_directory(path: &Path) -> std::io::Result<()> {
        create_directory(path)
    }

    #[cfg(windows)]
    #[test]
    fn a_current_pid_without_recorded_known_creation_time_does_not_prove_ownership() {
        let path = scratch("own-pid-missing-creation");
        let current = crate::platform::process::os_process_created_at(std::process::id())
            .expect("this Windows process has a known creation time");
        let mut unknown = own_record("unknown recorded generation", next_lease_generation_ms());
        unknown.process_created_at = None;
        assert!(!is_own(&unknown));
        write_record(&path, &unknown);
        let bytes = std::fs::read(&path).unwrap();
        let mut opener = holder(&path);
        assert_eq!(opener.acquire().unwrap_err(), unknown);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(!opener.is_held());
        assert_eq!(held_count(&path), 0);
        assert!(opener.io_owner().is_err());
        // A positively matching current generation retains ordinary recovery.
        let mut known = unknown;
        known.process_created_at = Some(current);
        write_record(&path, &known);
        opener.acquire().unwrap();
        assert!(opener.is_held());
        opener.release();
        assert!(!path.exists());
    }

    #[test]
    fn lease_record_reader_bounds_growing_input_and_retries_interrupted_reads() {
        struct Growing {
            consumed: std::sync::Arc<std::sync::atomic::AtomicUsize>,
            interrupt: bool,
        }
        impl std::io::Read for Growing {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                if std::mem::take(&mut self.interrupt) {
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                bytes.fill(b' ');
                self.consumed.fetch_add(bytes.len(), Ordering::Relaxed);
                Ok(bytes.len()) // Deliberately never reaches EOF.
            }
        }
        let consumed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let error = read_record_from(Growing {
            consumed: consumed.clone(),
            interrupt: true,
        })
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(consumed.load(Ordering::Relaxed), MAX_LEASE_RECORD_BYTES + 1);

        let record = LeaseRecord {
            process_id: 1,
            process_created_at: None,
            role: String::new(),
            since_unix_ms: 2,
            refreshed_unix_ms: 3,
        };
        let overhead = encode_record(&record).unwrap().len();
        let mut exact = record;
        exact.role = "a".repeat(MAX_LEASE_RECORD_BYTES - overhead);
        let bytes = encode_record(&exact).unwrap();
        assert_eq!(bytes.len(), MAX_LEASE_RECORD_BYTES);
        assert_eq!(read_record_from(bytes.as_slice()).unwrap(), exact);
        exact.role.push('a');
        assert_eq!(
            encode_record(&exact).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn excessive_lease_records_preserve_existing_claim_and_do_not_publish_stages() {
        let root = fresh_root("record-byte-budget");
        let existing = root.0.join(RECORD_NAME);
        let sentinel = vec![b'x'; MAX_LEASE_RECORD_BYTES + 1];
        std::fs::write(&existing, &sentinel).unwrap();
        assert!(read_record(&existing).is_none());
        let excessive = LeaseRecord {
            process_id: std::process::id(),
            process_created_at: None,
            role: "a".repeat(MAX_LEASE_RECORD_BYTES + 1),
            since_unix_ms: 2,
            refreshed_unix_ms: 3,
        };
        assert_eq!(
            replace_own(&existing, &excessive).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(
            create_exclusive(&existing, &excessive).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(std::fs::read(&existing).unwrap(), sentinel);
        let missing_parent = root.0.join("not-created");
        assert_eq!(
            create_exclusive(&missing_parent.join(RECORD_NAME), &excessive)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
        assert!(!missing_parent.exists());
        let entries: Vec<_> = std::fs::read_dir(&root.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from(RECORD_NAME)]);
    }

    #[test]
    fn lease_stage_fault_cleanup_preserves_collisions_and_prior_published_bytes() {
        use std::io::Write;
        let root = fresh_root("stage-faults");
        let destination = root.0.join(RECORD_NAME);
        std::fs::write(&destination, b"prior document").unwrap();
        let staged = root.0.join("owned.tmp");
        let occupied = root.0.join("occupied.tmp");
        std::fs::write(&occupied, b"unowned contender").unwrap();
        assert_eq!(
            LeaseStage::at(occupied.clone()).err().unwrap().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&occupied).unwrap(), b"unowned contender");
        let failed = (|| {
            let mut stage = LeaseStage::at(staged.clone())?;
            stage.write_with(b"complete", |file, _| {
                file.write_all(b"partial")?;
                Err(std::io::Error::other("injected partial lease write"))
            })
        })();
        assert!(failed
            .unwrap_err()
            .to_string()
            .contains("partial lease write"));
        assert!(!staged.exists());
        let unwind = std::panic::catch_unwind(|| {
            let mut stage = LeaseStage::at(staged.clone()).unwrap();
            stage
                .write_with(b"complete", |file, _| {
                    file.write_all(b"partial")?;
                    panic!("injected lease stage unwind");
                })
                .unwrap();
        });
        assert!(unwind.is_err());
        assert!(!staged.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"prior document");
        let record = own_record("fixture", 1);
        replace_own(&destination, &record).unwrap();
        assert_eq!(read_record(&destination), Some(record));
        assert!(std::fs::read_dir(&root.0).unwrap().all(|entry| {
            let entry = entry.unwrap();
            !entry.file_name().to_string_lossy().ends_with(".tmp") || entry.path() == occupied
        }));
    }

    #[cfg(windows)]
    #[test]
    fn shared_directory_acl_command_uses_trusted_path_and_propagates_timeout_without_retry() {
        let root = fresh_root("acl-command");
        let mut calls = 0;
        let error = grant_shared_directory_with(&root.0, |command| {
            calls += 1;
            let program = PathBuf::from(command.get_program());
            assert!(program.is_absolute());
            assert_eq!(program.file_name().unwrap(), "icacls.exe");
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                vec![
                    root.0.as_os_str(),
                    std::ffi::OsStr::new("/grant"),
                    std::ffi::OsStr::new("*S-1-5-32-545:(OI)(CI)M")
                ]
            );
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "injected owned-tool timeout",
            ))
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[cfg(windows)]
    #[test]
    fn lease_fixture_unwind_reaps_exact_child_before_retiring_root() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{
            Foundation::{DuplicateHandle, DUPLICATE_SAME_ACCESS, WAIT_OBJECT_0},
            System::Threading::{GetCurrentProcess, WaitForSingleObject},
        };
        let mut process = None;
        let mut path = None;
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let sleeper = Sleeper::start();
            path = Some(sleeper.root_path().to_owned());
            let mut duplicated = std::ptr::null_mut();
            assert_ne!(
                unsafe {
                    DuplicateHandle(
                        GetCurrentProcess(),
                        sleeper.process_handle(),
                        GetCurrentProcess(),
                        &mut duplicated,
                        0,
                        0,
                        DUPLICATE_SAME_ACCESS,
                    )
                },
                0
            );
            process = Some(unsafe { OwnedHandle::from_raw_handle(duplicated) });
            panic!("injected child setup assertion failure");
        }));
        assert!(unwind.is_err());
        assert_eq!(
            unsafe { WaitForSingleObject(process.unwrap().as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        assert!(!path.unwrap().exists());
    }

    struct ScratchPath {
        path: PathBuf,
        _root: std::sync::Arc<FixtureRoot>,
    }
    impl std::ops::Deref for ScratchPath {
        type Target = PathBuf;
        fn deref(&self) -> &PathBuf {
            &self.path
        }
    }
    impl AsRef<Path> for ScratchPath {
        fn as_ref(&self) -> &Path {
            &self.path
        }
    }
    fn scratch(name: &str) -> ScratchPath {
        let root = fresh_root(name);
        ScratchPath {
            path: root.0.join("device-owner.json"),
            _root: root,
        }
    }

    fn holder(path: &Path) -> LeaseHolder {
        LeaseHolder::new(
            path.to_path_buf(),
            "QC Remote's shared device broker".into(),
        )
    }

    struct ChildGuard {
        child: BoundedChild,
    }

    struct TestDirectory(std::sync::Arc<FixtureRoot>);

    #[test]
    fn lease_transition_child_helper() {
        let Ok(directory) = std::env::var("QC_DEVICE_LEASE_TEST_RACE_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let path = directory.join(RECORD_NAME);
        let mut child = holder(&path);
        child.transition_test_barrier = Some(directory.clone());
        let result = child.acquire();
        let outcome = if result.is_ok() { "held" } else { "deferred" };
        std::fs::write(directory.join("child-result"), outcome).unwrap();
        if result.is_ok() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !directory.join("release-child").exists() {
                assert!(Instant::now() < deadline, "parent releases the child lease");
                std::thread::sleep(Duration::from_millis(5));
            }
            child.release();
        }
    }

    #[test]
    fn transition_claim_crash_child_helper() {
        let Ok(lock_path) = std::env::var("QC_DEVICE_LEASE_TEST_TRANSITION_LOCK_PATH") else {
            return;
        };
        let lock_path = PathBuf::from(lock_path);
        let _claim = qc_owner_only_file::ClaimedFile::try_claim(&lock_path)
            .expect("child acquires the valid transition claim");
        let ready_path = lock_path.with_extension("ready");
        let ready_temp = lock_path.with_extension(format!("ready-{}.tmp", std::process::id()));
        std::fs::write(&ready_temp, std::process::id().to_string())
            .expect("child signals that it holds the transition claim");
        std::fs::rename(&ready_temp, &ready_path)
            .expect("child atomically publishes its ready marker");
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    #[test]
    fn lease_release_retry_child_helper() {
        let Ok(path) = std::env::var("QC_DEVICE_LEASE_TEST_RELEASE_PATH") else {
            return;
        };
        let path = PathBuf::from(path);
        let result_path = path.with_extension("release-child-result");
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let mut child = holder(&path);
            match child.acquire() {
                Ok(()) => {
                    std::fs::write(&result_path, b"acquired").unwrap();
                    child.release();
                    return;
                }
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => panic!("deferred release eventually makes the lease available"),
            }
        }
    }

    #[test]
    fn concurrent_processes_cannot_both_recover_the_same_stale_lease() {
        let _owned_directory = TestDirectory(fresh_root("process-race"));
        let directory = _owned_directory.0 .0.clone();
        let path = directory.join(RECORD_NAME);
        write_record(
            &path,
            &LeaseRecord {
                process_id: u32::MAX - 1,
                process_created_at: Some(1),
                role: "stale test owner".into(),
                since_unix_ms: 1,
                refreshed_unix_ms: 1,
            },
        );

        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "device_lease::tests::lease_transition_child_helper",
            ])
            .env("QC_DEVICE_LEASE_TEST_RACE_DIR", &directory)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("lease race helper process starts");
        let mut child = ChildGuard {
            child: BoundedChild::new(child, Some(_owned_directory.0.clone())),
        };
        let mut parent = holder(&path);
        parent.transition_test_barrier = Some(directory.clone());
        let parent_result = parent.acquire();

        let result_path = directory.join("child-result");
        let deadline = Instant::now() + Duration::from_secs(6);
        while !result_path.exists() {
            assert!(Instant::now() < deadline, "child reports its lease result");
            std::thread::sleep(Duration::from_millis(5));
        }
        let child_result = std::fs::read_to_string(&result_path).unwrap();
        assert_ne!(
            parent_result.is_ok(),
            child_result == "held",
            "exactly one process owns the recovered stale lease"
        );
        if let Err(holder) = &parent_result {
            assert_eq!(
                holder.process_id,
                child.child.id(),
                "the loser defers to the winning process"
            );
        }
        if parent_result.is_ok() {
            parent.release();
        }
        std::fs::write(directory.join("release-child"), b"release").unwrap();
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            if let Some(status) = child.child.try_wait().unwrap() {
                assert!(status.success(), "lease race helper exits successfully");
                break;
            }
            if Instant::now() >= deadline {
                panic!("lease race helper exits after release");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_busy_device_names_a_live_holder_and_ignores_a_dead_one() {
        let path = scratch("holder");
        assert!(busy_detail(&path).contains("not attached"));
        let sleeper = Sleeper::start();
        write_record(&path, &sleeper.record());
        let detail = busy_detail(&path);
        assert_eq!(
            detail,
            format!(
                "The Quad Cortex is in use by a hardware or conformance run (device broker --stdio) \
                 (process {}); waiting for it to let go - QC Remote connects by itself as soon as it does",
                sleeper.pid()
            )
        );
        // Our own record is never a reason to wait.
        let mut own = sleeper.record();
        own.process_id = std::process::id();
        own.process_created_at = crate::platform::process::os_process_created_at(own.process_id);
        write_record(&path, &own);
        assert!(read_live_other(&path, std::process::id()).is_none());
        let dead = LeaseRecord {
            process_id: u32::MAX - 1,
            process_created_at: None,
            role: "x".into(),
            since_unix_ms: 1,
            refreshed_unix_ms: 1,
        };
        write_record(&path, &dead);
        assert!(read_live_other(&path, std::process::id()).is_none());
        assert!(busy_detail(&path).contains("not attached"));
    }

    #[test]
    fn a_second_opener_defers_to_a_live_owner_and_never_touches_its_record() {
        let path = scratch("defer");
        let sleeper = Sleeper::start();
        let owner_record = sleeper.record();
        write_record(&path, &owner_record);

        let mut second = holder(&path);
        let refused = second
            .acquire()
            .expect_err("a live owner must be deferred to");
        assert_eq!(refused.process_id, sleeper.pid());
        assert!(!second.is_held());
        assert!(waiting_detail(&refused).starts_with(&format!(
            "The Quad Cortex is in use by {} (process {}); waiting for it to let go",
            owner_record.role,
            sleeper.pid()
        )));

        // Retrying, refreshing and releasing as a non-owner leave it byte-for-byte.
        let before = std::fs::read(&path).unwrap();
        for _ in 0..3 {
            assert!(second.acquire().is_err());
            second.tick();
            second.release();
        }
        drop(second);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_waiting_opener_takes_over_after_the_owner_dies() {
        let path = scratch("takeover");
        let mut sleeper = Sleeper::start();
        write_record(&path, &sleeper.record());
        let mut second = holder(&path);
        assert!(second.acquire().is_err());
        sleeper.kill();
        second
            .acquire()
            .expect("a record whose process is gone is stale");
        assert!(second.is_held());
        let landed = read_record(&path).unwrap();
        assert_eq!(landed.process_id, std::process::id());
        second.release();
        assert!(
            !path.exists(),
            "the owner removes its own record on release"
        );
    }

    #[test]
    fn a_stale_record_with_a_dead_pid_or_reused_pid_is_ignored_and_replaced() {
        let path = scratch("stale");
        let dead = LeaseRecord {
            process_id: u32::MAX - 1,
            process_created_at: Some(1),
            role: "gone".into(),
            since_unix_ms: 1,
            refreshed_unix_ms: 1,
        };
        write_record(&path, &dead);
        let mut opener = holder(&path);
        opener.acquire().expect("dead owner");
        opener.release();

        // A live pid whose creation time differs is a reused pid, not the owner.
        let sleeper = Sleeper::start();
        let mut reused = sleeper.record();
        reused.process_created_at = reused.process_created_at.map(|created| created + 1);
        if reused.process_created_at.is_some() {
            write_record(&path, &reused);
            opener
                .acquire()
                .expect("pid reused by an unrelated process");
            opener.release();
        }

        // An unreadable record cannot prove its previous owner is gone.
        // Only the attributable dead/reused generations above are reaped.
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(opener.acquire().is_err());
        assert!(!opener.is_held());
        assert!(opener.io_owner().is_err());
        opener.release();
        assert_eq!(std::fs::read(&path).unwrap(), b"{ not json");
    }

    #[test]
    fn the_owner_refreshes_and_restores_its_record_but_removes_only_its_own() {
        let path = scratch("owner");
        let mut owner = holder(&path);
        owner.acquire().unwrap();
        let first = read_record(&path).unwrap();
        assert!(is_own(&first));

        // Someone deletes the record of a live owner: the next refresh restores it.
        std::fs::remove_file(&path).unwrap();
        owner.last_refresh = Instant::now() - REFRESH_INTERVAL - Duration::from_millis(1);
        owner.tick();
        assert!(is_own(&read_record(&path).unwrap()));

        // If a live other process's record is there, release never removes it.
        let sleeper = Sleeper::start();
        write_record(&path, &sleeper.record());
        owner.release();
        assert_eq!(read_record(&path).unwrap().process_id, sleeper.pid());
    }

    #[test]
    fn dropping_the_holder_is_the_owners_clean_shutdown() {
        let path = scratch("drop");
        {
            let mut owner = holder(&path);
            owner.acquire().unwrap();
            assert!(path.exists());
        }
        assert!(!path.exists());
    }

    fn two_roots(name: &str) -> (ScratchPath, ScratchPath) {
        let base = fresh_root(name);
        let program_data = base.0.join("ProgramData");
        let local = base.0.join("Local");
        create_test_directory(&program_data).unwrap();
        create_test_directory(&local).unwrap();
        (
            ScratchPath {
                path: program_data,
                _root: base.clone(),
            },
            ScratchPath {
                path: local,
                _root: base,
            },
        )
    }

    fn packaged_copy(local: &Path) -> PathBuf {
        let dir = local
            .join("Packages")
            .join("OpenAI.Codex_test")
            .join("LocalCache")
            .join("Local")
            .join(APP_DIRECTORY);
        create_test_directory(&dir).unwrap();
        dir.join(RECORD_NAME)
    }

    fn shared_holder(program_data: &Path, local: &Path) -> LeaseHolder {
        LeaseHolder::with_locations(
            Locations::resolve(Some(program_data), Some(local)),
            "QC Remote's shared device broker".into(),
            false,
        )
    }

    #[test]
    fn transition_lock_timeout_refuses_without_reaping_a_live_claim() {
        let path = scratch("transition-timeout");
        let targets = vec![path.clone()];
        let owner = LeaseTransitionGuard::acquire(&targets, Duration::from_millis(100)).unwrap();
        assert!(LeaseTransitionGuard::acquire(&targets, Duration::from_millis(50)).is_err());
        assert!(
            path.with_extension("transition").exists(),
            "the active owner's lock remains"
        );
        drop(owner);
        let next = LeaseTransitionGuard::acquire(&targets, Duration::from_millis(100))
            .expect("a released transition lock is acquired normally");
        drop(next);
    }

    #[test]
    fn timed_out_last_release_is_retried_while_process_remains_alive() {
        let _directory = TestDirectory(fresh_root("release-retry"));
        let directory = _directory.0 .0.clone();
        let path = directory.join("device-owner.json");
        let mut owner = holder(&path);
        owner.acquire().unwrap();
        let transition =
            LeaseTransitionGuard::acquire(std::slice::from_ref(&path), Duration::from_millis(100))
                .unwrap();

        owner.release();
        assert!(
            path.exists(),
            "the lease remains until its transition lock is available"
        );
        drop(transition);

        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "device_lease::tests::lease_release_retry_child_helper",
            ])
            .env("QC_DEVICE_LEASE_TEST_RELEASE_PATH", &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("lease release retry helper process starts");
        let mut child = ChildGuard {
            child: BoundedChild::new(child, Some(_directory.0.clone())),
        };
        let result_path = path.with_extension("release-child-result");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !result_path.exists() {
            assert!(
                Instant::now() < deadline,
                "another process acquires after deferred cleanup"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(std::fs::read(&result_path).unwrap(), b"acquired");
        assert!(
            child.child.wait().unwrap().success(),
            "the child lease process exits cleanly"
        );
        assert!(crate::platform::process::os_process_is_alive(
            std::process::id()
        ));
    }

    #[test]
    fn own_generation_replacement_failure_or_false_success_never_admits_a_lease() {
        for false_success in [false, true] {
            let _directory = TestDirectory(fresh_root("generation-replacement-refusal"));
            let path = _directory.0 .0.join("device-owner.json");
            let old = own_record("retired generation", next_lease_generation_ms());
            write_record(&path, &old);
            let bytes = std::fs::read(&path).unwrap();
            let mut opener = holder(&path);
            let mut called = false;
            let result = opener.acquire_with_replacer(|_, current| {
                called = true;
                assert_ne!(current.since_unix_ms, old.since_unix_ms);
                if false_success {
                    Ok(())
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "controlled replacement refusal",
                    ))
                }
            });
            assert!(called, "actual own-record replacement branch was exercised");
            assert!(result.is_err());
            assert!(!opener.is_held());
            assert_eq!(held_count(&path), 0);
            assert!(opener.io_owner().is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_eq!(read_record(&path), Some(old.clone()));
            // A retry may acquire only after actual replacement/readback.
            opener.acquire().unwrap();
            let current = read_record(&path).unwrap();
            assert_ne!(current.since_unix_ms, old.since_unix_ms);
            assert_eq!(current.since_unix_ms, opener.since_unix_ms);
            let late = PendingLeaseRelease {
                paths: vec![path.clone()],
                generation: old,
                _cleanup: CleanupLimiter::production()
                    .reserve(vec![path.clone()])
                    .unwrap(),
            };
            let _transition = LeaseTransitionGuard::acquire(
                std::slice::from_ref(&path),
                Duration::from_millis(100),
            )
            .unwrap();
            assert!(remove_release_generation_locked(&late));
            assert_eq!(read_record(&path), Some(current));
            drop(_transition);
            opener.release();
            assert_eq!(held_count(&path), 0);
        }
    }

    #[test]
    fn malformed_existing_claim_is_preserved_without_new_holder_admission() {
        let _directory = TestDirectory(fresh_root("malformed-existing-claim"));
        let path = _directory.0 .0.join("device-owner.json");
        let sentinel = b"{malformed existing owner; liveness cannot be proved}";
        std::fs::write(&path, sentinel).unwrap();
        let mut opener = holder(&path);
        let refused = opener.acquire().unwrap_err();
        assert!(waiting_detail(&refused).contains("was preserved"));
        assert!(waiting_detail(&refused).contains("Opening is refused"));
        assert!(!waiting_detail(&refused).contains("being updated"));
        assert!(!opener.is_held());
        assert_eq!(held_count(&path), 0);
        assert!(opener.io_owner().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), sentinel);
    }

    #[test]
    fn cleanup_capacity_refuses_acquisition_before_directory_or_claim_effects() {
        let root = fresh_root("cleanup-admission-before-effects");
        let limiter = CleanupLimiter::new(1);
        let occupied = limiter
            .reserve(vec![root.0.join("old-generation.json")])
            .unwrap();
        let unopened = root.0.join("not-created");
        let mut opener = LeaseHolder::new(unopened.join("device-owner.json"), "fixture".into());
        opener.shared_directory = Some(unopened.clone());
        opener.cleanup_limiter = std::sync::Arc::clone(&limiter);
        let refused = opener.acquire().unwrap_err();
        assert_eq!(refused.role, CLEANUP_CAPACITY_ROLE);
        assert!(waiting_detail(&refused).contains("cleanup capacity"));
        assert!(
            !unopened.exists(),
            "no directory/ACL/claim effect before admission"
        );
        assert!(!opener.is_held());
        assert!(opener.cleanup_permit.is_none());
        drop(occupied);
        opener.acquire().unwrap();
        opener.release();
    }

    #[test]
    fn exact_cleanup_capability_survives_native_clone_queue_inflight_and_failure_until_ack() {
        let root = fresh_root("exact-cleanup-capability");
        let path = root.0.join("device-owner.json");
        let limiter = CleanupLimiter::new(1);
        let mut owner = holder(&path);
        owner.cleanup_limiter = std::sync::Arc::clone(&limiter);
        owner.acquire().unwrap();
        let generation = read_record(&path).unwrap();
        let capability = std::sync::Arc::downgrade(owner.cleanup_permit.as_ref().unwrap());
        let native = owner.io_owner().unwrap();
        assert_eq!(
            capability.strong_count(),
            2,
            "native clone shares exact admission"
        );
        let stable_generation = owner.since_unix_ms;
        assert!(owner
            .acquire_with_replacer(|_, _| Err(std::io::Error::other("controlled refusal")))
            .is_err());
        assert_eq!(owner.since_unix_ms, stable_generation);
        assert!(owner.is_held());
        assert_eq!(capability.strong_count(), 2);
        drop(native);
        let mut queued = None;
        owner.release_with(
            |_| false,
            |job| {
                queued = Some(job);
            },
        );
        assert!(!owner.is_held());
        assert_eq!(held_count(&path), 0);
        assert!(cleanup_lifetime::root_has_activity(&root.0));
        assert!(limiter.reserve(vec![path.clone()]).is_err());
        let job = queued.take().unwrap();
        assert!(same_generation(&job.generation, &generation));
        assert!(std::sync::Arc::ptr_eq(
            &capability.upgrade().unwrap(),
            &job._cleanup
        ));
        let job = attempt_release(job, |job| {
            assert!(same_generation(&job.generation, &generation));
            assert!(
                cleanup_lifetime::root_has_activity(&root.0),
                "popped in-flight job still owns root"
            );
            false
        })
        .unwrap();
        let job = attempt_release(job, |_| panic!("controlled cleanup unwind")).unwrap();
        assert!(cleanup_lifetime::root_has_activity(&root.0));
        assert!(limiter.reserve(vec![path.clone()]).is_err());
        assert!(attempt_release(job, remove_release_generation).is_none());
        assert!(!cleanup_lifetime::root_has_activity(&root.0));
        assert!(capability.upgrade().is_none());
        assert!(!path.exists());
        let next = limiter.reserve(vec![path]).unwrap();
        drop(next);
    }

    #[test]
    fn delayed_release_job_cannot_delete_a_new_generation_from_same_process() {
        let _directory = TestDirectory(fresh_root("new-generation"));
        let directory = _directory.0 .0.clone();
        let path = directory.join("device-owner.json");
        let old = own_record("lease generation", unix_ms());
        let newer = own_record("lease generation", old.since_unix_ms + 1);
        write_record(&path, &newer);
        let job = PendingLeaseRelease {
            paths: vec![path.clone()],
            generation: old,
            _cleanup: CleanupLimiter::production()
                .reserve(vec![path.clone()])
                .unwrap(),
        };

        let _transition =
            LeaseTransitionGuard::acquire(std::slice::from_ref(&path), Duration::from_millis(100))
                .unwrap();
        assert!(remove_release_generation_locked(&job));
        assert_eq!(read_record(&path), Some(newer));
    }

    #[test]
    fn lease_generation_is_unique_even_within_one_wall_clock_millisecond() {
        let first = next_lease_generation_ms();
        let second = next_lease_generation_ms();
        assert!(second > first);
        assert_eq!(next_generation_after(1_000, 1_000), 1_001);
    }

    #[test]
    fn same_process_holders_share_one_generation_until_the_last_release() {
        let _directory = TestDirectory(fresh_root("shared-generation"));
        let directory = _directory.0 .0.clone();
        let path = directory.join("device-owner.json");
        let mut first = holder(&path);
        let mut second = holder(&path);
        first.acquire().unwrap();
        let generation = read_record(&path).unwrap().since_unix_ms;
        second.acquire().unwrap();
        assert_eq!(read_record(&path).unwrap().since_unix_ms, generation);
        first.release();
        assert!(path.exists(), "the second local holder keeps the lease");
        second.release();
        assert!(
            !path.exists(),
            "the last local holder removes the generation"
        );
    }

    #[test]
    fn native_io_owner_retains_the_exact_acquired_generation_until_last_handle_drop() {
        let path = scratch("native-owner");
        let mut owner = holder(&path);
        assert!(
            owner.io_owner().is_err(),
            "no capability before acquisition"
        );
        owner.acquire().unwrap();
        let generation = read_record(&path).unwrap().since_unix_ms;
        let native = owner.io_owner().unwrap();
        let writer = std::sync::Arc::clone(&native);
        owner.release();
        assert_eq!(read_record(&path).unwrap().since_unix_ms, generation);
        drop(native);
        assert!(path.exists(), "retained writer still owns the lease");
        drop(writer);
        assert!(!path.exists(), "last native owner retires its generation");
        assert_eq!(held_count(&path), 0);
    }

    #[test]
    fn late_native_owner_drop_does_not_remove_a_replacement_generation() {
        let path = scratch("native-new-generation");
        let mut owner = holder(&path);
        owner.acquire().unwrap();
        let native = owner.io_owner().unwrap();
        let generation = read_record(&path).unwrap().since_unix_ms;
        owner.release();
        let replacement = own_record("replacement", generation + 1);
        write_record(&path, &replacement);
        drop(native);
        assert_eq!(read_record(&path), Some(replacement));
        assert_eq!(held_count(&path), 0);
    }

    #[test]
    fn transition_lock_reaps_a_claim_left_by_a_crashed_owner() {
        let _directory = TestDirectory(fresh_root("transition-crash"));
        let directory = _directory.0 .0.clone();
        let path = directory.join("device-owner.json");
        let lock = path.with_extension("transition");
        let claim = PathBuf::from(format!("{}.claim", lock.display()));
        let ready = lock.with_extension("ready");
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "device_lease::tests::transition_claim_crash_child_helper",
            ])
            .env("QC_DEVICE_LEASE_TEST_TRANSITION_LOCK_PATH", &lock)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("transition claim owner child starts");
        let mut child = ChildGuard {
            child: BoundedChild::new(child, Some(_directory.0.clone())),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            assert!(
                Instant::now() < deadline,
                "child signals after acquiring the valid transition claim"
            );
            assert!(
                child.child.try_wait().unwrap().is_none(),
                "transition claim owner remains alive until the parent kills it"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let claimed_pid: u32 = std::fs::read_to_string(&ready)
            .expect("the child wrote its ready marker")
            .parse()
            .expect("the ready marker contains the child pid");
        assert_eq!(claimed_pid, child.child.id());
        assert!(
            child.child.try_wait().unwrap().is_none(),
            "the child still owns the claim before the live-owner check"
        );
        assert!(lock.exists(), "the live owner keeps its lock document");
        assert!(claim.exists(), "the live owner keeps its claim sidecar");
        assert!(
            qc_owner_only_file::release_if_unclaimed(&lock).is_err(),
            "a live child claim is never reaped"
        );
        assert!(lock.exists(), "the live owner keeps its lock document");
        assert!(claim.exists(), "the live owner keeps its claim sidecar");

        child
            .child
            .kill()
            .expect("the parent terminates its exact owner child");
        let status = child
            .child
            .wait()
            .expect("the terminated claim owner is reaped");
        assert!(!status.success(), "the child was forcibly terminated");
        assert!(lock.exists(), "process death leaves the lock document");
        assert!(claim.exists(), "process death leaves the claim sidecar");

        let guard = LeaseTransitionGuard::acquire(&[path], TRANSITION_LOCK_WAIT)
            .expect("the unlocked abandoned claim is safely reaped");
        assert!(lock.exists(), "the new owner has installed its document");
        drop(guard);
        assert!(!lock.exists());
        assert!(!claim.exists());
        assert!(
            ready.exists(),
            "the owned test directory still contains its marker"
        );
        // The reaped original Child still owns a scratch-root Arc until its
        // guard is retired. Release that acknowledged owner before asserting
        // that dropping the last directory owner removes the scratch bytes.
        drop(child);
        drop(_directory);
        assert!(
            !directory.exists(),
            "the test removes its owned scratch directory"
        );
        assert!(!ready.exists());
    }

    #[test]
    fn locations_put_the_record_in_the_shared_directory_and_list_every_other_copy() {
        let (program_data, local) = two_roots("resolve");
        let record = packaged_copy(&local);
        std::fs::write(&record, b"{}").unwrap();
        let locations = Locations::resolve(Some(&program_data), Some(&local));
        assert_eq!(
            locations.primary,
            program_data.join("QC Remote").join("device-owner.json")
        );
        assert_eq!(
            locations.mirrors,
            vec![local.join("org.freevia.qcremote").join("device-owner.json")]
        );
        assert_eq!(locations.peers, vec![record]);
        // Without ProgramData the legacy file is primary and nothing is mirrored.
        let legacy = Locations::resolve(None, Some(&local));
        assert_eq!(
            legacy.primary,
            local.join("org.freevia.qcremote").join("device-owner.json")
        );
        assert!(legacy.mirrors.is_empty());
    }

    #[test]
    fn an_owner_recorded_only_in_a_packaged_processs_cache_is_deferred_to() {
        let (program_data, local) = two_roots("packaged");
        let sleeper = Sleeper::start();
        write_record(&packaged_copy(&local), &sleeper.record());
        let mut opener = shared_holder(&program_data, &local);
        let holder = opener
            .acquire()
            .expect_err("a live owner visible only through the package cache defers us");
        assert_eq!(holder.process_id, sleeper.pid());
        assert!(!opener.is_held());
        assert!(
            !Locations::resolve(Some(&program_data), Some(&local))
                .primary
                .exists(),
            "a deferred opener writes nothing"
        );
        assert!(opener
            .busy_detail()
            .contains(&format!("process {}", sleeper.pid())));
    }

    #[test]
    fn an_owner_recorded_only_in_the_legacy_directory_is_deferred_to() {
        let (program_data, local) = two_roots("legacy");
        let sleeper = Sleeper::start();
        let legacy = local.join(APP_DIRECTORY);
        create_test_directory(&legacy).unwrap();
        write_record(&legacy.join(RECORD_NAME), &sleeper.record());
        let mut opener = shared_holder(&program_data, &local);
        assert!(opener.acquire().is_err());
    }

    #[test]
    fn a_dead_pid_in_any_location_is_ignored_and_the_open_proceeds() {
        let (program_data, local) = two_roots("dead");
        let dead = LeaseRecord {
            process_id: u32::MAX - 1,
            process_created_at: Some(1),
            role: "gone".into(),
            since_unix_ms: 1,
            refreshed_unix_ms: 1,
        };
        write_record(&packaged_copy(&local), &dead);
        let mut opener = shared_holder(&program_data, &local);
        opener.acquire().expect("dead owners never block");
        assert!(opener.is_held());
        opener.release();
    }

    #[test]
    fn the_owner_writes_the_shared_record_and_its_mirror_and_removes_both() {
        let (program_data, local) = two_roots("mirror");
        let locations = Locations::resolve(Some(&program_data), Some(&local));
        let mut owner = shared_holder(&program_data, &local);
        owner.acquire().unwrap();
        assert!(is_own(&read_record(&locations.primary).unwrap()));
        assert!(is_own(&read_record(&locations.mirrors[0]).unwrap()));
        // A deleted mirror is restored on refresh.
        std::fs::remove_file(&locations.mirrors[0]).unwrap();
        owner.last_refresh = Instant::now() - REFRESH_INTERVAL - Duration::from_millis(1);
        owner.tick();
        assert!(is_own(&read_record(&locations.mirrors[0]).unwrap()));
        owner.release();
        assert!(!locations.primary.exists());
        assert!(!locations.mirrors[0].exists());
    }

    #[test]
    fn unknown_mirror_refuses_acquire_and_tick_preserves_its_bytes() {
        struct RestoreMirror {
            path: PathBuf,
            bytes: Vec<u8>,
        }
        impl Drop for RestoreMirror {
            fn drop(&mut self) {
                // Restore the attributable generation before owner/root Drop
                // even if a behavioral assertion unwinds.
                let _ = std::fs::write(&self.path, &self.bytes);
            }
        }
        let root = fresh_root("unknown-mirror-refusal");
        let primary = root.0.join("primary.json");
        let mirror = root.0.join("legacy.json");
        let sentinel = b"{unknown legacy owner; not proof of retirement}";
        std::fs::write(&mirror, sentinel).unwrap();
        let mut owner = LeaseHolder::new(primary.clone(), "fixture".into());
        owner.mirrors.push(mirror.clone());
        let refused = owner.acquire().unwrap_err();
        assert!(waiting_detail(&refused).contains("Opening is refused"));
        assert!(!owner.is_held());
        assert!(owner.io_owner().is_err());
        assert!(!primary.exists(), "no new primary/native admission");
        assert_eq!(std::fs::read(&mirror).unwrap(), sentinel);
        assert!(!testing::root_has_lease_activity(&root.0));

        // An absent mirror is still supported and published normally.
        std::fs::remove_file(&mirror).unwrap();
        owner.acquire().unwrap();
        let restore = RestoreMirror {
            path: mirror.clone(),
            bytes: std::fs::read(&mirror).unwrap(),
        };
        std::fs::write(&mirror, sentinel).unwrap();
        owner.last_refresh = Instant::now() - REFRESH_INTERVAL - Duration::from_millis(1);
        owner.tick();
        assert_eq!(std::fs::read(&mirror).unwrap(), sentinel);
        assert!(same_generation(
            &read_record(&primary).unwrap(),
            &own_record("fixture", owner.since_unix_ms)
        ));
        drop(restore);
        owner.release();
        assert!(!primary.exists());
        assert!(!mirror.exists());
    }

    #[test]
    fn an_attributable_dead_mirror_still_recovers_and_releases_normally() {
        let root = fresh_root("dead-mirror-recovery");
        let primary = root.0.join("primary.json");
        let mirror = root.0.join("legacy.json");
        write_record(
            &mirror,
            &LeaseRecord {
                process_id: u32::MAX - 1,
                process_created_at: Some(1),
                role: "gone".into(),
                since_unix_ms: 1,
                refreshed_unix_ms: 1,
            },
        );
        let mut owner = LeaseHolder::new(primary.clone(), "fixture".into());
        owner.mirrors.push(mirror.clone());
        owner.acquire().unwrap();
        assert!(same_generation(
            &read_record(&primary).unwrap(),
            &read_record(&mirror).unwrap()
        ));
        owner.release();
        assert!(!primary.exists());
        assert!(!mirror.exists());
    }

    #[test]
    fn unknown_read_only_peer_refuses_admission_without_mutating_or_locking_peer() {
        let root = fresh_root("unknown-read-only-peer");
        let primary = root.0.join("primary.json");
        let peer = root.0.join("packaged-peer.json");
        let sentinel = b"{unvalidated packaged owner}";
        std::fs::write(&peer, sentinel).unwrap();
        let mut owner = LeaseHolder::new(primary.clone(), "fixture".into());
        owner.peers.push(peer.clone());
        let refused = owner.acquire().unwrap_err();
        assert!(waiting_detail(&refused).contains("Opening is refused"));
        assert!(!owner.is_held());
        assert!(owner.io_owner().is_err());
        assert!(!primary.exists());
        assert_eq!(std::fs::read(&peer).unwrap(), sentinel);
        assert!(!peer.with_extension("transition").exists());
        assert!(!testing::root_has_lease_activity(&root.0));
        let stale = LeaseRecord {
            process_id: u32::MAX - 1,
            process_created_at: Some(1),
            role: "gone".into(),
            since_unix_ms: 1,
            refreshed_unix_ms: 1,
        };
        write_record(&peer, &stale);
        let peer_bytes = std::fs::read(&peer).unwrap();
        owner.acquire().unwrap();
        assert_eq!(std::fs::read(&peer).unwrap(), peer_bytes);
        owner.release();
        assert!(!primary.exists());
        assert_eq!(std::fs::read(&peer).unwrap(), peer_bytes);
        assert!(!peer.with_extension("transition").exists());
    }

    #[test]
    fn shared_primary_holder_also_serializes_with_a_legacy_primary_constructor() {
        let (program_data, local) = two_roots("legacy-transition-lock");
        let locations = Locations::resolve(Some(&program_data), Some(&local));
        let shared = shared_holder(&program_data, &local);
        let legacy = LeaseHolder::new(locations.mirrors[0].clone(), "older device broker".into());
        let shared_guard =
            LeaseTransitionGuard::acquire(&shared.transition_paths(), Duration::from_millis(100))
                .unwrap();
        assert!(
            LeaseTransitionGuard::acquire(&legacy.transition_paths(), Duration::from_millis(50))
                .is_err(),
            "the new broker holds the legacy mirror's transition key too"
        );
        drop(shared_guard);
        assert!(LeaseTransitionGuard::acquire(
            &legacy.transition_paths(),
            Duration::from_millis(100)
        )
        .is_ok());
    }

    #[test]
    fn releasing_never_removes_a_live_other_ownerss_mirror() {
        let (program_data, local) = two_roots("othermirror");
        let locations = Locations::resolve(Some(&program_data), Some(&local));
        let mut owner = shared_holder(&program_data, &local);
        owner.acquire().unwrap();
        let sleeper = Sleeper::start();
        write_record(&locations.mirrors[0], &sleeper.record());
        owner.release();
        assert_eq!(
            read_record(&locations.mirrors[0]).unwrap().process_id,
            sleeper.pid()
        );
    }

    fn scanned(pid: u32, line: Option<&str>) -> ScannedProcess {
        ScannedProcess {
            pid,
            created_at: Some(7),
            command_line: line.map(str::to_owned),
        }
    }

    #[test]
    fn the_scan_decision_names_only_a_live_other_stdio_broker() {
        let alive = |_pid: u32| true;
        let stdio = r#""C:\x\qc-device-broker.exe" --stdio --log y"#;
        let serve = r#""C:\x\qc-device-broker.exe" --broker-serve --descriptor-path z"#;
        // A stdio broker is an owner.
        let owner = stdio_owner_from_scan(&[scanned(10, Some(stdio))], 1, false, alive).unwrap();
        assert_eq!(owner.process_id, 10);
        assert!(owner.role.contains("--stdio"));
        // Not ourselves, not the app's serve broker, not an unreadable command line.
        assert!(stdio_owner_from_scan(&[scanned(1, Some(stdio))], 1, false, alive).is_none());
        assert!(stdio_owner_from_scan(&[scanned(10, Some(serve))], 1, false, alive).is_none());
        assert!(stdio_owner_from_scan(&[scanned(10, None)], 1, false, alive).is_none());
        // A flag that merely contains the word, or a path, is not --stdio.
        let lookalike = r#"qc-device-broker.exe --stdio-log C:\--stdio\x"#;
        assert!(stdio_owner_from_scan(&[scanned(10, Some(lookalike))], 1, false, alive).is_none());
        // Unknown creation time, or a process that is gone, never counts.
        let mut unknown = scanned(10, Some(stdio));
        unknown.created_at = None;
        assert!(stdio_owner_from_scan(&[unknown], 1, false, alive).is_none());
        assert!(stdio_owner_from_scan(&[scanned(10, Some(stdio))], 1, false, |_| false).is_none());
        // A hardware run never defers to another hardware run's scan.
        assert!(stdio_owner_from_scan(&[scanned(10, Some(stdio))], 1, true, alive).is_none());
        // The first live match wins over an earlier dead one.
        let both = [scanned(10, Some(stdio)), scanned(11, Some(stdio))];
        assert_eq!(
            stdio_owner_from_scan(&both, 1, false, |pid| pid == 11)
                .unwrap()
                .process_id,
            11
        );
    }

    #[test]
    fn lease_scratch_roots_are_retired_on_partial_setup_unwind() {
        let mut root = None;
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let path = scratch("root-unwind");
            root = Some(path.parent().unwrap().to_owned());
            std::fs::write(&path, b"partial fixture").unwrap();
            panic!("injected lease fixture setup assertion failure");
        }));
        assert!(unwind.is_err());
        assert!(!root.unwrap().exists());
    }
}
