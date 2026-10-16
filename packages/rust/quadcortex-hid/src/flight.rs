use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const MAX_ENTRIES: usize = 256;
const MAX_INCIDENTS: usize = 4;
const MAX_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 512;

fn bounded_text(text: String) -> String {
    let mut end = text.len().min(MAX_TEXT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn read_document(path: &Path) -> Option<FlightDocument> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return None;
    }
    let mut document: FlightDocument = serde_json::from_slice(&bytes).ok()?;
    while document.entries.len() > MAX_ENTRIES {
        document.entries.pop_front();
    }
    while document.incidents.len() > MAX_INCIDENTS {
        document.incidents.pop_front();
    }
    for entries in std::iter::once(&mut document.entries).chain(
        document
            .incidents
            .iter_mut()
            .map(|incident| &mut incident.entries),
    ) {
        while entries.len() > MAX_ENTRIES {
            entries.pop_front();
        }
        for entry in entries {
            entry.event = bounded_text(std::mem::take(&mut entry.event));
        }
    }
    for incident in &mut document.incidents {
        incident.reason = bounded_text(std::mem::take(&mut incident.reason));
    }
    Some(document)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FlightEntry {
    at_unix_ms: u128,
    #[serde(default)]
    process_id: u32,
    event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_type: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    report_count: Option<usize>,
    /// Set once this entry has absorbed at least one repeat of the same
    /// `(event, message_type)` pair that immediately preceded it. Absent
    /// (rather than `Some(1)`) for an ordinary, unrepeated entry so most
    /// entries keep their existing shape on disk.
    #[serde(skip_serializing_if = "Option::is_none")]
    repeat_count: Option<u32>,
    /// The timestamp of the first occurrence in a collapsed run; `at_unix_ms`
    /// tracks the most recent one. Only present alongside `repeat_count`.
    #[serde(skip_serializing_if = "Option::is_none")]
    first_at_unix_ms: Option<u128>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FlightDocument {
    version: u8,
    entries: VecDeque<FlightEntry>,
    /// Frozen pre-failure windows. The live ring is intentionally allowed to
    /// keep moving during reconnect, but reconnect traffic must not erase the
    /// operation that caused recovery to start.
    #[serde(default, skip_serializing_if = "VecDeque::is_empty")]
    incidents: VecDeque<FlightIncident>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FlightIncident {
    at_unix_ms: u128,
    reason: String,
    entries: VecDeque<FlightEntry>,
}

/// A bounded, payload-free record of the messages most recently sent to the QC.
/// It is persisted after every entry so a device-side hang does not erase the
/// evidence needed to identify the final operation.
pub struct FlightRecorder {
    path: Option<PathBuf>,
    document: FlightDocument,
    last_persisted_at: Instant,
}

impl FlightRecorder {
    pub fn open_default() -> Self {
        if let Some(path) = std::env::var_os("QC_FLIGHT_RECORDER_PATH").map(PathBuf::from) {
            return Self::open(Some(path));
        }
        #[cfg(windows)]
        {
            let Some(root) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
                return Self::open(None);
            };
            let path = root.join("QC Remote").join("device-flight-recorder.json");
            let legacy_path = root.join("QC Control").join("device-flight-recorder.json");
            Self::open_with_fallback(Some(path), Some(legacy_path))
        }
        // Linux and macOS diagnostics persistence is opt-in through the
        // explicit path above. The broker's credential/runtime storage has
        // its own secure path service; this recorder must not invent another.
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        Self::open(None)
    }

    fn open(path: Option<PathBuf>) -> Self {
        Self::open_with_fallback(path, None)
    }

    fn open_with_fallback(path: Option<PathBuf>, fallback: Option<PathBuf>) -> Self {
        let mut document = path
            .as_deref()
            .and_then(read_document)
            .or_else(|| fallback.as_deref().and_then(read_document))
            .unwrap_or_else(|| FlightDocument {
                version: 2,
                entries: VecDeque::new(),
                incidents: VecDeque::new(),
            });
        document.version = 2;
        Self {
            path,
            document,
            last_persisted_at: Instant::now(),
        }
    }

    pub fn event(&mut self, event: impl Into<String>) {
        self.push(FlightEntry {
            at_unix_ms: now_unix_ms(),
            process_id: std::process::id(),
            event: bounded_text(event.into()),
            message_type: None,
            report_count: None,
            repeat_count: None,
            first_at_unix_ms: None,
        });
    }

    pub fn outbound(&mut self, message_type: u16, report_count: usize) {
        self.push(FlightEntry {
            at_unix_ms: now_unix_ms(),
            process_id: std::process::id(),
            event: "outbound".into(),
            message_type: Some(message_type),
            report_count: Some(report_count),
            repeat_count: None,
            first_at_unix_ms: None,
        });
    }

    pub fn inbound(&mut self, message_type: u16, report_count: usize) {
        self.push(FlightEntry {
            at_unix_ms: now_unix_ms(),
            process_id: std::process::id(),
            event: "inbound".into(),
            message_type: Some(message_type),
            report_count: Some(report_count),
            repeat_count: None,
            first_at_unix_ms: None,
        });
    }

    /// Freeze the current payload-free timeline and flush it immediately.
    /// Unlike ordinary checkpoints this deliberately bypasses the one-second
    /// write throttle: a process or device failure is exactly when orderly
    /// Drop cannot be relied upon.
    pub fn incident(&mut self, reason: impl Into<String>) {
        let reason = bounded_text(reason.into());
        let at_unix_ms = now_unix_ms();
        self.event(format!("incident-{reason}"));
        self.document.incidents.push_back(FlightIncident {
            at_unix_ms,
            reason,
            entries: self.document.entries.clone(),
        });
        while self.document.incidents.len() > MAX_INCIDENTS {
            self.document.incidents.pop_front();
        }
        self.persist();
    }

    fn push(&mut self, entry: FlightEntry) {
        // A 256-entry buffer of one preset-folder refresh - hundreds of
        // identical `inbound messageType 4` reports back to back - used to
        // fill the whole record and evict every keepalive, handshake, and
        // tempo entry that would explain what the broker was doing before it.
        // Collapsing a run of consecutive, identically-typed traffic entries
        // into the one already on the back of the deque bounds any burst,
        // however long, to a single slot: the eviction loop below then only
        // ever has to make room for genuinely distinct evidence.
        if entry.message_type.is_some() {
            if let Some(last) = self.document.entries.back_mut() {
                if last.event == entry.event && last.message_type == entry.message_type {
                    let repeats = last.repeat_count.unwrap_or(1).saturating_add(1);
                    last.first_at_unix_ms.get_or_insert(last.at_unix_ms);
                    last.at_unix_ms = entry.at_unix_ms;
                    last.repeat_count = Some(repeats);
                    last.report_count = match (last.report_count, entry.report_count) {
                        (Some(total), Some(added)) => Some(total.saturating_add(added)),
                        (total, added) => total.or(added),
                    };
                    self.maybe_persist();
                    return;
                }
            }
        }
        self.document.entries.push_back(entry);
        while self.document.entries.len() > MAX_ENTRIES {
            // A healthy idle link emits routine KeepAlive traffic. Evicting
            // oldest-first let eight idle minutes overwrite the whole record, so a hang
            // investigated afterwards had no trace of the operation that hung.
            // Drop the oldest routine keepalive before any real operation.
            //
            // The search deliberately stops one short of the back. Including
            // the entry just pushed let a keepalive arriving at an already-full
            // record select *itself*: it was appended and removed in the same
            // call, so keepalives were structurally unrecordable under load. A
            // real ten-minute capture read as 256 entries with zero
            // messageType 32 while the broker was keeping alive at 0.994 Hz,
            // and the record was believed over the broker's own counters.
            // Excluding the newest entry costs one slot, once: the keepalive
            // that survives is then the one evicted by the next keepalive.
            let searchable = self.document.entries.len() - 1;
            let evict = self
                .document
                .entries
                .iter()
                .take(searchable)
                .position(is_routine_keepalive)
                .unwrap_or(0);
            self.document.entries.remove(evict);
        }
        self.maybe_persist();
    }

    fn maybe_persist(&mut self) {
        // Rewriting the bounded JSON document synchronously for every 129-byte
        // HID report adds hundreds of milliseconds to the initialization
        // burst. Keep recording in memory on the realtime path and checkpoint
        // at most once per second; Drop performs a final flush on orderly
        // shutdown.
        if self.last_persisted_at.elapsed() >= Duration::from_secs(1) {
            self.persist();
        }
    }

    fn persist(&mut self) -> bool {
        self.persist_with(qc_owner_only_file::publish_atomically)
    }

    // A failed checkpoint must preserve both the previous complete document
    // and retry eligibility. The publisher owns exclusive staging and atomic
    // replacement; this seam tests the recorder's behavior without pretending
    // to inject an OS partial-write fault.
    fn persist_with(&mut self, publish: impl FnOnce(&Path, &[u8]) -> Result<(), String>) -> bool {
        let Some(path) = self.path.as_deref() else {
            return false;
        };
        if let Some(parent) = path.parent() {
            if fs::create_dir_all(parent).is_err() {
                return false;
            }
        }
        let Ok(document) = serde_json::to_vec_pretty(&self.document) else {
            return false;
        };
        if publish(path, &document).is_err() {
            return false;
        }
        self.last_persisted_at = Instant::now();
        true
    }

    #[cfg(test)]
    fn for_test() -> Self {
        Self::open(None)
    }
}

impl Drop for FlightRecorder {
    fn drop(&mut self) {
        self.persist();
    }
}

/// True for dedicated idle KeepAlive traffic.
///
/// These are the highest-volume, lowest-value entries in the record: they are
/// by definition the "nothing happened" case. They stay recorded so an idle
/// stretch is still visible, but they are the first thing evicted.
fn is_routine_keepalive(entry: &FlightEntry) -> bool {
    matches!(entry.event.as_str(), "outbound" | "inbound")
        && entry.message_type == Some(quadcortex::profile::MESSAGE_TYPE_KEEP_ALIVE)
}

fn now_unix_ms() -> u128 {
    quadcortex::time::unix_time_ms() as u128
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            for _ in 0..64 {
                let directory = std::env::temp_dir().join(format!(
                    "qc-flight-review-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&directory) {
                    Ok(()) => return Self(directory),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("reserve flight fixture: {error}"),
                }
            }
            panic!("reserve fresh flight fixture");
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn restored_records_are_bounded_and_counter_saturation_cannot_panic() {
        let scratch = Scratch::new();
        let path = scratch.0.join("flight.json");
        let entry = FlightEntry {
            at_unix_ms: 1,
            process_id: 1,
            event: "inbound".into(),
            message_type: Some(42),
            report_count: Some(usize::MAX),
            repeat_count: Some(u32::MAX),
            first_at_unix_ms: Some(1),
        };
        let entries: VecDeque<_> = std::iter::repeat_n(entry, MAX_ENTRIES + 1).collect();
        let document = FlightDocument {
            version: 2,
            entries: entries.clone(),
            incidents: (0..MAX_INCIDENTS + 1)
                .map(|_| FlightIncident {
                    at_unix_ms: 1,
                    reason: "é".repeat(MAX_TEXT_BYTES),
                    entries: entries.clone(),
                })
                .collect(),
        };
        fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let mut restored = FlightRecorder::open(Some(path));
        assert_eq!(restored.document.entries.len(), MAX_ENTRIES);
        assert_eq!(restored.document.incidents.len(), MAX_INCIDENTS);
        assert!(restored
            .document
            .incidents
            .iter()
            .all(|incident| incident.entries.len() == MAX_ENTRIES
                && incident.reason.len() <= MAX_TEXT_BYTES));
        restored.inbound(42, 1);
        let last = restored.document.entries.back().unwrap();
        assert_eq!(last.repeat_count, Some(u32::MAX));
        assert_eq!(last.report_count, Some(usize::MAX));
        restored.event("é".repeat(MAX_TEXT_BYTES));
        assert_eq!(
            restored.document.entries.back().unwrap().event.len(),
            MAX_TEXT_BYTES
        );
        let mut oversized = String::with_capacity(1024 * 1024);
        oversized.push_str("event");
        restored.event(oversized);
        let retained = &restored.document.entries.back().unwrap().event;
        assert_eq!(retained, "event");
        assert!(retained.capacity() <= MAX_TEXT_BYTES);
    }

    #[test]
    fn failed_checkpoint_preserves_incidents_and_retry_success_reopens_the_complete_history() {
        let scratch = Scratch::new();
        let path = scratch.0.join("flight.json");
        let mut recorder = FlightRecorder::open(Some(path.clone()));
        recorder.incident("prior-incident");
        let previous = fs::read(&path).unwrap();
        recorder.event("later-event");
        recorder.last_persisted_at = Instant::now() - Duration::from_secs(2);
        let retry_at = recorder.last_persisted_at;
        assert!(!recorder.persist_with(|_, _| Err("injected publication failure".into())));
        assert_eq!(fs::read(&path).unwrap(), previous);
        assert_eq!(recorder.last_persisted_at, retry_at);
        assert!(recorder.last_persisted_at.elapsed() >= Duration::from_secs(1));
        assert!(recorder.persist_with(qc_owner_only_file::publish_atomically));
        assert!(recorder.last_persisted_at > retry_at);
        let reopened = FlightRecorder::open(Some(path));
        assert_eq!(reopened.document.incidents.len(), 1);
        assert_eq!(reopened.document.incidents[0].reason, "prior-incident");
        assert!(reopened
            .document
            .entries
            .iter()
            .any(|entry| entry.event == "later-event"));
        assert_eq!(
            fs::read_dir(&scratch.0).unwrap().count(),
            1,
            "no stage remains"
        );
    }

    #[test]
    fn oversized_persisted_record_is_not_loaded() {
        let scratch = Scratch::new();
        let path = scratch.0.join("oversized.json");
        fs::File::create(&path)
            .unwrap()
            .set_len(MAX_DOCUMENT_BYTES + 1)
            .unwrap();
        assert!(read_document(&path).is_none());
        let restored = FlightRecorder::open(Some(path));
        assert!(restored.document.entries.is_empty());
        assert!(restored.document.incidents.is_empty());
    }

    #[test]
    fn recorder_is_bounded_and_contains_no_payload_field() {
        let mut recorder = FlightRecorder::for_test();
        for message_type in 0..(MAX_ENTRIES + 20) {
            recorder.outbound(message_type as u16 + 1_000, 1);
        }
        assert_eq!(recorder.document.entries.len(), MAX_ENTRIES);
        assert!(recorder
            .document
            .entries
            .iter()
            .all(|entry| entry.process_id > 0));
        let json = serde_json::to_value(&recorder.document).unwrap();
        assert!(json.to_string().find("payload").is_none());
        assert_eq!(json["entries"][0]["messageType"], 1_020);
    }

    #[test]
    fn recorder_captures_inbound_frame_size_without_payload_data() {
        let mut recorder = FlightRecorder::for_test();
        recorder.inbound(40, 1191);
        let json = serde_json::to_value(&recorder.document).unwrap();
        assert_eq!(json["entries"][0]["event"], "inbound");
        assert_eq!(json["entries"][0]["messageType"], 40);
        assert_eq!(json["entries"][0]["reportCount"], 1191);
        assert!(json.to_string().find("payload").is_none());
    }

    #[test]
    fn idle_keepalive_traffic_never_evicts_a_real_operation() {
        let mut recorder = FlightRecorder::for_test();
        recorder.event("handshake-reply");
        recorder.outbound(15, 1);
        recorder.inbound(40, 1191);

        // A healthy idle link produces routine KeepAlive traffic. Eight minutes
        // of it must not overwrite the operation that explains a later hang.
        for _ in 0..(MAX_ENTRIES * 2) {
            recorder.outbound(quadcortex::profile::MESSAGE_TYPE_KEEP_ALIVE, 1);
            recorder.inbound(quadcortex::profile::MESSAGE_TYPE_KEEP_ALIVE, 1);
        }

        assert_eq!(recorder.document.entries.len(), MAX_ENTRIES);
        let events: Vec<&str> = recorder
            .document
            .entries
            .iter()
            .map(|entry| entry.event.as_str())
            .collect();
        assert!(events.contains(&"handshake-reply"));
        assert!(recorder
            .document
            .entries
            .iter()
            .any(|entry| entry.message_type == Some(15)));
        assert!(recorder
            .document
            .entries
            .iter()
            .any(|entry| entry.message_type == Some(40) && entry.report_count == Some(1191)));
    }

    #[test]
    fn a_full_record_still_witnesses_that_keepalives_are_flowing() {
        // Regression: the eviction search included the entry that had just
        // been pushed, so a keepalive arriving at a full record selected
        // *itself* for eviction and vanished in the same call. The record
        // therefore read as "the broker sent nothing", which is the one
        // conclusion it must never support when the broker is in fact healthy.
        let mut recorder = FlightRecorder::for_test();
        for message_type in 0..MAX_ENTRIES {
            recorder.outbound(message_type as u16 % 9 + 20, 1);
        }
        assert_eq!(recorder.document.entries.len(), MAX_ENTRIES);

        for _ in 0..MAX_ENTRIES {
            recorder.outbound(quadcortex::profile::MESSAGE_TYPE_KEEP_ALIVE, 1);
            recorder.inbound(quadcortex::profile::MESSAGE_TYPE_KEEP_ALIVE, 3);
        }

        assert_eq!(recorder.document.entries.len(), MAX_ENTRIES);
        let keepalives: Vec<&FlightEntry> = recorder
            .document
            .entries
            .iter()
            .filter(|entry| is_routine_keepalive(entry))
            .collect();
        assert!(
            !keepalives.is_empty(),
            "a full record under sustained keepalive traffic must still show the link is alive"
        );
        // The surviving keepalive has to be a recent one, not a fossil: its
        // timestamp is what tells a reader the link was alive seconds ago.
        let newest = recorder
            .document
            .entries
            .back()
            .expect("the record is not empty")
            .at_unix_ms;
        assert!(
            keepalives
                .iter()
                .any(|entry| entry.at_unix_ms >= newest.saturating_sub(1_000)),
            "the retained keepalive must be a current one"
        );
    }

    #[test]
    fn routine_keepalive_entries_are_evicted_before_operations() {
        let mut recorder = FlightRecorder::for_test();
        for message_type in 0..MAX_ENTRIES {
            recorder.outbound(message_type as u16 % 9 + 20, 1);
        }
        for _ in 0..MAX_ENTRIES {
            recorder.outbound(quadcortex::profile::MESSAGE_TYPE_KEEP_ALIVE, 1);
            recorder.inbound(quadcortex::profile::MESSAGE_TYPE_KEEP_ALIVE, 3);
        }

        assert_eq!(recorder.document.entries.len(), MAX_ENTRIES);
        // This assertion used to demand *zero* surviving keepalives, which the
        // buggy eviction delivered by removing each keepalive in the very call
        // that pushed it. That is not the property worth holding: it made the
        // record unable to distinguish a link that is keeping alive from one
        // that has gone silent. What must hold is that keepalives are evicted
        // in preference to operations and cannot accumulate - 512 keepalive
        // entries against a full record cost a bounded handful of slots, not
        // the record.
        let keepalive_entries = recorder
            .document
            .entries
            .iter()
            .filter(|entry| {
                entry.message_type == Some(quadcortex::profile::MESSAGE_TYPE_KEEP_ALIVE)
            })
            .count();
        assert!(
            keepalive_entries <= 2,
            "idle KeepAlive traffic must never displace real operations: {keepalive_entries} of {MAX_ENTRIES} slots went to keepalives"
        );
        let operations = MAX_ENTRIES - keepalive_entries;
        assert!(
            operations >= MAX_ENTRIES - 2,
            "only {operations} real operations survived 512 keepalive entries"
        );
    }

    #[test]
    fn a_repetitive_burst_does_not_crowd_out_low_volume_evidence() {
        // Regression: a single repeated burst (e.g. one preset-folder catalog
        // refresh, messageType 4) could fill the whole record and evict every
        // keepalive, handshake, and tempo entry that would explain what the
        // broker was doing beforehand. Collapsing a run of
        // identical consecutive entries into one, with a repeat count, keeps
        // a burst of any length from consuming more than a single slot.
        let mut recorder = FlightRecorder::for_test();
        recorder.event("handshake-reply");
        recorder.outbound(quadcortex::profile::MESSAGE_TYPE_KEEP_ALIVE, 1);
        recorder.event("tempo-clock-30");
        let burst_size = MAX_ENTRIES * 4;
        for _ in 0..burst_size {
            recorder.inbound(4, 128);
        }

        assert!(
            recorder.document.entries.len() < 5,
            "a single repeated burst must collapse instead of filling the buffer: {} entries",
            recorder.document.entries.len()
        );
        let events: Vec<&str> = recorder
            .document
            .entries
            .iter()
            .map(|entry| entry.event.as_str())
            .collect();
        assert!(events.contains(&"handshake-reply"), "{events:?}");
        assert!(events.contains(&"tempo-clock-30"), "{events:?}");

        let burst = recorder
            .document
            .entries
            .iter()
            .find(|entry| entry.message_type == Some(4))
            .expect("the burst is still represented by one entry");
        assert_eq!(burst.repeat_count, Some(burst_size as u32));
        assert_eq!(burst.report_count, Some(burst_size * 128));
        assert!(burst.first_at_unix_ms.is_some());

        let json = serde_json::to_value(&recorder.document).unwrap();
        assert!(json.to_string().find("payload").is_none());
    }

    #[test]
    fn recorder_flushes_on_drop() {
        let scratch = Scratch::new();
        let path = scratch.0.join("flight.json");
        {
            let mut recorder = FlightRecorder::open(Some(path.clone()));
            recorder.event("physical-transfer-test");
            recorder.inbound(40, 1191);
        }
        let document: FlightDocument =
            serde_json::from_slice(&fs::read(&path).expect("flight record persisted"))
                .expect("flight record is valid JSON");
        assert_eq!(document.entries.len(), 2);
        assert_eq!(document.entries[1].message_type, Some(40));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn incident_freezes_evidence_while_live_ring_keeps_moving() {
        let mut recorder = FlightRecorder::for_test();
        recorder.event("rpc-start-41-device.removeBlock");
        recorder.outbound(24, 1);
        recorder.incident("ambiguous-write");

        for message_type in 0..(MAX_ENTRIES + 20) {
            recorder.outbound(message_type as u16 + 1_000, 1);
        }

        let incident = recorder
            .document
            .incidents
            .back()
            .expect("incident retained");
        assert_eq!(incident.reason, "ambiguous-write");
        assert!(incident
            .entries
            .iter()
            .any(|entry| entry.event == "rpc-start-41-device.removeBlock"));
        assert!(incident
            .entries
            .iter()
            .any(|entry| entry.event == "incident-ambiguous-write"));
        assert!(serde_json::to_string(&recorder.document)
            .unwrap()
            .find("payload")
            .is_none());
    }

    #[test]
    fn incident_history_is_bounded() {
        let mut recorder = FlightRecorder::for_test();
        for index in 0..(MAX_INCIDENTS + 3) {
            recorder.incident(format!("failure-{index}"));
        }
        assert_eq!(recorder.document.incidents.len(), MAX_INCIDENTS);
        assert_eq!(
            recorder.document.incidents.front().unwrap().reason,
            "failure-3"
        );
    }
}
