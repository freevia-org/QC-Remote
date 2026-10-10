//! The device's request that the host stop driving it (SuspendConnection).
//!
//! While suspended, the unit locks its screen and volume knob and stops
//! answering requests. The owner therefore refuses to put any client request
//! on the wire until it resumes. Host-internal session upkeep is not a client
//! request and is never refused here: the keepalive and the startup lifecycle
//! dispatch without an operation, and the backup this host started is the
//! very transfer the suspension exists for. Teardown sends without a dispatch
//! ticket at all.
//!
//! The flag is session-scoped. A disconnect, a new connection or a new session
//! epoch clears it, so it cannot outlive the session that observed it. Two
//! independent device signals end it within a session: SuspendConnection with
//! `suspend = false`, and GeneralSettings reporting the screen lock released
//! after it was observed engaged. Nothing ends it on a timer: resuming writes
//! into a unit whose screen is still locked is the failure this gate exists
//! to prevent. A resume frame that never arrives leaves the gate up until the
//! session is reset, and the diagnostics say how long it has been up.
//!
//! At the end of a backup the unit releases the screen lock about a
//! millisecond before it sends `suspend = false`. The release lifts the gate;
//! the resume that follows still belongs to that suspension. It is recorded on
//! the ended suspension, its reason is still checked for a resync, and it is
//! not counted as a stray resume.
//!
//! Only the BACKUP reason has been observed on hardware. FACTORY_RESET and
//! SETTINGS_RESET (and any reason this build does not know) are handled
//! defensively: their end is assumed to invalidate every cached device state,
//! so the owner asks the host for a full session resynchronization.

use super::{OperationTicket, SessionRuntime};
use crate::proto::cortex_protobuf_v2 as pa;
use prost::Message;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};

/// Every refusal this gate produces starts with this text, so a host can
/// classify the refusal without parsing anything after it.
pub use crate::generated_host_policy::DEVICE_SUSPENDED_MARKER;

/// Appended when another frame of a request that already reached the device
/// would have followed. The request's effect is then genuinely unknown, and
/// the wording deliberately carries the phrase hosts already treat as an
/// unresolved write.
use crate::generated_host_policy::PARTIAL_REQUEST_SUFFIX;

/// The reason the device gave, as the protocol names it. `UNSPECIFIED` when
/// the frame asserted none.
const UNSPECIFIED_REASON: &str = "UNSPECIFIED";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
/// The currently active device suspension.
pub struct ActiveSuspension {
    /// Protocol reason: `BACKUP`, `FACTORY_RESET`, `SETTINGS_RESET`,
    /// `UNSPECIFIED`, or `UNKNOWN_<n>` for a value this build does not name.
    pub reason: String,
    /// Unix timestamp in milliseconds when this suspension began.
    pub since_unix_ms: u64,
    /// Client requests refused while this suspension has been active.
    pub refused_requests: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
/// The event that ended a previously active device suspension.
pub enum SuspensionEnd {
    /// SuspendConnection with `suspend = false`.
    DeviceResumed,
    /// GeneralSettings reported the screen lock released after it had been
    /// observed engaged.
    ScreenUnlocked,
    /// The session was disconnected, replaced or rebuilt while suspended.
    SessionEnded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
/// The most recently ended device suspension.
pub struct EndedSuspension {
    /// Protocol reason reported when the suspension began.
    pub reason: String,
    /// Unix timestamp in milliseconds when the suspension began.
    pub since_unix_ms: u64,
    /// Unix timestamp in milliseconds when the suspension ended.
    pub ended_unix_ms: u64,
    /// Event that ended the suspension.
    pub ended_by: SuspensionEnd,
    /// Client requests refused while the suspension was active.
    pub refused_requests: u64,
    /// The owner asked the host to resynchronize the whole session.
    pub resync_requested: bool,
    /// Reason carried by the device's resume frame (`suspend = false`). Set
    /// when that frame ended the suspension, or when it arrived after the
    /// screen unlock had already ended it. `None` while that resume is still
    /// outstanding, and for a suspension the session ended.
    pub resume_reason: Option<String>,
}

/// What a status surface reports. `active` is the authoritative flag; the
/// counters span every session this owner has served.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuspensionDiagnostics {
    /// Current suspension, if any; this is the authoritative active state.
    pub active: Option<ActiveSuspension>,
    /// Number of suspensions observed by this owner.
    pub suspensions_observed: u64,
    /// Total client requests refused during suspensions.
    pub refused_requests: u64,
    /// Number of session resynchronizations requested after a suspension.
    pub resyncs_requested: u64,
    /// Resume frames that matched no suspension of their session. The resume
    /// that follows a screen unlock which ended a suspension is not counted.
    pub unmatched_resumes: u64,
    /// Details of the most recently ended suspension, if one has ended.
    pub last_ended: Option<EndedSuspension>,
}

#[derive(Default)]
pub(super) struct SuspensionTracker {
    active: Option<Active>,
    /// The last screen-lock value GeneralSettings asserted in this session.
    screen_locked: Option<bool>,
    /// The screen unlock ended the last suspension and its resume frame has
    /// not arrived yet. Cleared by that frame, a new suspension, the next
    /// lock engage and the end of the session.
    awaiting_resume: bool,
    pending_resync: Option<String>,
    suspensions_observed: u64,
    refused_total: AtomicU64,
    resyncs_requested: u64,
    unmatched_resumes: u64,
    last_ended: Option<EndedSuspension>,
}

struct Active {
    reason: String,
    since_unix_ms: u64,
    refused: AtomicU64,
}

fn unix_ms() -> u64 {
    crate::time::unix_time_ms()
}

fn reason_name(reason: Option<&pa::suspend_connection_message::ReasonOneOf>) -> String {
    match reason {
        Some(pa::suspend_connection_message::ReasonOneOf::Reason(value)) => {
            pa::suspend_connection_message::Reason::try_from(*value)
                .map(|reason| reason.as_str_name().to_owned())
                .unwrap_or_else(|_| format!("UNKNOWN_{value}"))
        }
        None => UNSPECIFIED_REASON.to_owned(),
    }
}

/// Only an observed BACKUP is known to leave the cached device state intact.
/// Every other reason -- the two reset reasons, an unasserted reason and one
/// this build does not name -- is treated as invalidating it.
fn reason_preserves_state(reason: &str) -> bool {
    reason == "BACKUP"
}

fn describe(reason: &str) -> String {
    match reason {
        "BACKUP" => "BACKUP in progress (a device backup)".into(),
        "FACTORY_RESET" => "FACTORY_RESET in progress".into(),
        "SETTINGS_RESET" => "SETTINGS_RESET in progress".into(),
        other => format!("{other} in progress"),
    }
}

impl SuspensionTracker {
    fn begin(&mut self, reason: String) {
        self.awaiting_resume = false;
        if let Some(active) = &mut self.active {
            // A repeated suspend keeps its original start; a reason the device
            // now asserts replaces an unspecified one.
            if active.reason == UNSPECIFIED_REASON {
                active.reason = reason;
            }
            return;
        }
        self.suspensions_observed = self.suspensions_observed.saturating_add(1);
        self.active = Some(Active {
            reason,
            since_unix_ms: unix_ms(),
            refused: AtomicU64::new(0),
        });
    }

    fn end(&mut self, ended_by: SuspensionEnd, resume_reason: Option<&str>) {
        let Some(active) = self.active.take() else {
            if ended_by == SuspensionEnd::DeviceResumed {
                if std::mem::take(&mut self.awaiting_resume) {
                    self.confirm_resume(resume_reason.unwrap_or(UNSPECIFIED_REASON));
                } else {
                    self.unmatched_resumes = self.unmatched_resumes.saturating_add(1);
                }
            }
            return;
        };
        self.awaiting_resume = ended_by == SuspensionEnd::ScreenUnlocked;
        let resync = ended_by != SuspensionEnd::SessionEnded
            && (!reason_preserves_state(&active.reason)
                || resume_reason.is_some_and(|reason| !reason_preserves_state(reason)));
        if resync {
            self.resyncs_requested = self.resyncs_requested.saturating_add(1);
            self.pending_resync = Some(active.reason.clone());
        }
        self.last_ended = Some(EndedSuspension {
            reason: active.reason,
            since_unix_ms: active.since_unix_ms,
            ended_unix_ms: unix_ms(),
            ended_by,
            refused_requests: active.refused.load(Ordering::Relaxed),
            resync_requested: resync,
            resume_reason: (ended_by == SuspensionEnd::DeviceResumed)
                .then(|| resume_reason.unwrap_or(UNSPECIFIED_REASON).to_owned()),
        });
    }

    /// The resume frame of a suspension the screen unlock already ended. The
    /// gate lifted at the unlock; this only completes the record and applies
    /// the resume reason's resync check, which the unlock could not know.
    fn confirm_resume(&mut self, reason: &str) {
        let Some(ended) = self.last_ended.as_mut() else {
            return;
        };
        ended.resume_reason = Some(reason.to_owned());
        if !ended.resync_requested && !reason_preserves_state(reason) {
            ended.resync_requested = true;
            self.resyncs_requested = self.resyncs_requested.saturating_add(1);
            self.pending_resync = Some(reason.to_owned());
        }
    }

    /// Wipe everything one session established. The counters and the record
    /// of the last suspension survive, so a status reader can still see that
    /// a session ended while suspended.
    pub(super) fn session_ended(&mut self) {
        self.end(SuspensionEnd::SessionEnded, None);
        self.screen_locked = None;
        self.awaiting_resume = false;
        self.pending_resync = None;
    }

    pub(super) fn observe(&mut self, message_type: u16, payload: &[u8]) {
        match message_type {
            crate::profile::MESSAGE_TYPE_SUSPEND_CONNECTION => {
                let Ok(message) = pa::SuspendConnectionMessage::decode(payload) else {
                    return;
                };
                // `suspend` sits in its own oneof: a frame that does not assert
                // it says nothing about the suspension state.
                let Some(pa::suspend_connection_message::Suspend::Suspend(suspend)) =
                    message.suspend
                else {
                    return;
                };
                let reason = reason_name(message.reason.as_ref());
                if suspend {
                    self.begin(reason);
                } else {
                    self.end(SuspensionEnd::DeviceResumed, Some(&reason));
                }
            }
            crate::profile::MESSAGE_TYPE_GENERAL_SETTINGS => {
                let Ok(message) = pa::GeneralSettingsMessage::decode(payload) else {
                    return;
                };
                let Some(
                    pa::general_settings_message::LockScreenAndVolumeKnob::LockScreenAndVolumeKnob(
                        locked,
                    ),
                ) = message.lock_screen_and_volume_knob
                else {
                    return;
                };
                let was_locked = self.screen_locked.replace(locked);
                if locked {
                    // A new lock opens a new episode; a resume still owed by
                    // the previous one can no longer be told apart from it.
                    self.awaiting_resume = false;
                }
                // Only a release of a lock this session saw engaged ends the
                // suspension. A settings publication that merely reports the
                // lock off is not a transition and proves nothing.
                if !locked && was_locked == Some(true) {
                    self.end(SuspensionEnd::ScreenUnlocked, None);
                }
            }
            _ => {}
        }
    }

    fn refusal(&self) -> Option<String> {
        let active = self.active.as_ref()?;
        active.refused.fetch_add(1, Ordering::Relaxed);
        self.refused_total.fetch_add(1, Ordering::Relaxed);
        let seconds = unix_ms().saturating_sub(active.since_unix_ms) / 1000;
        Some(format!(
            "{DEVICE_SUSPENDED_MARKER}: {} for {seconds} s; the request was not sent. The unit's \
             screen stays locked until the device resumes control. Retry once it has resumed; if \
             the unit's screen is no longer locked, reset the device session to clear this",
            describe(&active.reason)
        ))
    }

    pub(super) fn diagnostics(&self) -> SuspensionDiagnostics {
        SuspensionDiagnostics {
            active: self.active.as_ref().map(|active| ActiveSuspension {
                reason: active.reason.clone(),
                since_unix_ms: active.since_unix_ms,
                refused_requests: active.refused.load(Ordering::Relaxed),
            }),
            suspensions_observed: self.suspensions_observed,
            refused_requests: self.refused_total.load(Ordering::Relaxed),
            resyncs_requested: self.resyncs_requested,
            unmatched_resumes: self.unmatched_resumes,
            last_ended: self.last_ended.clone(),
        }
    }
}

impl SessionRuntime {
    /// Whether the device currently holds this session suspended.
    pub fn is_suspended(&self) -> bool {
        self.suspension.active.is_some()
    }

    /// Return the current suspension and this owner's suspension counters.
    pub fn suspension(&self) -> SuspensionDiagnostics {
        self.suspension.diagnostics()
    }

    /// The reason of a suspension whose end requires the host to rebuild the
    /// whole session state, exactly once. The host performs a full session
    /// reset; a new session clears any request it did not act on.
    pub fn take_suspension_resync(&mut self) -> Option<String> {
        self.suspension.pending_resync.take()
    }

    /// The refusal for a client request that has not been admitted yet, if the
    /// device is suspended. Counted as a refused request.
    pub fn suspension_refusal(&self) -> Option<String> {
        self.suspension_refusal_for(None)
    }

    /// The refusal for a new client request, if the device is suspended. A
    /// backup this host started is the transfer the suspension exists for and
    /// is never refused; it is also validated as the one active backup.
    pub(super) fn suspension_refusal_for(
        &self,
        operation: Option<&OperationTicket>,
    ) -> Option<String> {
        if operation.is_some_and(|operation| operation.backup) {
            return None;
        }
        self.suspension.refusal()
    }

    /// The refusal for another frame of a request whose earlier frame already
    /// reached the device. Its outcome is unknown, and the text says so.
    pub(super) fn partial_suspension_refusal(&self, operation: &OperationTicket) -> Option<String> {
        self.suspension_refusal_for(Some(operation))
            .map(|refusal| format!("{refusal}; {PARTIAL_REQUEST_SUFFIX}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::delivery::ReportOutcome;
    use crate::device::session::SessionDriver;
    use crate::profile::{
        MESSAGE_TYPE_GENERAL_SETTINGS, MESSAGE_TYPE_GLOBAL_TEMPO, MESSAGE_TYPE_KEEP_ALIVE,
        MESSAGE_TYPE_LOCAL_BACKUP, MESSAGE_TYPE_SUSPEND_CONNECTION,
    };

    // Frames exactly as a unit sends them around a device backup: the screen
    // lock engages, the suspension starts, and both end after the transfer.
    const LOCK_ENGAGED: [u8; 4] = [0x08, 0x01, 0x30, 0x01];
    const SUSPEND_BACKUP: [u8; 4] = [0x10, 0x01, 0x18, 0x00];
    const LOCK_RELEASED: [u8; 4] = [0x08, 0x01, 0x30, 0x00];
    const RESUME_BACKUP: [u8; 4] = [0x10, 0x00, 0x18, 0x00];

    fn suspend_frame(suspend: bool, reason: pa::suspend_connection_message::Reason) -> Vec<u8> {
        pa::SuspendConnectionMessage {
            suspend: Some(pa::suspend_connection_message::Suspend::Suspend(suspend)),
            reason: Some(pa::suspend_connection_message::ReasonOneOf::Reason(
                reason as i32,
            )),
            ..Default::default()
        }
        .encode_to_vec()
    }

    fn live_owner() -> (SessionRuntime, u64) {
        let mut owner = SessionRuntime::new(0);
        let connection = owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "suspend".into(), 0));
        (owner, connection)
    }

    fn observe(owner: &mut SessionRuntime, connection: u64, message_type: u16, payload: &[u8]) {
        let ticket = owner.capture_observation(connection).unwrap();
        owner.observe_state(&ticket, message_type, payload).unwrap();
    }

    fn suspend_for_backup(owner: &mut SessionRuntime, connection: u64) {
        observe(
            owner,
            connection,
            MESSAGE_TYPE_GENERAL_SETTINGS,
            &LOCK_ENGAGED,
        );
        observe(
            owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &SUSPEND_BACKUP,
        );
    }

    fn assert_suspended_refusal(error: &str) {
        assert!(
            error.starts_with(DEVICE_SUSPENDED_MARKER),
            "not a suspension refusal: {error}"
        );
        assert!(error.contains("BACKUP in progress"), "{error}");
        assert!(
            !error.contains("device effect may be uncertain"),
            "a request that never left must not read as an unresolved write: {error}"
        );
    }

    #[test]
    fn recorded_backup_frames_suspend_the_session_and_report_the_reason() {
        let (mut owner, connection) = live_owner();
        assert!(!owner.is_suspended());
        suspend_for_backup(&mut owner, connection);
        assert!(owner.is_suspended());
        let status = owner.suspension();
        let active = status.active.expect("active suspension");
        assert_eq!(active.reason, "BACKUP");
        assert!(active.since_unix_ms > 0);
        assert_eq!(status.suspensions_observed, 1);
        let json = serde_json::to_value(owner.suspension()).unwrap();
        assert_eq!(json["active"]["reason"], "BACKUP");
        assert!(json["active"]["sinceUnixMs"].as_u64().is_some());
    }

    #[test]
    fn a_new_write_is_refused_with_the_suspension_error_while_suspended() {
        let (mut owner, connection) = live_owner();
        let before = owner.plan_operation(0, 1_000).unwrap();
        suspend_for_backup(&mut owner, connection);

        let error = owner.plan_operation(1, 1_000).unwrap_err();
        assert_suspended_refusal(&error);
        // A request admitted just before the suspension, but not yet sent, is
        // refused at the wire boundary rather than written into the locked unit.
        let error = owner.prepare_dispatch(Some(&before), None, 1).unwrap_err();
        assert_suspended_refusal(&error);
        let error = owner.validate_operation(&before, 1).unwrap_err();
        assert_suspended_refusal(&error);
        assert_eq!(owner.suspension().refused_requests, 3);
        assert_eq!(owner.suspension().active.unwrap().refused_requests, 3);
    }

    #[test]
    fn another_frame_of_a_request_already_on_the_wire_reports_an_uncertain_outcome() {
        let (mut owner, connection) = live_owner();
        let operation = owner.plan_operation(0, 1_000).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
        let first = owner
            .prepare_delivery(&dispatch, MESSAGE_TYPE_GLOBAL_TEMPO, 1, 0)
            .unwrap();
        owner.begin_native_report(&first, 0, 0).unwrap();
        owner
            .finish_native_report(&first, 0, ReportOutcome::Delivered, 0)
            .unwrap();
        suspend_for_backup(&mut owner, connection);

        let error = owner
            .prepare_delivery(&dispatch, MESSAGE_TYPE_GLOBAL_TEMPO, 1, 1)
            .unwrap_err();
        assert!(error.starts_with(DEVICE_SUSPENDED_MARKER), "{error}");
        assert!(error.contains("device effect may be uncertain"), "{error}");
        // Waiting on the frame that did leave is not refused after the fact.
        owner.validate_operation(&operation, 1).unwrap();
    }

    #[test]
    fn the_keepalive_keeps_flowing_while_suspended() {
        let (mut owner, connection) = live_owner();
        suspend_for_backup(&mut owner, connection);
        // The keepalive and the startup lifecycle dispatch without an operation.
        let dispatch = owner.prepare_dispatch(None, None, 5).unwrap();
        let delivery = owner
            .prepare_delivery(&dispatch, MESSAGE_TYPE_KEEP_ALIVE, 1, 5)
            .unwrap();
        owner.begin_native_report(&delivery, 0, 5).unwrap();
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 5)
            .unwrap();
        owner.require_delivery_complete(&delivery).unwrap();
        assert_eq!(owner.suspension().refused_requests, 0);
    }

    #[test]
    fn the_hosts_own_backup_transfer_proceeds_while_suspended() {
        let (mut owner, connection) = live_owner();
        let mut operation = owner.plan_operation(0, 180_000).unwrap();
        owner.begin_backup(&mut operation, 0).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 1).unwrap();
        let request = owner
            .prepare_delivery(&dispatch, MESSAGE_TYPE_LOCAL_BACKUP, 1, 1)
            .unwrap();
        owner.begin_native_report(&request, 0, 1).unwrap();
        owner
            .finish_native_report(&request, 0, ReportOutcome::Delivered, 1)
            .unwrap();
        // The request elicits the suspension.
        suspend_for_backup(&mut owner, connection);
        assert!(owner.is_suspended());

        // The backup's own keepalives ride its admission and must still go out.
        owner.validate_operation(&operation, 1_000).unwrap();
        let keepalive = owner
            .prepare_dispatch(Some(&operation), None, 1_000)
            .unwrap();
        let delivery = owner
            .prepare_delivery(&keepalive, MESSAGE_TYPE_KEEP_ALIVE, 1, 1_000)
            .unwrap();
        owner.begin_native_report(&delivery, 0, 1_000).unwrap();
        // ...and the advancing transfer is not stalled by the gate.
        assert!(owner.advance_backup(&operation, 1_000).is_ok());
        assert_eq!(owner.suspension().refused_requests, 0);

        // Anything else is still refused, including a second backup.
        assert_suspended_refusal(&owner.plan_operation(1_001, 180_000).unwrap_err());
    }

    #[test]
    fn a_forged_backup_flag_does_not_open_the_gate() {
        let (mut owner, connection) = live_owner();
        let mut operation = owner.plan_operation(0, 1_000).unwrap();
        suspend_for_backup(&mut owner, connection);
        operation.backup = true;
        assert!(owner.prepare_dispatch(Some(&operation), None, 1).is_err());
    }

    #[test]
    fn the_suspension_clears_when_the_device_resumes() {
        let (mut owner, connection) = live_owner();
        suspend_for_backup(&mut owner, connection);
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &RESUME_BACKUP,
        );
        assert!(!owner.is_suspended());
        owner.plan_operation(1, 1_000).unwrap();
        let ended = owner.suspension().last_ended.unwrap();
        assert_eq!(ended.ended_by, SuspensionEnd::DeviceResumed);
        assert_eq!(ended.reason, "BACKUP");
        assert_eq!(ended.resume_reason.as_deref(), Some("BACKUP"));
        assert!(!ended.resync_requested);
    }

    #[test]
    fn the_screen_lock_release_is_an_independent_resume_signal() {
        let (mut owner, connection) = live_owner();
        suspend_for_backup(&mut owner, connection);
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_GENERAL_SETTINGS,
            &LOCK_RELEASED,
        );
        assert!(!owner.is_suspended());
        assert_eq!(
            owner.suspension().last_ended.unwrap().ended_by,
            SuspensionEnd::ScreenUnlocked
        );
        // The resume frame that follows it is not counted as stray.
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &RESUME_BACKUP,
        );
        assert_eq!(owner.suspension().unmatched_resumes, 0);
        let ended = owner.suspension().last_ended.unwrap();
        assert_eq!(ended.ended_by, SuspensionEnd::ScreenUnlocked);
        assert_eq!(ended.resume_reason.as_deref(), Some("BACKUP"));
        assert!(owner.take_suspension_resync().is_none());
    }

    #[test]
    fn a_backup_in_device_frame_order_ends_once_and_records_its_resume() {
        let (mut owner, connection) = live_owner();
        // Lock, suspend, unlock, then the resume about a millisecond later.
        suspend_for_backup(&mut owner, connection);
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_GENERAL_SETTINGS,
            &LOCK_RELEASED,
        );
        // The gate lifts at the unlock, not at the resume.
        assert!(!owner.is_suspended());
        owner.plan_operation(1, 1_000).unwrap();
        let pending = owner.suspension().last_ended.unwrap();
        assert_eq!(pending.ended_by, SuspensionEnd::ScreenUnlocked);
        assert_eq!(pending.resume_reason, None);

        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &RESUME_BACKUP,
        );
        let status = owner.suspension();
        assert!(status.active.is_none());
        assert_eq!(status.suspensions_observed, 1);
        assert_eq!(status.unmatched_resumes, 0);
        assert_eq!(status.resyncs_requested, 0);
        let ended = status.last_ended.clone().unwrap();
        assert_eq!(ended.ended_by, SuspensionEnd::ScreenUnlocked);
        assert_eq!(ended.reason, "BACKUP");
        assert_eq!(ended.resume_reason.as_deref(), Some("BACKUP"));
        assert!(!ended.resync_requested);
        // The record keeps the moment the gate lifted.
        assert_eq!(ended.ended_unix_ms, pending.ended_unix_ms);
        assert!(owner.take_suspension_resync().is_none());
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["unmatchedResumes"], 0);
        assert_eq!(json["lastEnded"]["endedBy"], "screenUnlocked");
        assert_eq!(json["lastEnded"]["resumeReason"], "BACKUP");

        // The resume is consumed once: a second one is stray.
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &RESUME_BACKUP,
        );
        assert_eq!(owner.suspension().unmatched_resumes, 1);
    }

    #[test]
    fn a_resume_with_no_suspension_in_its_session_is_still_unmatched() {
        // No suspension at all.
        let (mut owner, connection) = live_owner();
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &RESUME_BACKUP,
        );
        assert_eq!(owner.suspension().unmatched_resumes, 1);
        assert!(owner.suspension().last_ended.is_none());

        // The unlock ended a suspension, but a new connection intervened.
        let (mut owner, connection) = live_owner();
        suspend_for_backup(&mut owner, connection);
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_GENERAL_SETTINGS,
            &LOCK_RELEASED,
        );
        let connection = owner.opened(10).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "suspend".into(), 10));
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &RESUME_BACKUP,
        );
        assert_eq!(owner.suspension().unmatched_resumes, 1);
        assert_eq!(owner.suspension().last_ended.unwrap().resume_reason, None);

        // The unlock ended a suspension, then the screen locked again.
        let (mut owner, connection) = live_owner();
        suspend_for_backup(&mut owner, connection);
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_GENERAL_SETTINGS,
            &LOCK_RELEASED,
        );
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_GENERAL_SETTINGS,
            &LOCK_ENGAGED,
        );
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &RESUME_BACKUP,
        );
        assert_eq!(owner.suspension().unmatched_resumes, 1);
    }

    #[test]
    fn a_reset_resume_after_the_screen_unlock_still_requests_a_resync() {
        use pa::suspend_connection_message::Reason;
        let (mut owner, connection) = live_owner();
        suspend_for_backup(&mut owner, connection);
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_GENERAL_SETTINGS,
            &LOCK_RELEASED,
        );
        // A backup unlock alone requests nothing.
        assert_eq!(owner.take_suspension_resync(), None);
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &suspend_frame(false, Reason::FactoryReset),
        );
        assert_eq!(
            owner.take_suspension_resync().as_deref(),
            Some(Reason::FactoryReset.as_str_name())
        );
        assert_eq!(owner.take_suspension_resync(), None);
        let status = owner.suspension();
        assert_eq!(status.resyncs_requested, 1);
        assert_eq!(status.unmatched_resumes, 0);
        let ended = status.last_ended.unwrap();
        assert_eq!(ended.reason, "BACKUP");
        assert_eq!(ended.ended_by, SuspensionEnd::ScreenUnlocked);
        assert_eq!(ended.resume_reason.as_deref(), Some("FACTORY_RESET"));
        assert!(ended.resync_requested);
    }

    #[test]
    fn a_lock_reported_off_without_a_release_does_not_resume() {
        let (mut owner, connection) = live_owner();
        // Suspension without the lock having been seen engaged first.
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &SUSPEND_BACKUP,
        );
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_GENERAL_SETTINGS,
            &LOCK_RELEASED,
        );
        assert!(owner.is_suspended());
    }

    #[test]
    fn the_suspension_clears_on_a_new_connection_epoch() {
        let (mut owner, connection) = live_owner();
        suspend_for_backup(&mut owner, connection);
        assert!(owner.is_suspended());
        // A replacement handshake starts a new session epoch.
        owner
            .next_handshake_write(10, "replacement", None)
            .expect("handshake write");
        assert!(!owner.is_suspended());
        assert_eq!(
            owner.suspension().last_ended.unwrap().ended_by,
            SuspensionEnd::SessionEnded
        );

        let (mut owner, connection) = live_owner();
        suspend_for_backup(&mut owner, connection);
        owner.disconnect(10, true);
        assert!(!owner.is_suspended());

        let (mut owner, connection) = live_owner();
        suspend_for_backup(&mut owner, connection);
        let _ = owner.opened(10).unwrap();
        assert!(!owner.is_suspended());
        // The session that ended suspended never asks for a resync: the next
        // session synchronizes from scratch anyway.
        assert!(owner.take_suspension_resync().is_none());
    }

    #[test]
    fn a_reset_resume_requests_a_resync_and_a_backup_resume_does_not() {
        use pa::suspend_connection_message::Reason;
        let (mut owner, connection) = live_owner();
        suspend_for_backup(&mut owner, connection);
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &RESUME_BACKUP,
        );
        assert_eq!(owner.take_suspension_resync(), None);

        for reason in [Reason::FactoryReset, Reason::SettingsReset] {
            let (mut owner, connection) = live_owner();
            observe(
                &mut owner,
                connection,
                MESSAGE_TYPE_SUSPEND_CONNECTION,
                &suspend_frame(true, reason),
            );
            let refusal = owner.plan_operation(1, 1_000).unwrap_err();
            assert!(refusal.contains(reason.as_str_name()), "{refusal}");
            observe(
                &mut owner,
                connection,
                MESSAGE_TYPE_SUSPEND_CONNECTION,
                &suspend_frame(false, reason),
            );
            assert!(!owner.is_suspended());
            assert_eq!(
                owner.take_suspension_resync().as_deref(),
                Some(reason.as_str_name())
            );
            // Exactly once.
            assert_eq!(owner.take_suspension_resync(), None);
            assert_eq!(owner.suspension().resyncs_requested, 1);
        }
    }

    #[test]
    fn a_suspension_without_a_reason_or_with_an_unknown_one_resyncs_defensively() {
        let (mut owner, connection) = live_owner();
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &[0x10, 0x01],
        );
        assert_eq!(owner.suspension().active.unwrap().reason, "UNSPECIFIED");
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &[0x10, 0x00],
        );
        assert_eq!(
            owner.take_suspension_resync().as_deref(),
            Some("UNSPECIFIED")
        );

        let (mut owner, connection) = live_owner();
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &[0x10, 0x01, 0x18, 0x09],
        );
        assert_eq!(owner.suspension().active.unwrap().reason, "UNKNOWN_9");
    }

    #[test]
    fn a_frame_that_asserts_no_suspend_value_changes_nothing() {
        let (mut owner, connection) = live_owner();
        suspend_for_backup(&mut owner, connection);
        // Only the reason: no claim about whether the session is suspended.
        observe(
            &mut owner,
            connection,
            MESSAGE_TYPE_SUSPEND_CONNECTION,
            &[0x18, 0x00],
        );
        assert!(owner.is_suspended());
    }
}
