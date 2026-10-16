//! The durable record of a user's Quit (after a Quit, only the user brings QC
//! Remote back).
//!
//! This broker writes it, bound to its own instance id, exactly when a user's
//! Quit commits: the icon's Quit ([`crate::rpc::BrokerHandle::quit_by_user`])
//! or `--control quit`, forced or not, with the tray switch on or off. A
//! failed write refuses the Quit and leaves the device open, so a Quit that
//! was not recorded is never reported as one.
//!
//! While it stands, nothing autonomous may start a broker or a background MCP
//! host for this directory: an AI client's tool call, a bridge, an open
//! window's automatic connect or timer, a gateway's recovery. Attaching to a
//! broker that is already live under another instance id is not a start and
//! stays allowed. Only a user action clears it: a GUI Connect, USB or device
//! click, an actual app launch, the sign-in resident entry (`--resident`), a
//! Settings switch turned on, or `--control start`. Automatic broker child
//! startup, the app's startup reconcile and bridge hosting never clear it.
//!
//! Every write, compare-and-remove and clear holds the election lock
//! (`<descriptor>.election`, [`crate::resident::election_path`]): the one
//! cross-process lock every app window, bridge and `--resident` start already
//! takes before it may spawn a broker, and that the app's spawn admission
//! rechecks this record under. A record therefore never changes in the middle
//! of an election, and two record operations never interleave. The lock is
//! taken for the file operation alone, bounded by [`LOCK_BUDGET`]; a lock that
//! cannot be had is an explicit failure, never a silent skip.
//!
//! It carries no time. Ordering is by broker instance only, never by clock: a
//! live broker under another instance id was started after this Quit (the
//! quit broker unpublished before it exited), one under the recorded instance
//! id is the quit broker still on its way out. The app reads the same file
//! (`apps/windows/src-tauri/src/device_broker_descriptor.rs`); the wire test
//! below pins the bytes both sides agree on.
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// `device-broker.json` -> `device-broker.user-quit.json`.
pub const USER_QUIT_EXTENSION: &str = "user-quit.json";

/// How long a record operation waits for the election lock. An election in
/// progress holds it until its broker publishes; nobody elects while a live
/// broker answers, so a Quit rarely waits at all.
pub const LOCK_BUDGET: Duration = Duration::from_secs(2);
const MAX_RECORD_BYTES: u64 = 8 * 1024;

/// Which user action quit the broker. Informational: every source stands the
/// same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UserQuitSource {
    /// The icon's Quit (`MenuAction::ReleaseAndExit`).
    Tray,
    /// `qc-device-broker --control quit --confirm`.
    Control,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserQuitRecord {
    /// The broker generation the user quit.
    #[serde(rename = "instanceId")]
    pub instance_id: String,
    pub source: UserQuitSource,
}

/// Where the record for the broker published at `descriptor` lives.
pub fn path_for(descriptor: &Path) -> PathBuf {
    descriptor.with_extension(USER_QUIT_EXTENSION)
}

/// Run `operation` holding the election lock for `descriptor`, waiting at
/// most `budget` for it. The claim is the same exclusive create-new claim a
/// `--resident` start's election takes, and excludes the app's election
/// (create-new on the same file). An abandoned lock is not reaped here; the
/// elections that own that policy do it.
pub fn with_election_lock<T>(
    descriptor: &Path,
    budget: Duration,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let election = crate::resident::election_path(descriptor);
    let deadline = Instant::now() + budget;
    let claim = loop {
        match qc_owner_only_file::ClaimedFile::try_claim(&election) {
            Ok(claim) => break claim,
            Err(error) if Instant::now() >= deadline => {
                return Err(format!(
                    "the election lock at {} could not be taken ({error})",
                    election.display()
                ))
            }
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let outcome = operation();
    let released = claim.release();
    match (outcome, released) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(error)) => Err(format!(
            "the election lock at {} could not be released: {error}",
            election.display()
        )),
        (Err(error), _) => Err(error),
    }
}

/// Publish the record atomically and owner-only, under the election lock.
pub fn publish(descriptor: &Path, record: &UserQuitRecord) -> Result<(), String> {
    publish_within(descriptor, record, LOCK_BUDGET)
}

pub fn publish_within(
    descriptor: &Path,
    record: &UserQuitRecord,
    budget: Duration,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(record).expect("a user Quit record serializes");
    with_election_lock(descriptor, budget, || {
        qc_owner_only_file::publish_atomically(&path_for(descriptor), &bytes)
    })
    .map_err(|error| {
        format!(
            "Could not record the Quit, so QC Remote was not quit and the device was left open: {error}"
        )
    })
}

/// The record, if any. `Some(Err(_))` is a record that cannot be read: it
/// still stands (fail closed), it just names no instance.
pub fn read(descriptor: &Path) -> Option<Result<UserQuitRecord, String>> {
    let bounded_read = || -> io::Result<Vec<u8>> {
        let file = fs::File::open(path_for(descriptor))?;
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "User Quit record exceeds 8 KiB",
            ));
        }
        Ok(bytes)
    };
    match bounded_read() {
        Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(|error| error.to_string())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => Some(Err(error.to_string())),
    }
}

/// What [`withdraw`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Withdrawal {
    /// The record named `instance_id` and is gone.
    Withdrawn,
    /// No record, or another generation's (a later Quit): left exactly as it
    /// is.
    NotOurs,
}

/// Compare-and-remove, under the election lock: remove the record a Quit that
/// did not commit wrote, only while it still names `instance_id`. A record
/// that was replaced (another generation's later Quit) or cannot be read is
/// never ours to remove. `Err` says the record may still stand.
pub fn withdraw(descriptor: &Path, instance_id: &str) -> Result<Withdrawal, String> {
    withdraw_within(descriptor, instance_id, LOCK_BUDGET)
}

pub fn withdraw_within(
    descriptor: &Path,
    instance_id: &str,
    budget: Duration,
) -> Result<Withdrawal, String> {
    with_election_lock(descriptor, budget, || match read(descriptor) {
        Some(Ok(record)) if record.instance_id == instance_id => {
            match fs::remove_file(path_for(descriptor)) {
                Ok(()) => Ok(Withdrawal::Withdrawn),
                Err(error) => Err(format!("the record could not be removed: {error}")),
            }
        }
        _ => Ok(Withdrawal::NotOurs),
    })
    .map_err(|error| format!("Could not withdraw the Quit record, so it may still stand: {error}"))
}

/// A user action superseded the Quit, under the election lock. `Ok(true)`
/// when a record was removed, `Ok(false)` when none stood. Each explicit start
/// calls this at exactly one point.
pub fn clear(descriptor: &Path) -> Result<bool, String> {
    clear_within(descriptor, LOCK_BUDGET)
}

pub fn clear_within(descriptor: &Path, budget: Duration) -> Result<bool, String> {
    with_election_lock(descriptor, budget, || {
        match fs::remove_file(path_for(descriptor)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    })
    .map_err(|error| format!("Could not clear the recorded Quit: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test's own scratch directory: a fresh random name, created
    /// exclusively, removed when the test ends or unwinds.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let directory = std::env::temp_dir().join(format!(
                "qc-user-quit-{label}-{:016x}",
                rand::random::<u64>()
            ));
            fs::create_dir(&directory).expect("a scratch directory of this test's own");
            Self(directory)
        }
        fn descriptor(&self) -> PathBuf {
            self.0.join("device-broker.json")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn record(instance: &str) -> UserQuitRecord {
        UserQuitRecord {
            instance_id: instance.into(),
            source: UserQuitSource::Tray,
        }
    }

    /// The bytes the app's reader (`device_broker_descriptor.rs`) pins too.
    #[test]
    fn the_record_wire_and_path_are_pinned() {
        assert_eq!(
            serde_json::to_string(&record("generation-1")).unwrap(),
            r#"{"instanceId":"generation-1","source":"tray"}"#
        );
        assert_eq!(
            serde_json::to_value(UserQuitSource::Control).unwrap(),
            "control"
        );
        assert_eq!(
            path_for(Path::new("data/device-broker.json")),
            PathBuf::from("data/device-broker.user-quit.json")
        );
    }

    #[test]
    fn publish_read_withdraw_and_clear() {
        let scratch = Scratch::new("cycle");
        let descriptor = scratch.descriptor();
        assert_eq!(read(&descriptor), None);
        assert_eq!(clear(&descriptor), Ok(false));
        publish(&descriptor, &record("quit")).unwrap();
        assert!(qc_owner_only_file::is_owner_only(&path_for(&descriptor)).unwrap());
        assert_eq!(read(&descriptor), Some(Ok(record("quit"))));
        assert_eq!(withdraw(&descriptor, "another"), Ok(Withdrawal::NotOurs));
        assert_eq!(read(&descriptor), Some(Ok(record("quit"))), "not ours");
        assert_eq!(withdraw(&descriptor, "quit"), Ok(Withdrawal::Withdrawn));
        assert_eq!(read(&descriptor), None);
        fs::write(path_for(&descriptor), b"not a record").unwrap();
        assert!(
            matches!(read(&descriptor), Some(Err(_))),
            "unreadable stands"
        );
        assert_eq!(
            withdraw(&descriptor, "quit"),
            Ok(Withdrawal::NotOurs),
            "an unreadable record is never withdrawn"
        );
        assert_eq!(clear(&descriptor), Ok(true));
        assert_eq!(read(&descriptor), None);
        assert!(
            !crate::resident::election_path(&descriptor).exists(),
            "every operation released the election lock"
        );
    }

    #[test]
    fn oversized_quit_record_still_stands_and_cannot_be_withdrawn() {
        let scratch = Scratch::new("oversized");
        let descriptor = scratch.descriptor();
        let path = path_for(&descriptor);
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_RECORD_BYTES + 1).unwrap();
        drop(file);
        assert!(matches!(read(&descriptor), Some(Err(_))));
        assert_eq!(withdraw(&descriptor, "quit"), Ok(Withdrawal::NotOurs));
        assert_eq!(fs::metadata(&path).unwrap().len(), MAX_RECORD_BYTES + 1);
        assert_eq!(clear(&descriptor), Ok(true));
    }

    /// A withdrawal compares and removes in one step under the lock: a record
    /// a later Quit replaced it with survives.
    #[test]
    fn a_withdrawal_never_removes_a_newer_quit() {
        let scratch = Scratch::new("withdraw-newer");
        let descriptor = scratch.descriptor();
        publish(&descriptor, &record("older")).unwrap();
        publish(&descriptor, &record("newer")).unwrap();
        assert_eq!(withdraw(&descriptor, "older"), Ok(Withdrawal::NotOurs));
        assert_eq!(read(&descriptor), Some(Ok(record("newer"))));
    }

    /// Every record operation waits for the election lock and fails
    /// explicitly when it cannot have it; the record is left unchanged.
    #[test]
    fn record_operations_hold_the_election_lock_and_fail_when_it_is_held() {
        let scratch = Scratch::new("locked");
        let descriptor = scratch.descriptor();
        publish(&descriptor, &record("quit")).unwrap();
        let held = qc_owner_only_file::ClaimedFile::try_claim(&crate::resident::election_path(
            &descriptor,
        ))
        .expect("this test holds the election");
        let short = Duration::from_millis(60);
        let error = publish_within(&descriptor, &record("other"), short).unwrap_err();
        assert!(error.contains("was not quit"), "{error}");
        assert!(error.contains("election lock"), "{error}");
        assert!(clear_within(&descriptor, short)
            .unwrap_err()
            .contains("Could not clear"));
        assert!(withdraw_within(&descriptor, "quit", short)
            .unwrap_err()
            .contains("may still stand"));
        assert_eq!(read(&descriptor), Some(Ok(record("quit"))), "unchanged");
        held.release().unwrap();
        assert_eq!(clear(&descriptor), Ok(true));
    }

    #[test]
    fn a_record_that_cannot_be_written_is_an_explicit_failure() {
        let scratch = Scratch::new("blocked");
        let file = scratch.0.join("not-a-directory");
        fs::write(&file, b"a file cannot hold a record").unwrap();
        let error = publish(&file.join("device-broker.json"), &record("x")).unwrap_err();
        assert!(error.contains("was not quit"), "{error}");
        assert!(error.contains("device was left open"), "{error}");
    }
}
