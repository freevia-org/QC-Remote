//! Platform-neutral lifecycle for the QC's uncorrelated LocalBackup stream.
//!
//! Hosts provide only a monotonic clock, deliver payloads, and execute the
//! returned I/O action. Replay prevention, liveness, progress, and stream
//! boundary policy belong here because they are properties of the protocol.

use crate::profile;
use crate::responses::{BackupAssembler, BackupFrameShape};

/// Snapshot of a backup transfer returned after each absorbed chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupProgress {
    /// The completed backup document, present exactly once: on the chunk that
    /// finishes the stream.
    pub document: Option<String>,
    /// Whether the start of the JSON backup document has been seen.
    pub started: bool,
    /// Number of document chunks accepted so far.
    pub chunks: usize,
    /// Number of chunks discarded because they arrived before the document
    /// start (the tail of an earlier, still-draining transfer).
    pub ignored_prefix_chunks: usize,
    /// Number of stream terminators among the discarded pre-start chunks.
    pub ignored_prefix_terminators: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BackupDiagnostics {
    frames: u64,
    owner_rejections: u64,
    last_action: Option<i32>,
    last_payload_bytes: usize,
    last_has_request_id: Option<bool>,
    last_terminal: Option<bool>,
    last_has_data: Option<bool>,
    last_data_bytes: usize,
    last_can_apply_backup: Option<bool>,
    last_applied_backup: Option<bool>,
    last_outcome: &'static str,
    last_owner_rejection: Option<&'static str>,
}

impl Default for BackupDiagnostics {
    fn default() -> Self {
        Self {
            frames: 0,
            owner_rejections: 0,
            last_action: None,
            last_payload_bytes: 0,
            last_has_request_id: None,
            last_terminal: None,
            last_has_data: None,
            last_data_bytes: 0,
            last_can_apply_backup: None,
            last_applied_backup: None,
            last_outcome: "none",
            last_owner_rejection: None,
        }
    }
}

impl BackupDiagnostics {
    fn observe(&mut self, shape: BackupFrameShape) {
        self.frames = self.frames.saturating_add(1);
        self.last_action = Some(shape.action);
        self.last_payload_bytes = shape.payload_bytes;
        self.last_has_request_id = Some(shape.request_id_present);
        self.last_terminal = shape.terminal;
        self.last_has_data = Some(shape.has_data);
        self.last_data_bytes = shape.data_bytes;
        self.last_can_apply_backup = shape.can_apply_backup;
        self.last_applied_backup = shape.applied_backup;
        self.last_outcome = shape.outcome;
    }

    fn summary(&self) -> String {
        format!(
            "type40_observations={}, assembler_frames={}, owner_rejections={}, last_action={}, last_payload_bytes={}, last_has_request_id={}, last_terminal={}, last_has_data={}, last_data_bytes={}, last_can_apply_backup={}, last_applied_backup={}, last_outcome={}, last_owner_rejection={}",
            self.frames.saturating_add(self.owner_rejections),
            self.frames,
            self.owner_rejections,
            self.last_action
                .map_or_else(|| "none".into(), |v| v.to_string()),
            self.last_payload_bytes,
            self.last_has_request_id
                .map_or_else(|| "unobserved".into(), |v| v.to_string()),
            self.last_terminal
                .map_or_else(|| "absent".into(), |v| v.to_string()),
            self.last_has_data
                .map_or_else(|| "absent".into(), |v| v.to_string()),
            self.last_data_bytes,
            self.last_can_apply_backup
                .map_or_else(|| "absent".into(), |v| v.to_string()),
            self.last_applied_backup
                .map_or_else(|| "absent".into(), |v| v.to_string()),
            self.last_outcome,
            self.last_owner_rejection.unwrap_or("none"),
        )
    }
}

/// I/O action the host must perform next, as decided by [`BackupRuntime::advance`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupAction {
    /// Nothing to do until the next payload or the next poll.
    Wait,
    /// Send the dedicated QC KeepAlive. Busy inbound traffic must not defer it.
    Keepalive,
    /// The transfer timed out; carries a human-readable reason. The runtime is
    /// terminal and the export request is not replayed.
    Failed(String),
}

/// How a terminal [`BackupRuntime`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupTerminalState {
    /// The full backup document was assembled.
    Complete,
    /// The transfer was cancelled locally with [`BackupRuntime::cancel`].
    Cancelled,
    /// The transfer timed out or the stream was malformed or oversized.
    Failed,
}

/// One LocalBackup transfer, expressed entirely in monotonic milliseconds.
#[derive(Debug)]
pub struct BackupRuntime {
    assembler: BackupAssembler,
    deadline_ms: u64,
    first_chunk_deadline_ms: u64,
    progress_deadline_ms: u64,
    next_keepalive_ms: u64,
    terminal: bool,
    terminal_state: Option<BackupTerminalState>,
    diagnostics: BackupDiagnostics,
}

fn window(from_ms: u64, timeout_ms: u64, deadline_ms: u64) -> u64 {
    from_ms.saturating_add(timeout_ms).min(deadline_ms)
}

impl BackupRuntime {
    /// Begins a transfer at monotonic time `now_ms`.
    ///
    /// `timeout_ms` is the overall budget; the first-chunk and stall windows
    /// from the protocol profile are clamped so they never extend past it.
    pub fn start(now_ms: u64, timeout_ms: u64) -> Self {
        let deadline_ms = now_ms.saturating_add(timeout_ms);
        Self {
            assembler: BackupAssembler::default(),
            deadline_ms,
            first_chunk_deadline_ms: window(
                now_ms,
                profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS,
                deadline_ms,
            ),
            progress_deadline_ms: deadline_ms,
            next_keepalive_ms: now_ms.saturating_add(profile::KEEPALIVE_INTERVAL_MS),
            terminal: false,
            terminal_state: None,
            diagnostics: BackupDiagnostics::default(),
        }
    }

    /// Whether the start of the backup document has been received.
    pub fn started(&self) -> bool {
        self.assembler.started()
    }

    /// Whether the transfer has ended (completed, cancelled or failed).
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// How the transfer ended, or `None` while it is still running.
    pub fn terminal_state(&self) -> Option<BackupTerminalState> {
        self.terminal_state
    }

    /// Cancellation is local and terminal. It never emits a second uncorrelated
    /// LocalBackup request and never permits a partial document to be resumed.
    pub fn cancel(&mut self) {
        if !self.terminal {
            self.terminal = true;
            self.terminal_state = Some(BackupTerminalState::Cancelled);
        }
    }

    /// Feed one LocalBackup chunk. A completed document is returned exactly
    /// once; callers own platform-specific persistence of that document.
    pub fn absorb(&mut self, now_ms: u64, payload: &[u8]) -> Result<BackupProgress, String> {
        if self.terminal {
            return Err("QC backup transfer is already complete".into());
        }
        if let Some(error) = self.expire(now_ms) {
            return Err(error);
        }
        let was_started = self.assembler.started();
        let previous_chunks = self.assembler.chunks();
        let previous_ignored = self.assembler.ignored_prefix_chunks();
        let (document, shape) = match self.assembler.push_classified(payload) {
            Ok(result) => result,
            Err(error) => {
                self.diagnostics.frames = self.diagnostics.frames.saturating_add(1);
                self.diagnostics.last_action = None;
                self.diagnostics.last_payload_bytes = payload.len();
                self.diagnostics.last_has_request_id = None;
                self.diagnostics.last_terminal = None;
                self.diagnostics.last_has_data = None;
                self.diagnostics.last_data_bytes = 0;
                self.diagnostics.last_can_apply_backup = None;
                self.diagnostics.last_applied_backup = None;
                self.diagnostics.last_outcome = match &error {
                    crate::responses::ResponseDecodeError::OversizedBackup => "oversized",
                    crate::responses::ResponseDecodeError::InvalidBackupDocument => {
                        "invalid-document"
                    }
                    _ => "decode-error",
                };
                // A malformed/oversized message is terminal. Never replay an
                // uncorrelated export request or splice separate streams.
                self.terminal = true;
                self.terminal_state = Some(BackupTerminalState::Failed);
                return Err(error.to_string());
            }
        };
        self.diagnostics.observe(shape);
        if document.is_some() {
            self.terminal = true;
            self.terminal_state = Some(BackupTerminalState::Complete);
        } else if self.assembler.chunks() > previous_chunks {
            self.progress_deadline_ms = window(
                now_ms,
                profile::BACKUP_STREAM_STALL_TIMEOUT_MS,
                self.deadline_ms,
            );
        } else if !was_started && self.assembler.ignored_prefix_chunks() > previous_ignored {
            // An earlier uncorrelated transfer is still draining. Do not inject
            // a duplicate request; extend only the observation window.
            self.first_chunk_deadline_ms = window(
                now_ms,
                profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS,
                self.deadline_ms,
            );
        }
        Ok(BackupProgress {
            document,
            started: self.assembler.started(),
            chunks: self.assembler.chunks(),
            ignored_prefix_chunks: self.assembler.ignored_prefix_chunks(),
            ignored_prefix_terminators: self.assembler.ignored_prefix_terminators(),
        })
    }

    /// Decide the next protocol action. The host performs the physical write.
    pub fn advance(&mut self, now_ms: u64) -> BackupAction {
        if self.terminal {
            return BackupAction::Wait;
        }
        if let Some(error) = self.expire(now_ms) {
            return BackupAction::Failed(error);
        }
        if now_ms >= self.next_keepalive_ms {
            self.next_keepalive_ms = now_ms.saturating_add(profile::KEEPALIVE_INTERVAL_MS);
            return BackupAction::Keepalive;
        }
        BackupAction::Wait
    }

    pub(super) fn record_owner_rejection(&mut self, reason: &'static str) {
        self.diagnostics.owner_rejections = self.diagnostics.owner_rejections.saturating_add(1);
        self.diagnostics.last_owner_rejection = Some(reason);
    }

    // Observation must not consume the keepalive action. Both input and timer
    // paths share only the terminal deadline transition.
    fn expire(&mut self, now_ms: u64) -> Option<String> {
        if now_ms >= self.deadline_ms {
            self.terminal = true;
            self.terminal_state = Some(BackupTerminalState::Failed);
            return Some(format!(
                "QC backup timed out: overall deadline reached after the single request, {} complete document chunk(s), and {} ignored prefix chunk(s); backup diagnostics: {}",
                self.assembler.chunks(),
                self.assembler.ignored_prefix_chunks(),
                self.diagnostics.summary()
            ));
        }
        if self.assembler.started() {
            if now_ms >= self.progress_deadline_ms {
                self.terminal = true;
                self.terminal_state = Some(BackupTerminalState::Failed);
                return Some(format!(
                    "QC backup timed out: stream stalled after {} chunk(s); the partial document was discarded and the export request was not replayed; backup diagnostics: {}",
                    self.assembler.chunks(),
                    self.diagnostics.summary()
                ));
            }
        } else if now_ms >= self.first_chunk_deadline_ms {
            self.terminal = true;
            self.terminal_state = Some(BackupTerminalState::Failed);
            return Some(format!(
                "QC backup timed out: no JSON document start arrived after the single request; ignored {} stale chunk(s) and {} stale terminator(s); backup diagnostics: {}",
                self.assembler.ignored_prefix_chunks(),
                self.assembler.ignored_prefix_terminators(),
                self.diagnostics.summary()
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(text: &str, last: bool) -> Vec<u8> {
        fn push_varint(mut value: usize, target: &mut Vec<u8>) {
            loop {
                let byte = (value & 0x7f) as u8;
                value >>= 7;
                target.push(if value == 0 { byte } else { byte | 0x80 });
                if value == 0 {
                    break;
                }
            }
        }
        let mut payload = vec![0x08, 0x01, 0x1a]; // UPDATE + field 3: backup_json
        push_varint(text.len(), &mut payload);
        payload.extend_from_slice(text.as_bytes());
        payload.extend_from_slice(&[0x30, u8::from(last)]); // explicit field 6
        payload
    }

    fn create_echo(text: &str, last: bool) -> Vec<u8> {
        let mut payload = vec![0x08, 0x00, 0x1a]; // CREATE + field 3: backup_json
        payload.push(text.len() as u8);
        payload.extend_from_slice(text.as_bytes());
        payload.extend_from_slice(&[0x30, u8::from(last)]);
        payload
    }

    #[test]
    fn timeout_diagnostics_classify_frame_shape_without_echoing_payload() {
        let secret_like = "{\"ownerPreset\":\"Private Name 9917\"}";
        let mut runtime = BackupRuntime::start(0, 180_000);
        runtime.absorb(1, &create_echo(secret_like, false)).unwrap();
        let message = match runtime.advance(profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS) {
            BackupAction::Failed(message) => message,
            other => panic!("expected timeout, got {other:?}"),
        };
        assert!(message.contains("type40_observations=1"));
        assert!(message.contains("assembler_frames=1"));
        assert!(message.contains("last_action=0"));
        assert!(message.contains("last_terminal=false"));
        assert!(message.contains("last_has_data=true"));
        assert!(message.contains(&format!("last_data_bytes={}", secret_like.len())));
        assert!(message.contains("last_outcome=ignored-action"));
        assert!(!message.contains(secret_like));
        assert!(!message.contains("Private Name 9917"));
    }

    #[test]
    fn timeout_diagnostics_capture_control_fields_without_ids_or_unknown_payload() {
        // UPDATE, request_id=42, can_apply_backup=false, applied_backup=true,
        // plus an unknown field 7. The backup chunk and terminal flag are absent.
        let payload = [
            0x08, 0x01, // action = UPDATE
            0x10, 0x2a, // request_id = 42
            0x20, 0x00, // can_apply_backup = false (present)
            0x28, 0x01, // applied_backup = true (present)
            0x38, 0x63, // unknown field 7 = 99
        ];
        let mut runtime = BackupRuntime::start(0, 180_000);
        runtime.absorb(1, &payload).unwrap();
        let message = match runtime.advance(profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS) {
            BackupAction::Failed(message) => message,
            other => panic!("expected timeout, got {other:?}"),
        };

        assert!(message.contains("last_action=1"));
        assert!(message.contains(&format!("last_payload_bytes={}", payload.len())));
        assert!(message.contains("last_has_request_id=true"));
        assert!(message.contains("last_terminal=absent"));
        assert!(message.contains("last_has_data=false"));
        assert!(message.contains("last_can_apply_backup=false"));
        assert!(message.contains("last_applied_backup=true"));
        assert!(message.contains("last_outcome=missing-terminal"));
        assert!(!message.contains("42"));
        assert!(!message.contains("99"));
    }

    #[test]
    fn timeout_diagnostics_distinguish_missing_fields_and_prefix_chunks() {
        let cases = [
            (vec![0x08, 0x01], "missing-terminal", "last_has_data=false"),
            (
                vec![0x08, 0x01, 0x30, 0x00],
                "missing-data",
                "last_terminal=false",
            ),
        ];
        for (payload, outcome, field) in cases {
            let mut runtime = BackupRuntime::start(0, 180_000);
            runtime.absorb(1, &payload).unwrap();
            let message = match runtime.advance(profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS) {
                BackupAction::Failed(message) => message,
                other => panic!("expected timeout, got {other:?}"),
            };
            assert!(message.contains(&format!("last_outcome={outcome}")));
            assert!(message.contains(field));
            assert!(message.contains(&format!("last_payload_bytes={}", payload.len())));
            assert!(message.contains("last_has_request_id=false"));
            assert!(message.contains("last_can_apply_backup=absent"));
            assert!(message.contains("last_applied_backup=absent"));
        }

        let mut prefix = BackupRuntime::start(0, 180_000);
        prefix.absorb(1, &chunk("not-a-document", false)).unwrap();
        let message = match prefix.advance(profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS + 1) {
            BackupAction::Failed(message) => message,
            other => panic!("expected timeout, got {other:?}"),
        };
        assert!(message.contains("last_outcome=ignored-prefix"));
        assert!(message.contains("last_action=1"));
    }

    #[test]
    fn accepted_chunk_classification_records_shape_not_document_text() {
        let text = "{\"type\":\"backup\",\"creator\":\"quad\"}";
        let mut runtime = BackupRuntime::start(0, 180_000);
        runtime.absorb(1, &chunk(text, true)).unwrap();
        assert_eq!(runtime.diagnostics.last_outcome, "complete");
        assert_eq!(runtime.diagnostics.last_action, Some(1));
        assert_eq!(runtime.diagnostics.last_terminal, Some(true));
        assert_eq!(runtime.diagnostics.last_data_bytes, text.len());
        assert!(!runtime.diagnostics.summary().contains(text));
    }

    #[test]
    fn never_replays_an_uncorrelated_export_request() {
        let mut runtime = BackupRuntime::start(1_000, 180_000);
        assert!(matches!(
            runtime.advance(1_000 + profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS),
            BackupAction::Failed(message) if message.contains("single request")
        ));
        assert_eq!(runtime.terminal_state(), Some(BackupTerminalState::Failed));
    }

    #[test]
    fn keepalive_is_independent_of_inbound_progress() {
        let mut runtime = BackupRuntime::start(0, 180_000);
        runtime
            .absorb(1_000, &chunk("{\"type\":\"backup\",", false))
            .unwrap();
        runtime
            .absorb(2_000, &chunk("\"creator\":\"quad\",", false))
            .unwrap();
        assert_eq!(
            runtime.advance(profile::KEEPALIVE_INTERVAL_MS),
            BackupAction::Keepalive
        );
    }

    #[test]
    fn completes_a_document_once() {
        let mut runtime = BackupRuntime::start(0, 180_000);
        let progress = runtime
            .absorb(
                1,
                &chunk("{\"type\":\"backup\",\"creator\":\"quad\"}", true),
            )
            .unwrap();
        assert_eq!(
            progress.document.as_deref(),
            Some("{\"type\":\"backup\",\"creator\":\"quad\"}")
        );
        assert!(runtime.is_terminal());
        assert_eq!(
            runtime.terminal_state(),
            Some(BackupTerminalState::Complete)
        );
        assert!(runtime.absorb(2, &chunk("{}", true)).is_err());
    }

    #[test]
    fn expired_backup_observation_cannot_complete_or_renew_a_stream() {
        let complete = chunk("{\"type\":\"backup\",\"creator\":\"quad\"}", true);
        let mut overall = BackupRuntime::start(0, 100);
        assert!(overall
            .absorb(100, &complete)
            .unwrap_err()
            .contains("overall deadline"));
        assert_eq!(overall.terminal_state(), Some(BackupTerminalState::Failed));

        let mut first = BackupRuntime::start(0, 180_000);
        assert!(first
            .absorb(profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS, &complete)
            .unwrap_err()
            .contains("no JSON document start"));

        let mut partial = BackupRuntime::start(0, 180_000);
        partial
            .absorb(1, &chunk("{\"type\":\"backup\",", false))
            .unwrap();
        assert!(partial
            .absorb(
                1 + profile::BACKUP_STREAM_STALL_TIMEOUT_MS,
                &chunk("\"creator\":\"quad\"}", true)
            )
            .unwrap_err()
            .contains("stream stalled"));
        assert_eq!(partial.terminal_state(), Some(BackupTerminalState::Failed));
    }

    #[test]
    fn cancellation_and_malformed_started_streams_cannot_be_retried() {
        let mut cancelled = BackupRuntime::start(0, 180_000);
        cancelled.cancel();
        assert_eq!(
            cancelled.terminal_state(),
            Some(BackupTerminalState::Cancelled)
        );
        assert_eq!(
            cancelled.advance(profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS),
            BackupAction::Wait
        );

        let mut malformed = BackupRuntime::start(0, 180_000);
        malformed
            .absorb(1, &chunk("{\"type\":\"backup\",", false))
            .unwrap();
        assert!(malformed.absorb(2, &[0xff]).is_err());
        assert_eq!(
            malformed.terminal_state(),
            Some(BackupTerminalState::Failed)
        );
        assert_eq!(
            malformed.advance(profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS),
            BackupAction::Wait
        );
    }
}
