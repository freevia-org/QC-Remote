pub use crate::generated_broker_surface::DeviceBrokerDescriptor;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often a running broker re-stamps `published_at_unix_ms` on the descriptor.
///
/// That timestamp is the only liveness signal a descriptor carries. Readers
/// treat a copy older than `DEVICE_BROKER_DESCRIPTOR_STALE_AFTER` as belonging to a process
/// that is gone and refuse to use it, which by then may be invalid anyway.
#[allow(dead_code)]
pub const DEVICE_BROKER_DESCRIPTOR_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// The reader-side staleness threshold.
///
/// If a descriptor's `published_at_unix_ms` is older than `now_ms - 150_000`,
/// it is treated as stale and ignored by clients.
pub const DEVICE_BROKER_DESCRIPTOR_STALE_AFTER: Duration = Duration::from_secs(150);

/// Returns the path where the broker descriptor is stored.
///
/// The descriptor lives in `app_local_data_dir/device-broker.json`.
pub fn device_broker_descriptor_path(app_local_data_dir: &Path) -> PathBuf {
    app_local_data_dir.join("device-broker.json")
}

/// Reads a descriptor from disk if it exists and is fresh.
///
/// Returns `None` if:
/// - The file is missing
/// - The file is not valid JSON
/// - The descriptor is older than `now_ms - 150_000` (stale)
pub fn read_descriptor(path: &Path, now_ms: u64) -> Option<DeviceBrokerDescriptor> {
    // Small local discovery metadata, not a firmware payload. Reuse the
    // same held-file 128 KiB envelope as the local MCP discovery reader.
    let bytes = crate::local_stdio::read_local_document(path).ok()?;
    let descriptor: DeviceBrokerDescriptor = serde_json::from_slice(&bytes).ok()?;

    // Check if descriptor is stale
    let age_ms = now_ms.saturating_sub(descriptor.published_at_unix_ms);
    if age_ms > DEVICE_BROKER_DESCRIPTOR_STALE_AFTER.as_millis() as u64 {
        return None;
    }

    Some(descriptor)
}

/// Attempts to claim the descriptor slot for writing.
///
/// This is the atomic tie-breaker for the election race. Two processes racing to become
/// the broker both call this function; the OS guarantees only one `create_new` open succeeds.
/// The loser must NOT overwrite what it read — it must re-read the winner's now-published
/// descriptor and attach as a client instead.
///
/// Returns:
/// - `Ok(file)` if this caller won the election race and now owns the descriptor file
/// - `Err(())` if someone else's file already exists (caller must fall back to reading)
pub(crate) fn try_claim_descriptor_slot(path: &Path) -> Result<fs::File, ()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| ())
}

/// Releases the descriptor slot by removing the file.
///
/// This is called by the broker on clean shutdown after its grace period expires
/// with zero clients, so the *next* launcher doesn't have to wait out the full 150s
/// staleness window.
pub fn release_descriptor_slot(path: &Path) {
    let _ = qc_owner_only_file::release_if_unclaimed(path);
}

pub use crate::broker_paths::unix_time_ms;

/// `device-broker.json` -> `device-broker.user-quit.json`: the user's Quit as
/// the broker records it (`services/device-broker/src/user_quit.rs` is the
/// only writer; this side reads and clears it). While it stands nothing
/// autonomous starts a broker or a bridge host again; see
/// `crate::user_quit_refusal`.
pub(crate) const USER_QUIT_EXTENSION: &str = "user-quit.json";

/// What this side reads of the record: the broker generation that was quit.
/// The writer's `source` is informational and ignored here.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct UserQuitRecord {
    #[serde(rename = "instanceId")]
    pub(crate) instance_id: String,
}

pub(crate) fn user_quit_path(descriptor: &Path) -> PathBuf {
    descriptor.with_extension(USER_QUIT_EXTENSION)
}

/// The record, if any. `Some(Err(_))` cannot be read: it still stands (fail
/// closed) but names no generation.
pub(crate) fn read_user_quit(descriptor: &Path) -> Option<Result<UserQuitRecord, String>> {
    match crate::local_stdio::read_local_document(&user_quit_path(descriptor)) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(|error| error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => Some(Err(error.to_string())),
    }
}

/// A user action superseded the Quit: an explicit Connect, USB or device
/// click, or an actual app launch. Never called on an automatic path. The
/// caller holds the election lock (`crate::acknowledge_user_connect_in`).
/// `Ok(true)` when a record was removed.
pub(crate) fn clear_user_quit(descriptor: &Path) -> Result<bool, String> {
    match fs::remove_file(user_quit_path(descriptor)) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Could not clear the recorded Quit: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The broker's writer pins the same bytes and path
    /// (`user_quit::tests::the_record_wire_and_path_are_pinned`).
    #[test]
    fn the_user_quit_record_reads_the_brokers_wire_and_clears() {
        let directory = std::env::temp_dir().join(format!(
            "qc-user-quit-reader-{:032x}",
            rand::random::<u128>()
        ));
        fs::create_dir(&directory).expect("a scratch directory of this test's own");
        let _guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
        let descriptor = directory.join("device-broker.json");
        assert_eq!(
            user_quit_path(Path::new("data/device-broker.json")),
            PathBuf::from("data/device-broker.user-quit.json")
        );
        assert_eq!(read_user_quit(&descriptor), None);
        assert_eq!(clear_user_quit(&descriptor), Ok(false));
        fs::write(
            user_quit_path(&descriptor),
            br#"{"instanceId":"generation-1","source":"tray"}"#,
        )
        .unwrap();
        assert_eq!(
            read_user_quit(&descriptor),
            Some(Ok(UserQuitRecord {
                instance_id: "generation-1".into()
            }))
        );
        fs::write(user_quit_path(&descriptor), b"not a record").unwrap();
        assert!(matches!(read_user_quit(&descriptor), Some(Err(_))));
        assert_eq!(clear_user_quit(&descriptor), Ok(true));
        assert_eq!(read_user_quit(&descriptor), None);
    }

    fn write_fixture(file: &mut fs::File, descriptor: &DeviceBrokerDescriptor) {
        let wire = serde_json::json!({
            "port": descriptor.port,
            "bearerToken": descriptor.bearer_token.as_str(),
            "instanceId": descriptor.instance_id.as_str(),
            "processId": descriptor.process_id,
            "processCreatedAt": descriptor.process_created_at,
            "publishedAtUnixMs": descriptor.published_at_unix_ms,
            "gatewayApiVersion": descriptor.gateway_api_version,
        });
        std::io::Write::write_all(file, &serde_json::to_vec(&wire).unwrap()).unwrap();
    }

    /// The descriptor path in a directory of this test's own, with the guard
    /// that removes it. Bind the guard before any file taken from the
    /// directory, so it drops last.
    fn scratch_path(name: &str) -> (crate::tests::RemoveDirOnDrop, PathBuf) {
        let dir = crate::tests::unique_fixture_directory(&format!("broker-descriptor-{name}"));
        let path = dir.join("device-broker.json");
        (crate::tests::RemoveDirOnDrop::new(dir), path)
    }

    #[test]
    fn descriptor_scratch_owners_with_the_same_label_do_not_share_or_remove_each_others_bytes() {
        // Random label keeps historical baseline-owned paths isolated too.
        let label = format!("same-label-{:032x}", rand::random::<u128>());
        let (first_guard, first) = scratch_path(&label);
        let (second_guard, second) = scratch_path(&label);
        assert_ne!(
            first.parent(),
            second.parent(),
            "two owners reused one scratch directory"
        );
        fs::write(&first, b"first owner").unwrap();
        fs::write(&second, b"second owner").unwrap();
        drop(first_guard);
        assert!(!first.exists());
        assert_eq!(fs::read(&second).unwrap(), b"second owner");
        let second_root = second.parent().unwrap().to_path_buf();
        drop(second_guard);
        assert!(!second_root.exists());
    }

    #[test]
    fn discovery_and_quit_documents_enforce_the_same_local_byte_envelope() {
        let directory = std::env::temp_dir().join(format!(
            "qc-broker-document-envelope-{:032x}",
            rand::random::<u128>()
        ));
        fs::create_dir(&directory).expect("exclusive scratch directory");
        let _root = crate::tests::RemoveDirOnDrop::new(directory.clone());
        let descriptor = directory.join("device-broker.json");
        let quit = user_quit_path(&descriptor);
        let now = unix_time_ms();
        let descriptor_bytes = serde_json::to_vec(&serde_json::json!({
            "port": 1234, "bearerToken": "fixture-token", "instanceId": "generation",
            "processId": std::process::id(), "publishedAtUnixMs": now,
            "unknownFutureField": "preserved"
        }))
        .unwrap();
        let quit_bytes = br#"{"instanceId":"generation","unknownFutureField":"preserved"}"#;
        for (path, bytes) in [
            (&descriptor, descriptor_bytes.as_slice()),
            (&quit, quit_bytes.as_slice()),
        ] {
            let mut exact = bytes.to_vec();
            exact.resize(128 * 1024, b' ');
            fs::write(path, &exact).unwrap();
            if path == &descriptor {
                assert_eq!(
                    read_descriptor(path, now).unwrap().instance_id,
                    "generation"
                );
            } else {
                assert_eq!(
                    read_user_quit(&descriptor).unwrap().unwrap().instance_id,
                    "generation"
                );
            }
            exact.push(b' ');
            fs::write(path, &exact).unwrap();
            if path == &descriptor {
                assert!(
                    read_descriptor(path, now).is_none(),
                    "overbudget discovery cannot be admitted"
                );
            } else {
                assert!(
                    matches!(read_user_quit(&descriptor), Some(Err(_))),
                    "overbudget Quit still stands fail-closed, never absent"
                );
            }
            assert_eq!(
                fs::read(path).unwrap(),
                exact,
                "reader refusal preserves all original bytes"
            );
        }
    }

    #[test]
    fn a_formatted_descriptor_never_shows_the_bearer_token() {
        let descriptor = DeviceBrokerDescriptor {
            port: 54123,
            bearer_token: "s3cret-broker-token".into(),
            instance_id: "live-broker".into(),
            process_id: 42,
            process_created_at: Some(7),
            published_at_unix_ms: 1,
            gateway_api_version: Some(3),
        };
        for shown in [format!("{descriptor:?}"), format!("{descriptor:#?}")] {
            assert!(!shown.contains("s3cret-broker-token"), "{shown}");
            assert!(shown.contains("<redacted>"), "{shown}");
            assert!(shown.contains("live-broker") && shown.contains("54123"));
        }
    }

    #[test]
    fn app_cleanup_cannot_reap_a_live_broker_claim() {
        let (_guard, path) = scratch_path("protected-broker-claim");
        release_descriptor_slot(&path);
        let mut claim = qc_owner_only_file::ClaimedFile::try_claim(&path).unwrap();
        let descriptor = DeviceBrokerDescriptor {
            port: 54123,
            bearer_token: "owner-only-test-token".into(),
            instance_id: "live-broker".into(),
            process_id: std::process::id(),
            process_created_at: None,
            published_at_unix_ms: unix_time_ms(),
            gateway_api_version: None,
        };
        claim
            .write(
                &serde_json::to_vec(&serde_json::json!({
                    "port": descriptor.port,
                    "bearerToken": descriptor.bearer_token.as_str(),
                    "instanceId": descriptor.instance_id.as_str(),
                    "processId": descriptor.process_id,
                    "processCreatedAt": descriptor.process_created_at,
                    "publishedAtUnixMs": descriptor.published_at_unix_ms,
                    "gatewayApiVersion": descriptor.gateway_api_version,
                }))
                .unwrap(),
            )
            .unwrap();
        release_descriptor_slot(&path);
        assert_eq!(read_descriptor(&path, unix_time_ms()), Some(descriptor));
        drop(claim);
        release_descriptor_slot(&path);
        assert!(
            !path.exists(),
            "the same reaper releases a dead owner immediately"
        );
        qc_owner_only_file::ClaimedFile::try_claim(&path)
            .unwrap()
            .release()
            .unwrap();
    }

    #[test]
    fn read_descriptor_returns_none_when_file_is_absent() {
        let (_guard, path) = scratch_path("absent");
        assert!(read_descriptor(&path, unix_time_ms()).is_none());
    }

    #[test]
    fn write_then_read_round_trips() {
        let (_guard, path) = scratch_path("round-trip");

        let mut file = try_claim_descriptor_slot(&path).expect("first claim succeeds");
        let now = unix_time_ms();
        let descriptor = DeviceBrokerDescriptor {
            port: 54123,
            bearer_token: "test-token".into(),
            instance_id: "instance-a".into(),
            process_id: std::process::id(),
            process_created_at: Some(133_000_000_000_000_000),
            published_at_unix_ms: now,
            gateway_api_version: None,
        };
        write_fixture(&mut file, &descriptor);
        drop(file);
        let read_back = read_descriptor(&path, now).expect("descriptor is present and fresh");
        assert_eq!(read_back.port, 54123);
        assert_eq!(read_back.instance_id, "instance-a");
        assert_eq!(read_back.process_created_at, Some(133_000_000_000_000_000));
    }

    /// The broker's writer (`services/device-broker/src/rpc.rs`) serializes
    /// its own struct; this pins the wire name both sides must share.
    #[test]
    fn process_created_at_travels_as_camel_case_and_may_be_absent() {
        let with: DeviceBrokerDescriptor = serde_json::from_str(
            r#"{"port":1,"bearerToken":"t","instanceId":"i","processId":7,"processCreatedAt":133000000000000001,"publishedAtUnixMs":5}"#,
        )
        .unwrap();
        assert_eq!(with.process_created_at, Some(133_000_000_000_000_001));
        let null: DeviceBrokerDescriptor = serde_json::from_str(
            r#"{"port":1,"bearerToken":"t","instanceId":"i","processId":7,"processCreatedAt":null,"publishedAtUnixMs":5}"#,
        )
        .unwrap();
        assert_eq!(null.process_created_at, None);
        let without: DeviceBrokerDescriptor = serde_json::from_str(
            r#"{"port":1,"bearerToken":"t","instanceId":"i","processId":7,"publishedAtUnixMs":5}"#,
        )
        .unwrap();
        assert_eq!(without.process_created_at, None);
        assert_eq!(without.gateway_api_version, None);
        let versioned: DeviceBrokerDescriptor = serde_json::from_str(
            r#"{"port":1,"bearerToken":"t","instanceId":"i","processId":7,"publishedAtUnixMs":5,"gatewayApiVersion":12}"#,
        )
        .unwrap();
        assert_eq!(versioned.gateway_api_version, Some(12));
    }

    #[test]
    fn read_descriptor_treats_a_descriptor_older_than_150s_as_stale() {
        let (_guard, path) = scratch_path("stale");

        let mut file = try_claim_descriptor_slot(&path).unwrap();
        let old_timestamp = unix_time_ms() - 151_000;
        write_fixture(
            &mut file,
            &DeviceBrokerDescriptor {
                port: 1,
                bearer_token: "t".into(),
                instance_id: "i".into(),
                process_id: 1,
                process_created_at: None,
                published_at_unix_ms: old_timestamp,
                gateway_api_version: None,
            },
        );
        drop(file);
        assert!(read_descriptor(&path, unix_time_ms()).is_none());
    }

    #[test]
    fn second_claim_of_the_same_path_fails() {
        let (_guard, path) = scratch_path("race");

        let _first = try_claim_descriptor_slot(&path).expect("first claim succeeds");
        assert!(
            try_claim_descriptor_slot(&path).is_err(),
            "a second claim of the same descriptor path must fail atomically"
        );
    }

    #[test]
    fn release_removes_the_file_so_the_next_launcher_does_not_wait_out_staleness() {
        let (_guard, path) = scratch_path("release");

        let mut file = try_claim_descriptor_slot(&path).unwrap();
        write_fixture(
            &mut file,
            &DeviceBrokerDescriptor {
                port: 1,
                bearer_token: "t".into(),
                instance_id: "i".into(),
                process_id: 1,
                process_created_at: None,
                published_at_unix_ms: unix_time_ms(),
                gateway_api_version: None,
            },
        );
        drop(file);
        release_descriptor_slot(&path);
        assert!(
            try_claim_descriptor_slot(&path).is_ok(),
            "the slot must be free immediately after release"
        );
    }
}
