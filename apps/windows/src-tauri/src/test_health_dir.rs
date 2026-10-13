//! The directory the unit tests' runtime health record and broker stderr log
//! live in, one per test process, and the sweep that removes the ones earlier
//! test processes left behind.
//!
//! Every test in this binary shares the directory, and brokers the tests
//! spawn keep `broker-stderr.log` open and can outlive any single test - or
//! the test process itself - so no test may remove it, and the harness has no
//! hook that runs after the last one. Instead the directory records who may
//! still use it, and a later test process removes it only once every one of
//! them is proven gone:
//!
//! - its name carries the test process's pid and OS creation time, and the
//!   `owner` file inside repeats both;
//! - `users/` holds one `<pid>-<created>` file for each child spawned with its
//!   stderr in the directory, written right after the spawn, and a
//!   `pending-<n>` file while a spawn is in flight.
//!
//! The sweep runs once per test process, on the first use of the directory.
//! It removes a sibling only when its name, owner and every user parse, every
//! recorded process is verifiably dead (no live process has its pid, or the
//! one that has it was created at another time), and nothing in it changed
//! for an hour. Anything unreadable, unparsable or in doubt is kept: never a
//! deletion on a guessed owner. That includes the pid-only directories
//! earlier builds left, which have no owner record at all.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

const PREFIX: &str = "qc-remote-test-health-";
const OWNER_FILE: &str = "owner";
const USERS_DIRECTORY: &str = "users";
const PENDING_PREFIX: &str = "pending-";
/// Written by a sweep that has proven a directory dead, before it removes
/// anything, so a sweep interrupted part way leaves a directory a later one
/// still recognises instead of one whose records are half gone.
const REMOVING_FILE: &str = "removing";
const OWNER_VERSION: &str = "qc-remote-test-health-owner-v1";
/// Nothing in a directory may have changed for this long before it is
/// removed: a margin for a writer that no record names.
const QUIET_PERIOD: Duration = Duration::from_secs(60 * 60);

/// One process, as the OS identifies it: a pid alone is reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessRecord {
    pid: u32,
    created_at: u64,
}

impl ProcessRecord {
    fn file_name(self) -> String {
        format!("{}-{}", self.pid, self.created_at)
    }

    /// Exactly the text `file_name` writes: plain decimal digits, no sign,
    /// no leading zero, nothing else.
    fn parse(text: &str) -> Option<Self> {
        let (pid, created_at) = text.split_once('-')?;
        if ![pid, created_at]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return None;
        }
        let record = Self {
            pid: pid.parse().ok()?,
            created_at: created_at.parse().ok()?,
        };
        (record.file_name() == text).then_some(record)
    }
}

fn owner_text(owner: ProcessRecord) -> String {
    format!("{OWNER_VERSION}\n{}\n{}\n", owner.pid, owner.created_at)
}

static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// This test process's directory. The first call creates it, records this
/// process as its owner, and sweeps the dead siblings earlier runs left.
pub(crate) fn directory() -> &'static Path {
    DIRECTORY.get_or_init(|| {
        let root = std::env::temp_dir();
        let pid = std::process::id();
        let Some(created_at) = qc_native_platform::process::os_process_created_at(pid) else {
            // No creation time to record (an OS this crate reads none on): a
            // name the sweep never parses, so this directory is never swept.
            return root.join(format!("{PREFIX}{pid}-unidentified"));
        };
        let own = ProcessRecord { pid, created_at };
        let directory = root.join(format!("{PREFIX}{}", own.file_name()));
        // `users/` first: a directory with an owner record always has it.
        if fs::create_dir_all(directory.join(USERS_DIRECTORY)).is_ok() {
            let _ = fs::write(directory.join(OWNER_FILE), owner_text(own));
        }
        sweep(
            &root,
            &directory,
            &recorded_process_is_dead,
            SystemTime::now(),
            QUIET_PERIOD,
        );
        directory
    })
}

/// Makes this test process's directory permanently unsweepable, for when a
/// child that may hold its log could not be recorded. A leak, never a
/// deletion under a live process.
fn renounce() {
    let _ = fs::remove_file(directory().join(OWNER_FILE));
}

static NEXT_PENDING: AtomicU64 = AtomicU64::new(0);

/// A spawn in flight whose child will hold the broker stderr log. Until
/// [`PendingUse::record`] names the child, its `pending-<n>` file keeps every
/// sweep away; a spawn that fails drops this and removes the file.
pub(crate) struct PendingUse {
    pending: Option<PathBuf>,
}

impl PendingUse {
    pub(crate) fn begin() -> Self {
        let pending = directory().join(USERS_DIRECTORY).join(format!(
            "{PENDING_PREFIX}{}",
            NEXT_PENDING.fetch_add(1, Ordering::Relaxed)
        ));
        Self {
            pending: fs::write(&pending, b"").is_ok().then_some(pending),
        }
    }

    /// Names the spawned child. A child whose creation time cannot be read
    /// or written down leaves the directory unsweepable instead.
    pub(crate) fn record(mut self, pid: u32) {
        let pending = self.pending.take();
        let recorded =
            qc_native_platform::process::os_process_created_at(pid).is_some_and(|created_at| {
                let record = ProcessRecord { pid, created_at };
                let path = directory().join(USERS_DIRECTORY).join(record.file_name());
                fs::write(path, b"").is_ok()
            });
        match pending {
            Some(pending) if recorded => {
                let _ = fs::remove_file(pending);
            }
            // The pending file stays, and keeps the sweep away on its own.
            Some(_) => {}
            None if recorded => {}
            None => renounce(),
        }
    }
}

impl Drop for PendingUse {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            let _ = fs::remove_file(pending);
        }
    }
}

/// True only when the OS proves the recorded process gone: no live process
/// has its pid, or the one that has it was created at another time. An
/// unreadable creation time is doubt, and doubt is alive.
fn recorded_process_is_dead(record: ProcessRecord) -> bool {
    use qc_native_platform::process::{os_process_created_at, os_process_is_alive};
    if !os_process_is_alive(record.pid) {
        return true;
    }
    matches!(
        os_process_created_at(record.pid),
        Some(created_at) if created_at != record.created_at
    )
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Remove,
    Keep(&'static str),
}

/// Whether `directory`, a sibling of this process's own, may be removed.
fn assess(
    directory: &Path,
    is_dead: &dyn Fn(ProcessRecord) -> bool,
    now: SystemTime,
    quiet_period: Duration,
) -> Verdict {
    let Some(owner) = directory
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(PREFIX))
        .and_then(ProcessRecord::parse)
    else {
        return Verdict::Keep("not a directory name this sweep writes");
    };
    if !fs::symlink_metadata(directory).is_ok_and(|meta| meta.is_dir() && !meta.is_symlink()) {
        return Verdict::Keep("not a plain directory");
    }
    if fs::read_to_string(directory.join(OWNER_FILE)).ok() != Some(owner_text(owner)) {
        return Verdict::Keep("no owner record matching its name");
    }
    // Asked before the users are listed: only the owner adds users, so once
    // it is dead the list read below is final.
    if !is_dead(owner) {
        return Verdict::Keep("its owner is alive or unknown");
    }
    if fs::symlink_metadata(directory.join(REMOVING_FILE)).is_ok_and(|meta| meta.is_file()) {
        // An earlier sweep proved everything below and was interrupted.
        return Verdict::Remove;
    }
    let Some(users) = read_users(&directory.join(USERS_DIRECTORY)) else {
        return Verdict::Keep("a user record is unreadable, unparsable or pending");
    };
    if !users.into_iter().all(is_dead) {
        return Verdict::Keep("a recorded child is alive or unknown");
    }
    if !quiet_since(directory, now, quiet_period) {
        return Verdict::Keep("changed too recently, or a time is unreadable");
    }
    Verdict::Remove
}

/// Every user record, or `None` when any entry is not one - a pending spawn,
/// a link, a name that does not parse, or a directory that cannot be read.
fn read_users(users: &Path) -> Option<Vec<ProcessRecord>> {
    fs::read_dir(users)
        .ok()?
        .map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            ProcessRecord::parse(entry.file_name().to_str()?)
        })
        .collect()
}

/// Whether the directory, its entries and its user records all last changed
/// at least `quiet_period` before `now`.
fn quiet_since(directory: &Path, now: SystemTime, quiet_period: Duration) -> bool {
    let quiet = |path: &Path| {
        fs::symlink_metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= quiet_period)
    };
    let entries_quiet = |path: &Path| {
        fs::read_dir(path).is_ok_and(|mut entries| {
            entries.all(|entry| entry.is_ok_and(|entry| quiet(&entry.path())))
        })
    };
    quiet(directory) && entries_quiet(directory) && entries_quiet(&directory.join(USERS_DIRECTORY))
}

/// Removes a directory `assess` found dead. Every entry but the owner
/// record and the removing mark goes first; those two are then removed
/// explicitly, in order - owner, then removing - and only once both are
/// gone is the now-empty directory itself removed. A single `remove_dir_all`
/// over the last two files would leave their relative order to the OS, so an
/// interruption there could strand a directory holding only one of the two
/// marks; removing them as separate, ordered steps instead means the worst
/// an interruption leaves behind is one of two fully-specified states, not
/// an unspecified third. Links are removed, never followed. False when
/// anything is left.
fn remove(directory: &Path) -> bool {
    if fs::write(directory.join(REMOVING_FILE), b"").is_err() {
        return false;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return false;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let name = entry.file_name();
        if name == OWNER_FILE || name == REMOVING_FILE {
            continue;
        }
        let path = entry.path();
        let removed = match entry.file_type() {
            Ok(kind) if kind.is_dir() => with_retries(|| fs::remove_dir_all(&path)),
            Ok(_) => with_retries(|| fs::remove_file(&path)),
            Err(_) => false,
        };
        if !removed {
            return false;
        }
    }
    with_retries(|| fs::remove_file(directory.join(OWNER_FILE)))
        && with_retries(|| fs::remove_file(directory.join(REMOVING_FILE)))
        && with_retries(|| fs::remove_dir(directory))
}

/// A removal retried with a short, growing pause (about a second in all, to
/// match `remove_test_directory`), for a handle a scanner or indexer is
/// still closing. Already gone is done.
fn with_retries(mut remove: impl FnMut() -> io::Result<()>) -> bool {
    const ATTEMPTS: u64 = 8;
    for attempt in 1..=ATTEMPTS {
        match remove() {
            Ok(()) => return true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return true,
            Err(_) if attempt < ATTEMPTS => std::thread::sleep(Duration::from_millis(25 * attempt)),
            Err(_) => {}
        }
    }
    false
}

/// Removes every sibling of `own` under `root` that `assess` finds dead, and
/// returns the ones removed. Never panics and never fails a test: whatever
/// cannot be read or removed is simply left.
fn sweep(
    root: &Path,
    own: &Path,
    is_dead: &dyn Fn(ProcessRecord) -> bool,
    now: SystemTime,
    quiet_period: Duration,
) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let ours = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(PREFIX));
        if !ours || path == own {
            continue;
        }
        if assess(&path, is_dead, now, quiet_period) == Verdict::Remove && remove(&path) {
            removed.push(path);
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture's own root, outside the real sweep's reach (its name does
    /// not start with `PREFIX`), removed whole when the test ends.
    struct FixtureRoot(PathBuf);

    impl Drop for FixtureRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture_root(name: &str) -> FixtureRoot {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "qc-health-sweep-fixture-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        FixtureRoot(root)
    }

    fn record(pid: u32, created_at: u64) -> ProcessRecord {
        ProcessRecord { pid, created_at }
    }

    /// A directory as `directory()` and `PendingUse` leave it: owner, users
    /// and the two files the tests write.
    fn health_directory(root: &Path, owner: ProcessRecord, users: &[&str]) -> PathBuf {
        let directory = root.join(format!("{PREFIX}{}", owner.file_name()));
        fs::create_dir_all(directory.join(USERS_DIRECTORY)).unwrap();
        fs::write(directory.join(OWNER_FILE), owner_text(owner)).unwrap();
        fs::write(directory.join("runtime-health.json"), b"{}").unwrap();
        fs::write(directory.join("broker-stderr.log"), b"log").unwrap();
        for user in users {
            fs::write(directory.join(USERS_DIRECTORY).join(user), b"").unwrap();
        }
        directory
    }

    #[test]
    fn process_records_parse_only_what_they_write() {
        assert_eq!(ProcessRecord::parse("12-345"), Some(record(12, 345)));
        assert_eq!(
            ProcessRecord::parse(&record(7, 0).file_name()),
            Some(record(7, 0))
        );
        for text in [
            "12",
            "12-",
            "-345",
            "+12-345",
            "12-+345",
            "012-345",
            "12-345-6",
            "12 -345",
            "pending-0",
            "4294967296-1",
        ] {
            assert_eq!(ProcessRecord::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_sweep_removes_only_directories_whose_every_recorded_process_is_dead() {
        let fixture = fixture_root("decisions");
        let root = &fixture.0;
        let dead = [
            record(101, 1001),
            record(201, 2001),
            record(103, 1003),
            record(104, 1004),
            record(105, 1005),
            record(106, 1006),
            record(108, 1008),
            record(109, 1009),
            record(110, 1010),
        ];
        // Everything not named dead is alive, as an unknown answer is.
        let is_dead = |candidate: ProcessRecord| dead.contains(&candidate);
        let later = SystemTime::now() + Duration::from_secs(2 * 60 * 60);

        let all_dead = health_directory(root, record(101, 1001), &["201-2001"]);
        let owner_alive = health_directory(root, record(102, 1002), &[]);
        let child_alive = health_directory(root, record(103, 1003), &["201-2001", "303-3003"]);
        let pending = health_directory(root, record(104, 1004), &["pending-0"]);
        let unparsable_owner = health_directory(root, record(105, 1005), &[]);
        fs::write(unparsable_owner.join(OWNER_FILE), b"not an owner record").unwrap();
        let mismatched_owner = health_directory(root, record(106, 1006), &[]);
        fs::write(
            mismatched_owner.join(OWNER_FILE),
            owner_text(record(106, 9)),
        )
        .unwrap();
        // An earlier build's directory: a pid-only name and no records.
        let pid_only = root.join(format!("{PREFIX}107"));
        fs::create_dir_all(&pid_only).unwrap();
        fs::write(pid_only.join("broker-stderr.log"), b"log").unwrap();
        let no_users = health_directory(root, record(108, 1008), &[]);
        fs::remove_dir(no_users.join(USERS_DIRECTORY)).unwrap();
        // A removal interrupted after its mark: users already gone.
        let interrupted = health_directory(root, record(109, 1009), &[]);
        fs::remove_dir(interrupted.join(USERS_DIRECTORY)).unwrap();
        fs::write(interrupted.join(REMOVING_FILE), b"").unwrap();
        let own = health_directory(root, record(110, 1010), &[]);
        let unrelated = root.join("unrelated-directory");
        fs::create_dir_all(&unrelated).unwrap();

        // Just written, so inside the quiet period even though all is dead.
        assert_eq!(
            assess(&all_dead, &is_dead, SystemTime::now(), QUIET_PERIOD),
            Verdict::Keep("changed too recently, or a time is unreadable")
        );
        for kept in [
            &owner_alive,
            &child_alive,
            &pending,
            &unparsable_owner,
            &mismatched_owner,
            &pid_only,
            &no_users,
        ] {
            assert!(
                matches!(
                    assess(kept, &is_dead, later, QUIET_PERIOD),
                    Verdict::Keep(reason) if !reason.is_empty()
                ),
                "{} must be kept",
                kept.display()
            );
        }

        let mut removed = sweep(root, &own, &is_dead, later, QUIET_PERIOD);
        removed.sort();
        let mut expected = vec![all_dead.clone(), interrupted.clone()];
        expected.sort();
        assert_eq!(removed, expected);
        assert!(!all_dead.exists());
        assert!(!interrupted.exists());
        for kept in [
            &owner_alive,
            &child_alive,
            &pending,
            &unparsable_owner,
            &mismatched_owner,
            &pid_only,
            &no_users,
            &own,
            &unrelated,
        ] {
            assert!(kept.exists(), "{} must be kept", kept.display());
        }
        assert!(pid_only.join("broker-stderr.log").exists());
        assert!(child_alive.join("broker-stderr.log").exists());
    }

    #[test]
    fn a_live_process_is_never_read_as_dead() {
        let pid = std::process::id();
        let Some(created_at) = qc_native_platform::process::os_process_created_at(pid) else {
            // No creation time on this OS: nothing is ever recorded or swept.
            return;
        };
        assert!(!recorded_process_is_dead(record(pid, created_at)));
        // The same pid created at another time is a reused pid: the recorded
        // process is gone.
        assert!(recorded_process_is_dead(record(pid, created_at ^ 1)));
    }

    #[test]
    fn a_recorded_spawn_replaces_its_pending_file() {
        let users = directory().join(USERS_DIRECTORY);
        if !users.is_dir() {
            // An OS without creation times keeps no records.
            return;
        }
        let pid = std::process::id();
        let Some(created_at) = qc_native_platform::process::os_process_created_at(pid) else {
            return;
        };
        let pending = PendingUse::begin();
        let pending_path = pending.pending.clone().expect("pending file written");
        assert!(pending_path.is_file());
        pending.record(pid);
        assert!(!pending_path.exists());
        assert!(users.join(record(pid, created_at).file_name()).is_file());

        let failed_spawn = PendingUse::begin();
        let failed_path = failed_spawn.pending.clone().expect("pending file written");
        drop(failed_spawn);
        assert!(!failed_path.exists());
    }
}
