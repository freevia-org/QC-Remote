//! Host-side tracking of a pointer that a raw mouse frame left DOWN on the unit.
//!
//! The protocol's mouse frames are `PRESS` 0, `RELEASE` 1, `MOVE` 2, `TAP` 3 and
//! `DRAG` 4. Observed on CorOS 4.1.0 (2026-10-06): `PRESS` is touch-down and
//! `RELEASE` is touch-up, so a lone `PRESS` puts a pointer down on the unit and
//! leaves it there (the row under it looks pressed) until a `RELEASE` lifts it.
//! Every gesture the gateway builds balances its own presses (a hold is a
//! `PRESS`, a wait and a `RELEASE`), so only the explicit raw-frame command, or
//! a hold whose touch-up could not be delivered, can strand a pointer.
//! [`TouchGuard`] is the host's record of that, shared by every host that sends
//! the raw command:
//!
//! - after a raw `PRESS` the pointer is held; every other screen gesture is
//!   refused with a typed error beginning [`TOUCH_HELD`] until a `RELEASE` is
//!   sent. A raw `MOVE`, the `RELEASE` itself, a further raw `PRESS` and a raw
//!   `TAP` stay allowed: during an item drag (a DRAG frame begins a held drag) each
//!   `PRESS` acts as a pointer-position update for drop-target hit testing, so
//!   several in a row are legitimate; a `TAP` is a pointer-position update plus a
//!   release in one frame (observed on CorOS 4.1.0, 2026-10-06: a long `PRESS` on
//!   a Grid block then one `TAP` on a cell drops the block there), so a delivered
//!   raw `TAP` ends the hold exactly as a `RELEASE` does;
//! - a hold (`long_tap_screen`) whose touch-up could not be sent, even after the
//!   one best-effort second touch-up the broker gives it, is recorded here as a
//!   held pointer too, so the failure is visible to the next gesture instead of
//!   leaving the unit pressed with nothing saying so.
//!
//! **The host never releases a held pointer by itself.** An earlier version sent
//! a `RELEASE` at the held point five seconds after a raw `PRESS`. That frame was
//! unrequested and the five seconds were never measured. It is not neutral: the
//! unit treats `PRESS` then `RELEASE` at one point as a click (observed: it
//! toggled or opened what was under it), a `PRESS` held about 0.9 s then
//! `RELEASE` on a parameter control assigns the value to the current scene, a
//! `RELEASE` over a lifted item drops it where the pointer is (on a Grid block
//! that can be the live red delete bar), and on a Gig View tile a `RELEASE`
//! selected that scene. So a timer that lifted an abandoned press could itself
//! click, assign or drop something the caller never asked for, and a timer that
//! outlived its session could do that to a different session. The guard instead
//! refuses everything until the caller sends the release or raw tap, which the
//! refusal names.
//!
//! This module is pure state and frame construction; the host owns the clock and
//! the transport. It never retries.

use crate::commands::DeviceOperation;
use crate::commands::ScreenMouseType;

/// Prefix of the typed refusal a gesture gets while a pointer is still down.
pub const TOUCH_HELD: &str = "TOUCH_HELD";

/// A pointer a raw `PRESS` (or a hold that could not be completed) put down and
/// nothing has lifted yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeldTouch {
    /// Horizontal position of the press in screen pixels.
    pub x: f32,
    /// Vertical position of the press in screen pixels.
    pub y: f32,
    /// Host clock, in milliseconds, when the press was sent.
    pub since_ms: u64,
}

/// Which pointer, if any, a raw `PRESS` or a failed hold has left down.
#[derive(Debug, Default)]
pub struct TouchGuard {
    held: Option<HeldTouch>,
    note: Option<String>,
}

impl TouchGuard {
    /// The pointer currently held, if any.
    pub fn held(&self) -> Option<HeldTouch> {
        self.held
    }

    /// Whether `operation` may be sent now. Every screen gesture is refused with
    /// a [`TOUCH_HELD`] error while a pointer is down, except the raw `RELEASE`
    /// that ends it, a raw `MOVE`, a further raw `PRESS` (a pointer-position
    /// update during an item drag) and a raw `TAP` (pointer update plus release).
    /// Operations that are not screen gestures are never refused here.
    pub fn admit(&self, operation: &DeviceOperation, now_ms: u64) -> Result<(), String> {
        let Some(held) = self.held else {
            return Ok(());
        };
        let allowed = match operation {
            DeviceOperation::ScreenMouseFrame { kind, .. } => matches!(
                kind,
                ScreenMouseType::Release
                    | ScreenMouseType::Move
                    | ScreenMouseType::Press
                    | ScreenMouseType::Tap
            ),
            DeviceOperation::ScreenTap { .. }
            | DeviceOperation::ScreenHold { .. }
            | DeviceOperation::ScreenDrag { .. }
            | DeviceOperation::ScreenDragSteps { .. } => false,
            _ => true,
        };
        if allowed {
            return Ok(());
        }
        let down_ms = now_ms.saturating_sub(held.since_ms);
        let mut message = format!(
            "{TOUCH_HELD}: a pointer at ({}, {}) has been down for {down_ms} ms and no release has been sent; send a release frame (or a raw tap, which is a pointer update plus a release) to lift it. The host never releases it by itself, and no other screen gesture is sent meanwhile",
            held.x, held.y
        );
        if let Some(note) = &self.note {
            message.push_str(&format!("; {note}"));
        }
        Err(message)
    }

    /// Record that a raw `PRESS` at `(x, y)` is being sent. Call it before the
    /// frame goes out: if the write's outcome is unknown the pointer may be down.
    pub fn note_press(&mut self, x: f32, y: f32, now_ms: u64) {
        self.note = None;
        self.held = Some(HeldTouch {
            x,
            y,
            since_ms: now_ms,
        });
    }

    /// Record that a hold at `(x, y)` pressed the pointer but its touch-up could
    /// not be delivered. `reason` is quoted by the next refusal. The pointer
    /// stays held until a release is sent; nothing is retried here.
    pub fn note_unreleased_hold(&mut self, x: f32, y: f32, now_ms: u64, reason: String) {
        self.held = Some(HeldTouch {
            x,
            y,
            since_ms: now_ms,
        });
        self.note = Some(reason);
    }

    /// Record that a `RELEASE` (or a raw `TAP`) was sent successfully, so
    /// nothing is held.
    pub fn note_release_sent(&mut self) {
        self.held = None;
        self.note = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap() -> DeviceOperation {
        DeviceOperation::ScreenTap { x: 1.0, y: 2.0 }
    }

    fn raw(kind: ScreenMouseType) -> DeviceOperation {
        let to = (kind == ScreenMouseType::Drag).then_some((5.0, 6.0));
        DeviceOperation::ScreenMouseFrame {
            kind,
            x: 1.0,
            y: 2.0,
            to,
        }
    }

    #[test]
    fn nothing_is_refused_while_no_pointer_is_down() {
        let guard = TouchGuard::default();
        assert_eq!(guard.held(), None);
        for operation in [
            tap(),
            raw(ScreenMouseType::Press),
            raw(ScreenMouseType::Release),
            raw(ScreenMouseType::Drag),
            DeviceOperation::ScreenHold { x: 1.0, y: 2.0 },
        ] {
            assert_eq!(guard.admit(&operation, 0), Ok(()), "{operation:?}");
        }
    }

    #[test]
    fn a_raw_press_blocks_every_gesture_except_press_release_move_and_tap() {
        let mut guard = TouchGuard::default();
        guard.note_press(10.0, 20.0, 1_000);
        for operation in [
            tap(),
            DeviceOperation::ScreenHold { x: 1.0, y: 2.0 },
            DeviceOperation::ScreenDrag {
                x: 1.0,
                y: 2.0,
                to_x: 3.0,
                to_y: 4.0,
            },
            DeviceOperation::ScreenDragSteps {
                x: 1.0,
                y: 2.0,
                to_x: 3.0,
                to_y: 4.0,
                steps: 4,
                hold_ms: 0,
            },
            raw(ScreenMouseType::Drag),
        ] {
            let error = guard.admit(&operation, 1_400).unwrap_err();
            assert!(error.starts_with(TOUCH_HELD), "{operation:?}: {error}");
            assert!(error.contains("(10, 20)"));
            assert!(error.contains("400 ms"));
        }
        // The release that ends it and a raw move are allowed, and so is
        // anything that is not a screen gesture.
        assert_eq!(guard.admit(&raw(ScreenMouseType::Release), 1_400), Ok(()));
        assert_eq!(guard.admit(&raw(ScreenMouseType::Move), 1_400), Ok(()));
        // A raw tap is a pointer update plus a release: allowed (the host clears
        // the hold once it is delivered, see TouchGuard::note_release_sent).
        assert_eq!(guard.admit(&raw(ScreenMouseType::Tap), 1_400), Ok(()));
        // A further press is a pointer-position update (item drag), not refused;
        // the recorded point follows it.
        assert_eq!(guard.admit(&raw(ScreenMouseType::Press), 1_400), Ok(()));
        guard.note_press(30.0, 40.0, 1_400);
        assert_eq!(guard.held().unwrap().since_ms, 1_400);
        assert_eq!(
            (guard.held().unwrap().x, guard.held().unwrap().y),
            (30.0, 40.0)
        );
        assert_eq!(guard.admit(&DeviceOperation::Undo, 1_400), Ok(()));
    }

    #[test]
    fn a_sent_release_clears_the_guard() {
        let mut guard = TouchGuard::default();
        guard.note_press(10.0, 20.0, 0);
        assert!(guard.admit(&tap(), 1).is_err());
        guard.note_release_sent();
        assert_eq!(guard.held(), None);
        assert_eq!(guard.admit(&tap(), 2), Ok(()));
    }

    #[test]
    fn the_refusal_says_the_host_never_releases_by_itself_and_names_the_way_out() {
        let mut guard = TouchGuard::default();
        guard.note_press(10.0, 20.0, 0);
        let refusal = guard.admit(&tap(), 5_000).unwrap_err();
        assert!(refusal.contains("never releases it by itself"), "{refusal}");
        assert!(refusal.contains("release frame"), "{refusal}");
        assert!(refusal.contains("raw tap"), "{refusal}");
        // Nothing here is time driven: however long the press has been down the
        // guard still holds it and sends nothing.
        assert!(guard.held().is_some());
        let much_later = guard.admit(&tap(), 3_600_000).unwrap_err();
        assert!(much_later.contains("3600000 ms"), "{much_later}");
    }

    #[test]
    fn a_hold_whose_touch_up_failed_is_held_says_why_and_clears_on_release() {
        let mut guard = TouchGuard::default();
        guard.note_unreleased_hold(
            120.0,
            80.0,
            500,
            "the touch-up could not be sent: gone".into(),
        );
        assert_eq!(guard.held().unwrap().x, 120.0);
        let refusal = guard.admit(&tap(), 900).unwrap_err();
        assert!(refusal.starts_with(TOUCH_HELD), "{refusal}");
        assert!(refusal.contains("(120, 80)"), "{refusal}");
        assert!(
            refusal.contains("the touch-up could not be sent: gone"),
            "{refusal}"
        );
        // The explicit release still clears it, and clears the reason with it.
        guard.note_release_sent();
        assert_eq!(guard.admit(&tap(), 1_000), Ok(()));
        guard.note_press(1.0, 2.0, 1_100);
        assert!(!guard.admit(&tap(), 1_200).unwrap_err().contains("gone"));
    }
}
