//! The resident host: one process per user session that is the device broker,
//! draws the broker icon, and serves the background MCP host. The three
//! cannot work independently, so the running binary is one, while the source
//! stays separate. This module only supervises the host; the host itself is
//! supplied by the binary through [`ResidentHost`] (the shipped
//! `qc-device-broker.exe` links `qc_remote_lib`'s background MCP host).
//!
//! Two rules, kept apart on purpose:
//! - **Residence** (does the process outlive its last client?): only while
//!   the tray switch is on and the icon thread is alive
//!   ([`crate::rpc::BrokerHandle::set_resident`], set by the tray state
//!   thread). The host runs exactly while the process is resident.
//! - **The device hold** (is the Quad Cortex kept open?): unchanged. Past the
//!   grace only for a participant connection or the registered icon's idle
//!   policy. A resident reap closes the device session and keeps the process.
//!   The host is an ordinary participant, attached only while it has active
//!   MCP clients, so it holds the device exactly as a window would.
use crate::rpc::BrokerHandle;
use serde::Deserialize;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::{Duration, Instant};

pub(crate) const DESCRIPTOR_FILE: &str = "device-broker.json";

/// The background services a resident process runs beside the broker.
pub trait ResidentHost: Send + Sync + 'static {
    /// Start serving for the descriptor directory. Must never start a second
    /// broker process: this process is the broker. `host_token` is this
    /// process's [`host_token`]: the host presents it as `hostToken` in its
    /// `broker.hello`, which is how the broker tells its own host from every
    /// other client. It never leaves the process.
    ///
    /// `readiness` answers whether this broker's device session is open
    /// without a connection, a request or a participant, so the host can
    /// report readiness while it holds no participant connection.
    fn start(
        &self,
        directory: &Path,
        host_token: &str,
        readiness: DeviceReadinessProbe,
    ) -> Result<Box<dyn RunningHost>, String>;

    /// The icon's Unlink on a local MCP client: end that one session in this
    /// process's host, by the id the host published for it. `Ok(false)` when
    /// the host holds no such session (it ended meanwhile, or another process
    /// published it). Must touch no other session and add no block: the
    /// bridge's heartbeat no longer lists it, and that chat's next request
    /// links it again.
    fn end_local_session(&self, session_id: &str) -> Result<bool, String> {
        let _ = session_id;
        Err("This build's background host cannot end a session".into())
    }

    /// The icon's Unlink on a remote MCP client: revoke the OAuth grant of the
    /// remote session this process's host published under `session_id`. The
    /// host resolves the grant from its own live relay presence, never from
    /// the icon, and revokes that one grant only; the account and this
    /// computer's pairing are untouched. Blocking, bounded: call it off the
    /// icon's UI and state threads.
    fn unlink_remote_session(&self, session_id: &str) -> Result<RemoteUnlink, String> {
        let _ = session_id;
        Err("This build's background host cannot unlink a remote app".into())
    }

    /// The icon's Unlink on a linked remote app: revoke the OAuth grant the
    /// host's own linked-apps list holds for the opaque `entry_id` it
    /// published, whether or not a session is live, then refresh that list.
    /// Same rules as [`ResidentHost::unlink_remote_session`].
    fn unlink_linked_app(&self, entry_id: &str) -> Result<RemoteUnlink, String> {
        let _ = entry_id;
        Err("This build's background host cannot unlink a remote app".into())
    }
}

/// What the host did for a remote Unlink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteUnlink {
    /// Listed just before the revoke and gone just after; that app must sign
    /// in again.
    Unlinked,
    /// The account's list, read just before, no longer had it; nothing sent.
    NotListed,
    /// No such live remote session (or listed app) here any more.
    SessionEnded,
    /// Live, but the relay did not name its grant; nothing was revoked.
    GrantUnknown,
    /// The host's list came from an earlier pairing; nothing was sent.
    StaleList,
}

/// What the broker in this process says about its device session. See
/// [`BrokerHandle::device_readiness`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceReadiness {
    /// The session is open.
    Ready,
    /// The broker is serving and the session is not open; `phase` is the
    /// worker's own phase (for example `disconnected`, also after an idle
    /// release, or `handshaking` while a connect runs).
    NotReady { phase: String },
    /// The broker cannot answer for the device at all.
    Unavailable(String),
}

/// The one read of the broker the host is given: device readiness, and
/// nothing that could open, reopen, attach or release.
#[derive(Clone)]
pub struct DeviceReadinessProbe(BrokerHandle);

impl DeviceReadinessProbe {
    pub fn new(handle: BrokerHandle) -> Self {
        Self(handle)
    }

    pub fn read(&self) -> DeviceReadiness {
        self.0.device_readiness()
    }
}

/// A random token generated once per process. Only code in this process can
/// know it, so a connection that presents it is this process's own host; a
/// client that merely names itself after the host is not.
pub fn host_token() -> &'static str {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN.get_or_init(|| {
        let bytes: [u8; 32] = rand::random();
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    })
}

/// Whether `presented` is this process's host token, compared in constant time.
pub fn is_host_token(presented: &str) -> bool {
    use subtle::ConstantTimeEq;
    let expected = host_token().as_bytes();
    presented.len() == expected.len() && bool::from(presented.as_bytes().ct_eq(expected))
}

/// A started host.
pub trait RunningHost: Send {
    /// The host's own thread ended (it stopped, failed or panicked).
    fn finished(&self) -> bool;
    /// Ask it to stop and wait at most `within`. Never longer.
    fn stop(&mut self, within: Duration);
}

const TICK: Duration = Duration::from_millis(250);
const RESTART_BACKOFF: Duration = Duration::from_secs(5);
/// How long a supervisor tick may spend stopping the host it no longer wants.
const STOP_BOUND: Duration = Duration::from_secs(3);

/// Owns the running host. Every path that takes `host` either is the
/// supervisor tick or bounds its wait with `try_lock`, so a wedged host can
/// delay but never prevent process exit or an installer's stop.
pub struct Supervisor {
    host: Mutex<Option<Box<dyn RunningHost>>>,
    paused: AtomicBool,
    exiting: AtomicBool,
}

impl Supervisor {
    /// Starts the supervising thread for a published broker.
    pub fn start(
        handle: BrokerHandle,
        services: Arc<dyn ResidentHost>,
    ) -> Result<Arc<Self>, String> {
        let supervisor = Arc::new(Self {
            host: Mutex::new(None),
            paused: AtomicBool::new(false),
            exiting: AtomicBool::new(false),
        });
        let running = Arc::clone(&supervisor);
        std::thread::Builder::new()
            .name("qc-resident-host".into())
            .spawn(move || running.run(handle, services))
            .map_err(|error| error.to_string())?;
        Ok(supervisor)
    }

    fn run(&self, handle: BrokerHandle, services: Arc<dyn ResidentHost>) {
        let directory = handle.directory();
        let mut marker = SessionMarker::default();
        let mut next_attempt = Instant::now();
        while !self.exiting.load(Ordering::Acquire) {
            let resident = handle.is_resident() && !handle.shutting_down();
            marker.set(resident);
            let wanted = resident && !self.paused.load(Ordering::Acquire);
            {
                // A panic while the slot was held leaves it poisoned; the
                // slot itself is still the best record of the host.
                let mut host = self
                    .host
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !wanted {
                    if let Some(mut running) = host.take() {
                        stop_quietly(running.as_mut(), STOP_BOUND);
                    }
                } else if host.as_ref().is_none_or(|running| running.finished())
                    && Instant::now() >= next_attempt
                    && !self.exiting.load(Ordering::Acquire)
                {
                    *host = None;
                    next_attempt = Instant::now() + RESTART_BACKOFF;
                    match catch_unwind(AssertUnwindSafe(|| {
                        services.start(
                            &directory,
                            host_token(),
                            DeviceReadinessProbe::new(handle.clone()),
                        )
                    })) {
                        Ok(Ok(running)) => *host = Some(running),
                        Ok(Err(error)) => {
                            eprintln!("The background MCP host could not start; retrying: {error}")
                        }
                        Err(_) => eprintln!("The background MCP host panicked while starting"),
                    }
                }
            }
            std::thread::sleep(TICK);
        }
        marker.set(false);
    }

    /// For process exit: stop the host, bounded by `within` in total, and
    /// never start it again. Called after clients were sent
    /// `broker.shuttingDown`, so the host's own connection already knows.
    pub fn stop_for_exit(&self, within: Duration) {
        self.exiting.store(true, Ordering::Release);
        self.stop_within(within);
    }

    /// For an installer's request: stop the host and keep it stopped until
    /// [`Supervisor::resume`]. Returns whether it stopped within `within`.
    pub fn pause(&self, within: Duration) -> bool {
        self.paused.store(true, Ordering::Release);
        self.stop_within(within)
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::Release);
    }

    pub fn host_running(&self) -> bool {
        match self.host.try_lock() {
            Ok(host) => host.as_ref().is_some_and(|running| !running.finished()),
            Err(TryLockError::Poisoned(_)) => false,
            Err(TryLockError::WouldBlock) => true,
        }
    }

    fn stop_within(&self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            match self.host.try_lock() {
                Ok(mut host) => {
                    if let Some(mut running) = host.take() {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        stop_quietly(running.as_mut(), remaining);
                        return running.finished();
                    }
                    return true;
                }
                Err(TryLockError::Poisoned(poisoned)) => {
                    if let Some(mut running) = poisoned.into_inner().take() {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        stop_quietly(running.as_mut(), remaining);
                    }
                    return false;
                }
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

fn stop_quietly(running: &mut dyn RunningHost, within: Duration) {
    if catch_unwind(AssertUnwindSafe(|| running.stop(within))).is_err() {
        eprintln!("The background MCP host panicked while stopping");
    }
}

/// The named object `qc-remote.exe` reads to learn that the session's resident
/// host is running (its Settings row and its "start it now" decision). The
/// name is the one the separate `qc-remote-tray.exe` held before 0.1.178, so
/// an old tray from another installation cannot start a second background
/// host beside this one, and an old app still reads it as running.
pub const SESSION_MARKER: &str = "Local\\QCRemote.DeviceBrokerTray.v1";

#[derive(Default)]
struct SessionMarker {
    #[cfg(windows)]
    handle: Option<isize>,
}

impl SessionMarker {
    #[cfg(windows)]
    fn set(&mut self, present: bool) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::CreateMutexW;
        match (present, self.handle) {
            (true, None) => {
                let name: Vec<u16> = SESSION_MARKER.encode_utf16().chain(Some(0)).collect();
                // SAFETY: default security, an unowned mutex, a terminated name
                // read only during the call.
                let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
                if !handle.is_null() {
                    self.handle = Some(handle as isize);
                }
            }
            (false, Some(handle)) => {
                // SAFETY: closing the handle this value owns.
                unsafe { CloseHandle(handle as _) };
                self.handle = None;
            }
            _ => {}
        }
    }

    #[cfg(not(windows))]
    fn set(&mut self, _present: bool) {}
}

impl Drop for SessionMarker {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// The outcome of a `--resident` start's election.
pub enum Election {
    /// This process serves; drop the guard once the descriptor is published.
    Serve(ElectionGuard),
    /// A live broker already holds the descriptor (or another process is
    /// mid-election and will publish one): exit cleanly.
    AlreadyServed,
}

/// The app's election lock (`<descriptor>.election`), held from the decision
/// to serve until publication, and removed on drop.
pub struct ElectionGuard(Option<qc_owner_only_file::ClaimedFile>);

impl Drop for ElectionGuard {
    fn drop(&mut self) {
        if let Some(claim) = self.0.take() {
            let _ = claim.release();
        }
    }
}

/// Mirrors `DEVICE_BROKER_ELECTION_LOCK_STALE_AFTER` in
/// apps/windows/src-tauri/src/lib.rs.
#[cfg(any(not(target_os = "linux"), test))]
const ELECTION_LOCK_STALE_AFTER: Duration = Duration::from_secs(15);

pub fn election_path(descriptor: &Path) -> PathBuf {
    descriptor.with_extension("election")
}

/// Join the same election every app window and MCP bridge runs before it
/// spawns a broker, so a logon start can never put a second broker on the
/// device or delete a live broker's descriptor. Never touches USB.
pub fn elect(descriptor: &Path, budget: Duration) -> Result<Election, String> {
    #[cfg(target_os = "linux")]
    {
        let directory = descriptor
            .parent()
            .ok_or("Broker descriptor has no directory")?;
        crate::platform::paths::prepare_directory(directory)?;
        match qc_owner_only_file::read_owner_only(descriptor) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Refusing an unsafe broker descriptor: {error}")),
        }
    }
    elect_with(descriptor, budget, &os_liveness)
}

fn elect_with(
    descriptor: &Path,
    budget: Duration,
    alive: &dyn Fn(u32, Option<u64>) -> bool,
) -> Result<Election, String> {
    if let Some(directory) = descriptor.parent() {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
    }
    let election = election_path(descriptor);
    let deadline = Instant::now() + budget;
    let mut reported_held = false;
    loop {
        if descriptor_is_live(descriptor, alive) {
            return Ok(Election::AlreadyServed);
        }
        match qc_owner_only_file::ClaimedFile::try_claim(&election) {
            Ok(claim) => {
                let guard = ElectionGuard(Some(claim));
                // A broker may have published between the check and the
                // claim; a replaced descriptor gets one more look.
                if descriptor_is_live(descriptor, alive) {
                    return Ok(Election::AlreadyServed);
                }
                std::thread::sleep(Duration::from_millis(100));
                if descriptor_is_live(descriptor, alive) {
                    return Ok(Election::AlreadyServed);
                }
                // Proven dead, or not a descriptor at all: the new claim
                // (`create_new`) needs both the descriptor and its `.claim`
                // sidecar free. A killed broker leaves both behind, and
                // removing only the descriptor made this start's own claim
                // fail. The app's election winner clears the pair the same
                // way (`release_descriptor_slot`): only under the sidecar's
                // lock, so a claim another process still holds refuses and
                // is left exactly as it is. Its holder is a broker
                // publishing (or about to): this start does not serve.
                if let Err(error) = qc_owner_only_file::release_if_unclaimed(descriptor) {
                    eprintln!(
                        "Not serving: the descriptor claim at {} is held by another process \
                         and was left in place ({error})",
                        descriptor.display()
                    );
                    return Ok(Election::AlreadyServed);
                }
                return Ok(Election::Serve(guard));
            }
            Err(_) => {
                // A start that died holding the election leaves the lock and
                // its `.claim` sidecar. Removing only the lock left the
                // sidecar to refuse every later claim, so each logon start
                // waited out its budget and exited 0 without serving. Clear
                // the pair the way the app does (`release_descriptor_slot`):
                // only under the sidecar's lock, so a live holder refuses and
                // keeps both. A sidecar whose lock is already gone (what that
                // older cleanup left behind) is aged by the sidecar itself.
                #[cfg(target_os = "linux")]
                let abandoned = true;
                #[cfg(not(target_os = "linux"))]
                let abandoned = {
                    let age = |path: &Path| {
                        std::fs::metadata(path)
                            .and_then(|metadata| metadata.modified())
                            .ok()
                            .and_then(|modified| modified.elapsed().ok())
                    };
                    match age(&election) {
                        Some(age) => age > ELECTION_LOCK_STALE_AFTER,
                        None => {
                            !election.exists()
                                && age(&qc_owner_only_file::claim_path(&election))
                                    .is_some_and(|age| age > ELECTION_LOCK_STALE_AFTER)
                        }
                    }
                };
                // Linux relies on the OS lock and inode identity alone. A
                // process killed before publication must not keep the slot
                // for 15 seconds while the next launch's budget expires.
                if abandoned {
                    match qc_owner_only_file::release_if_unclaimed(&election) {
                        Ok(()) => continue,
                        Err(error) if !reported_held => {
                            reported_held = true;
                            eprintln!(
                                "The election lock at {} looks abandoned but its claim is held; \
                             waiting for its holder ({error})",
                                election.display()
                            );
                        }
                        Err(_) => {}
                    }
                }
            }
        }
        if Instant::now() >= deadline {
            // Someone else holds the election and will publish a broker,
            // which becomes the resident host itself.
            return Ok(Election::AlreadyServed);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn os_liveness(pid: u32, created_at: Option<u64>) -> bool {
    #[cfg(target_os = "linux")]
    {
        if !crate::platform::process::os_process_is_alive(pid) {
            return false;
        }
        // An unreadable identity is unknown, never authority to reap an owner.
        match (
            created_at,
            crate::platform::process::os_process_created_at(pid),
        ) {
            (Some(recorded), Some(current)) => recorded == current,
            _ => true,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::platform::process::os_process_is_alive(pid)
            && created_at.is_none_or(|created| {
                crate::platform::process::os_process_created_at(pid) == Some(created)
            })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DescriptorProcessIdentity {
    process_id: u32,
    #[serde(default)]
    process_created_at: Option<u64>,
}

fn descriptor_is_live(descriptor: &Path, alive: &dyn Fn(u32, Option<u64>) -> bool) -> bool {
    // Election is a process-liveness check, not a broker-client connection.
    // Keep its minimal projection separate from the full descriptor used by
    // clients, so unrelated connection fields cannot make a live owner look
    // dead and trigger a second broker.
    let read = || -> Option<Option<DescriptorProcessIdentity>> {
        match std::fs::read(descriptor) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => Some(None),
            Ok(bytes) => Some(serde_json::from_slice::<DescriptorProcessIdentity>(&bytes).ok()),
        }
    };
    let parsed = match read() {
        None => return false,
        Some(Some(parsed)) => parsed,
        // The owner atomically renames a complete descriptor over this path;
        // retry once in case the old generation disappeared during publish.
        Some(None) => {
            std::thread::sleep(Duration::from_millis(100));
            match read() {
                Some(Some(parsed)) => parsed,
                _ => return false,
            }
        }
    };
    parsed.process_id != 0 && alive(parsed.process_id, parsed.process_created_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn scratch(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "qc-resident-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn descriptor_json(pid: u32, created: u64) -> String {
        serde_json::json!({
            "port": 1, "bearerToken": "t", "instanceId": "i", "processId": pid,
            "processCreatedAt": created, "publishedAtUnixMs": 1, "gatewayApiVersion": 3
        })
        .to_string()
    }

    fn minimal_descriptor_json(pid: u32, created: Option<u64>) -> String {
        serde_json::json!({
            "processId": pid,
            "processCreatedAt": created,
        })
        .to_string()
    }

    #[test]
    fn a_live_broker_means_a_resident_start_exits_cleanly() {
        let directory = scratch("live");
        let descriptor = directory.join(DESCRIPTOR_FILE);
        std::fs::write(&descriptor, descriptor_json(42, 7)).unwrap();
        let alive = |pid: u32, created: Option<u64>| pid == 42 && created == Some(7);
        assert!(matches!(
            elect_with(&descriptor, Duration::from_secs(1), &alive).unwrap(),
            Election::AlreadyServed
        ));
        assert!(
            descriptor.exists(),
            "a live broker's descriptor is never touched"
        );
        assert!(
            !election_path(&descriptor).exists(),
            "no election lock left behind"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_live_minimal_process_identity_means_a_resident_start_exits_cleanly() {
        let directory = scratch("live-minimal");
        let descriptor = directory.join(DESCRIPTOR_FILE);
        std::fs::write(&descriptor, minimal_descriptor_json(42, Some(7))).unwrap();
        let alive = |pid: u32, created: Option<u64>| pid == 42 && created == Some(7);
        assert!(matches!(
            elect_with(&descriptor, Duration::from_secs(1), &alive).unwrap(),
            Election::AlreadyServed
        ));
        assert!(
            descriptor.exists(),
            "the live owner's descriptor is untouched"
        );
        assert!(
            !election_path(&descriptor).exists(),
            "no election lock remains"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_dead_or_recycled_holder_is_replaced_under_the_election_lock() {
        let directory = scratch("dead");
        let descriptor = directory.join(DESCRIPTOR_FILE);
        std::fs::write(&descriptor, descriptor_json(42, 7)).unwrap();
        // pid alive but a different creation time: a recycled pid.
        let recycled = |pid: u32, created: Option<u64>| pid == 42 && created == Some(8);
        let Election::Serve(guard) =
            elect_with(&descriptor, Duration::from_secs(1), &recycled).unwrap()
        else {
            panic!("a recycled pid must not keep the descriptor");
        };
        assert!(
            !descriptor.exists(),
            "the stale descriptor is cleared for the claim"
        );
        assert!(
            election_path(&descriptor).exists(),
            "held until publication"
        );
        drop(guard);
        assert!(!election_path(&descriptor).exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// A resident broker killed outright (Stop-Process) leaves an unlocked
    /// `.claim` sidecar and a descriptor naming its dead pid. The next
    /// `--resident` start must take over: clear both, serve, and claim the
    /// slot exactly as `serve_broker_tcp` does, so it never exits 1.
    #[test]
    fn a_killed_owners_stale_claim_never_blocks_the_next_resident_start() {
        let directory = scratch("killed");
        let descriptor = directory.join(DESCRIPTOR_FILE);
        // Process exit closes the claim's handle but leaves both names.
        drop(qc_owner_only_file::ClaimedFile::try_claim(&descriptor).unwrap());
        std::fs::write(&descriptor, descriptor_json(42, 7)).unwrap();
        assert!(qc_owner_only_file::claim_path(&descriptor).exists());
        let dead = |_: u32, _: Option<u64>| false;
        let Election::Serve(guard) =
            elect_with(&descriptor, Duration::from_secs(1), &dead).unwrap()
        else {
            panic!("a dead owner's descriptor must not keep the slot");
        };
        assert!(
            !descriptor.exists(),
            "the dead owner's descriptor is cleared"
        );
        assert!(
            !qc_owner_only_file::claim_path(&descriptor).exists(),
            "the dead owner's unlocked claim is cleared"
        );
        // What `serve_broker_tcp` does next; refusing it is the exit 1.
        let claim = qc_owner_only_file::ClaimedFile::try_claim(&descriptor)
            .expect("the resident start claims the freed descriptor slot");
        drop(guard);
        assert!(
            !election_path(&descriptor).exists(),
            "no election lock left behind"
        );
        assert!(!qc_owner_only_file::claim_path(&election_path(&descriptor)).exists());
        claim.release().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// A claim another process holds is a broker publishing: the start never
    /// removes it, never serves, and releases the election lock it took.
    /// A live pid in the descriptor is the same: nothing is touched.
    #[test]
    fn a_held_or_live_claim_is_never_removed_and_the_start_steps_aside() {
        let directory = scratch("held");
        let descriptor = directory.join(DESCRIPTOR_FILE);
        let mut held = qc_owner_only_file::ClaimedFile::try_claim(&descriptor).unwrap();
        // Even a descriptor naming a dead pid cannot free a held claim.
        held.write(descriptor_json(42, 7).as_bytes()).unwrap();
        assert!(matches!(
            elect_with(&descriptor, Duration::from_secs(1), &|_, _| false).unwrap(),
            Election::AlreadyServed
        ));
        assert_eq!(
            std::fs::read_to_string(&descriptor).unwrap(),
            descriptor_json(42, 7),
            "the holder's descriptor is untouched"
        );
        assert!(
            qc_owner_only_file::claim_path(&descriptor).exists(),
            "the held claim is untouched"
        );
        assert!(
            qc_owner_only_file::ClaimedFile::try_claim(&descriptor).is_err(),
            "the holder still owns the slot"
        );
        assert!(
            !election_path(&descriptor).exists(),
            "the election lock this start took is released"
        );

        let alive = |pid: u32, created: Option<u64>| pid == 42 && created == Some(7);
        assert!(matches!(
            elect_with(&descriptor, Duration::from_secs(1), &alive).unwrap(),
            Election::AlreadyServed
        ));
        assert!(descriptor.exists() && qc_owner_only_file::claim_path(&descriptor).exists());
        assert!(!election_path(&descriptor).exists());
        held.release().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Age a file past the election's staleness bound, as a crash long ago.
    fn age_past_stale(path: &Path) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - ELECTION_LOCK_STALE_AFTER * 4)
            .unwrap();
    }

    /// A start that died holding the election leaves the lock
    /// and its `.claim` sidecar. Removing only the lock left the sidecar to
    /// refuse every later claim, so every logon start exited 0 unserved.
    #[test]
    fn a_crashed_election_holder_never_blocks_the_next_resident_start() {
        let directory = scratch("crashed-election");
        let descriptor = directory.join(DESCRIPTOR_FILE);
        let election = election_path(&descriptor);
        // Process exit closes the claim's handle but leaves both names.
        drop(qc_owner_only_file::ClaimedFile::try_claim(&election).unwrap());
        age_past_stale(&election);
        assert!(qc_owner_only_file::claim_path(&election).exists());
        let Election::Serve(guard) =
            elect_with(&descriptor, Duration::from_millis(500), &|_, _| false).unwrap()
        else {
            panic!("a crashed election holder must not keep every start from serving");
        };
        drop(guard);
        assert!(!election.exists() && !qc_owner_only_file::claim_path(&election).exists());

        // What the older cleanup left behind: the sidecar alone.
        drop(qc_owner_only_file::ClaimedFile::try_claim(&election).unwrap());
        std::fs::remove_file(&election).unwrap();
        age_past_stale(&qc_owner_only_file::claim_path(&election));
        let Election::Serve(guard) =
            elect_with(&descriptor, Duration::from_millis(500), &|_, _| false).unwrap()
        else {
            panic!("an orphaned election sidecar must not keep every start from serving");
        };
        drop(guard);
        assert!(!election.exists() && !qc_owner_only_file::claim_path(&election).exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_recovers_a_fresh_abandoned_election_without_waiting_for_age() {
        let directory = scratch("linux-fresh-crash");
        let descriptor = directory.join(DESCRIPTOR_FILE);
        let election = election_path(&descriptor);
        drop(qc_owner_only_file::ClaimedFile::try_claim(&election).unwrap());
        let Election::Serve(guard) =
            elect_with(&descriptor, Duration::from_millis(500), &|_, _| false).unwrap()
        else {
            panic!("a freshly abandoned OS lock must not consume the restart budget");
        };
        drop(guard);
        assert!(!election.exists());
        assert!(!qc_owner_only_file::claim_path(&election).exists());
        std::fs::remove_dir(directory).unwrap();
    }

    /// A held election lock is never cleared, however old its file.
    #[test]
    fn an_old_but_held_election_lock_is_never_overtaken() {
        let directory = scratch("old-held-election");
        let descriptor = directory.join(DESCRIPTOR_FILE);
        let election = election_path(&descriptor);
        let peer = qc_owner_only_file::ClaimedFile::try_claim(&election).unwrap();
        age_past_stale(&election);
        assert!(matches!(
            elect_with(&descriptor, Duration::from_millis(300), &|_, _| false).unwrap(),
            Election::AlreadyServed
        ));
        assert!(election.exists() && qc_owner_only_file::claim_path(&election).exists());
        peer.release().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_peer_holding_the_election_is_never_overtaken() {
        let directory = scratch("peer");
        let descriptor = directory.join(DESCRIPTOR_FILE);
        let peer = qc_owner_only_file::ClaimedFile::try_claim(&election_path(&descriptor)).unwrap();
        let started = Instant::now();
        assert!(matches!(
            elect_with(&descriptor, Duration::from_millis(300), &|_, _| false).unwrap(),
            Election::AlreadyServed
        ));
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert!(
            election_path(&descriptor).exists(),
            "the peer's lock is left alone"
        );
        peer.release().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn two_racing_resident_starts_elect_exactly_one() {
        let directory = scratch("race");
        let descriptor = directory.join(DESCRIPTOR_FILE);
        let served = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let (descriptor, served, barrier) = (
                    descriptor.clone(),
                    Arc::clone(&served),
                    Arc::clone(&barrier),
                );
                std::thread::spawn(move || {
                    barrier.wait();
                    if let Election::Serve(guard) =
                        elect_with(&descriptor, Duration::from_millis(600), &|_, _| false).unwrap()
                    {
                        served.fetch_add(1, Ordering::SeqCst);
                        // Hold past the loser's budget, as a real publish would.
                        std::thread::sleep(Duration::from_millis(800));
                        drop(guard);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(served.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    struct FakeHost {
        stopped: Arc<AtomicBool>,
        wedged: bool,
    }
    impl RunningHost for FakeHost {
        fn finished(&self) -> bool {
            self.stopped.load(Ordering::SeqCst)
        }
        fn stop(&mut self, within: Duration) {
            if self.wedged {
                // A wedged host ignores the request; it may only use its bound.
                std::thread::sleep(within);
                return;
            }
            self.stopped.store(true, Ordering::SeqCst);
        }
    }

    fn supervisor_with(host: FakeHost) -> Supervisor {
        Supervisor {
            host: Mutex::new(Some(Box::new(host))),
            paused: AtomicBool::new(false),
            exiting: AtomicBool::new(false),
        }
    }

    #[test]
    fn exit_stops_the_host_and_a_wedged_host_cannot_hold_it_past_its_bound() {
        let stopped = Arc::new(AtomicBool::new(false));
        let supervisor = supervisor_with(FakeHost {
            stopped: Arc::clone(&stopped),
            wedged: false,
        });
        supervisor.stop_for_exit(Duration::from_secs(1));
        assert!(stopped.load(Ordering::SeqCst));
        assert!(supervisor.exiting.load(Ordering::SeqCst));

        let wedged = supervisor_with(FakeHost {
            stopped: Arc::new(AtomicBool::new(false)),
            wedged: true,
        });
        let started = Instant::now();
        wedged.stop_for_exit(Duration::from_millis(200));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "exit is bounded"
        );

        // A tick holding the slot (a start that hangs) cannot block exit either.
        let busy = Arc::new(supervisor_with(FakeHost {
            stopped: Arc::new(AtomicBool::new(false)),
            wedged: false,
        }));
        let holder = Arc::clone(&busy);
        let (held_tx, held) = std::sync::mpsc::channel();
        let hold = std::thread::spawn(move || {
            let _slot = holder.host.lock().unwrap();
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(800));
        });
        held.recv().unwrap();
        let started = Instant::now();
        busy.stop_for_exit(Duration::from_millis(150));
        assert!(started.elapsed() < Duration::from_millis(600));
        hold.join().unwrap();
    }

    #[test]
    fn an_installer_pause_stops_the_host_until_resumed() {
        let stopped = Arc::new(AtomicBool::new(false));
        let supervisor = supervisor_with(FakeHost {
            stopped: Arc::clone(&stopped),
            wedged: false,
        });
        assert!(supervisor.pause(Duration::from_secs(1)));
        assert!(stopped.load(Ordering::SeqCst));
        assert!(supervisor.paused.load(Ordering::SeqCst));
        assert!(!supervisor.host_running());
        supervisor.resume();
        assert!(!supervisor.paused.load(Ordering::SeqCst));
        let wedged = supervisor_with(FakeHost {
            stopped: Arc::new(AtomicBool::new(false)),
            wedged: true,
        });
        let started = Instant::now();
        assert!(
            !wedged.pause(Duration::from_millis(100)),
            "reported, not waited on"
        );
        assert!(started.elapsed() < Duration::from_millis(600));
    }

    #[test]
    fn only_this_processs_host_token_identifies_the_host() {
        let token = host_token();
        assert_eq!(token.len(), 64);
        assert_eq!(token, host_token(), "one token per process");
        assert!(is_host_token(token));
        assert!(!is_host_token(""));
        assert!(!is_host_token(&token[..63]));
        let mut forged = token.to_string();
        forged.replace_range(0..1, if token.starts_with('0') { "1" } else { "0" });
        assert!(!is_host_token(&forged));
    }
}
