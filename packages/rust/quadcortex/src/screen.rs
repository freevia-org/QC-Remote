use crate::commands::*;
use crate::profile;
use crate::proto::cortex_protobuf_v2 as pa;

/// The five mouse frame types of the protocol's `RemoteControlMouse.Type`, by
/// wire value: `PRESS` 0, `RELEASE` 1, `MOVE` 2, `TAP` 3, `DRAG` 4. The enum
/// names are the type-72 schema embedded in the Cortex Control desktop
/// executable; what the unit does with each is our measurement, not a vendor
/// statement.
///
/// Observed on CorOS 4.1.0 (2026-10-06, direct USB, 800x480 screen), one frame
/// at a time with a settle wait and a graphics-tree check: the labels are
/// literal. `PRESS` (0) is touch-DOWN and `RELEASE` (1) is touch-UP; a click
/// completes on `RELEASE`. A lone `PRESS` on a menu row highlights it (a pressed
/// look) without activating it; `PRESS` then `RELEASE` at the same point
/// activates it; a lone `RELEASE` with no press pending did nothing on a menu
/// button or row (on a Gig View tile a `RELEASE` selected that scene, and on a
/// list it clicks whatever the last `PRESS` pointed at, so it is not neutral
/// everywhere). `TAP` and `DRAG` are complete one-frame gestures and leave no
/// highlight. `DRAG` is a
/// true pointer drag from `(x, y)` to `(to_x, to_y)` whose effect depends on the
/// control under it: on the Tempo dialog's knob it is linear (a DRAG from (125,
/// 325) to (125, 275) changed the preset tempo from 120 to 170 BPM, one BPM per
/// pixel upward, and the reverse drag restored 120), while in a scrolling list
/// the unit derives a velocity from the short gesture and flings. On value
/// controls it is a 1:1 pointer delta applied from where the control currently
/// is, and it grabs only when `(x, y)` is ON the control's current handle: on the
/// Cab editor's microphone pad a 43 px horizontal drag left moved POSITION from
/// 0.0 to 3.7 (about 0.086 per pixel), a 50 px vertical drag down moved DISTANCE
/// from 0.0 to 4.5 (about 0.09 per pixel), POSITION clamps at the centre line, and
/// a drag started at the pad centre after the dot had moved did nothing; on the I/O
/// Settings screen the OUT 1 LEVEL knob moved 0.26 dB per pixel (a 10 px drag up
/// took 0.0 dB to 2.6 dB and the reverse drag restored it). A caller
/// must read the handle position first. On the Grid a `DRAG` on a placed block, or
/// a `PRESS` held then `DRAG`, did not lift the block.
///
/// On the Grid screen a `DRAG` that travels more than about 245 px vertically,
/// starting anywhere (starts from y=479 up to y=300, and from y=0 up to y=160,
/// were tried), opens Gig View when it moves up and the I/O Settings screen when
/// it moves down: a swipe by travel, not an edge gesture (239 px did nothing,
/// 254 px opened Gig View; 260 px down from the top and 340 px down from y=100
/// opened I/O). Shorter drags do nothing on the Grid. A caller that wants a
/// list-sized swipe on the Grid must keep it short. Lists with no overflow
/// ignore a drag; lists that overflow fling (the preset-menu popup, the model
/// picker, the Settings pane, the preset browser's bank-number grid, and the
/// input and output selector popups).
///
/// Both item moves reduce to two frames: lift, then one `TAP` on the target.
/// Grid: a long `PRESS` (held about 1.4 s, no further frames) on the block
/// detaches it into drag mode, then a single `TAP` at the target cell centre drops
/// it there (a block moved from column 2 row 2 to column 4 row 0 in one step).
/// Modes: a `DRAG` in the source tile, then a `TAP` at the target tile centre
/// about 0.6 s later, merges them (Preset dragged onto Scene gave a hybrid). A
/// `TAP` is a pointer-position update plus a release in one frame, so the
/// `PRESS` then `RELEASE` at the target described below is the lower-level
/// equivalent. The red delete target is live while an item is held: never `TAP`
/// at it (about x 610..790, y 30..75 on the Grid, y 8..52 on Modes).
///
/// Grid block move, a recipe of built-in frames observed on CorOS 4.1.0
/// (2026-10-06, verified twice, a column move then a row move): a `PRESS` at the
/// block centre, kept down (detached by the first check at about 0.5 to 1.2 s; 1.6
/// s was used), puts the Grid in drag mode: a drop dot at every cell, the dashed
/// lower route and a red delete bar at the top right (about x 610..790, y 30..75,
/// live: never `PRESS` or `RELEASE` there). A `PRESS` at the centre of the target
/// cell (a pointer-position update while held) shows a ghost block there, and a
/// `RELEASE` at that cell drops the block. On a list, `PRESS` frames at successive
/// points do not scroll it: they are pointer jumps, and the `RELEASE` clicks
/// whatever the last `PRESS` pointed at, while a `PRESS` on one row with the
/// `RELEASE` on another gives no click. `MOVE` had no
/// visible effect in any variant tried: not as hover over menu rows, not while a
/// `PRESS` was held on a row or on the Tempo knob (x and y as a position, as a
/// delta or as 0..1, with and without `to_x` and `to_y`, at 50 ms steps), and a
/// `PRESS` on one row with the `RELEASE` on another gave no click. Further
/// negatives, all with no effect: negative and 5000 values, normalised values,
/// reversed endpoints, a held `PRESS` on a Grid block, a menu row, the Tempo knob
/// or the Cab microphone dot, and unknown event types 5 to 10 and 16 sent alone
/// or during a held press on the knob (the tempo stayed 120 in all 15 knob
/// variants). Its purpose is unknown (an older note says it wakes a dormant
/// framebuffer stream). `PRESS`, `MOVE` and `TAP` frames with `x` and `y` of zero
/// (coordinates omitted on the wire) act at (0, 0) literally: they do not reuse
/// the last pointer position or the last `MOVE` target. A `RELEASE`'s own
/// coordinates are ignored for buttons: a click completed at the `PRESS` point
/// whether the `RELEASE` carried (0, 0) or a far-away point.
///
/// Item drag, observed on the Modes configuration screen (CorOS 4.1.0,
/// 2026-10-06), a recipe of built-in frames and not a new gesture. A `DRAG` frame
/// with `(x, y)` inside a source tile begins a held drag (touch-down plus initial
/// move): the tile lifts and a red delete target appears at the top right
/// (x 610..790, y 8..52, live while an item is lifted: never `PRESS` or
/// `RELEASE` there). During the held drag each `PRESS` frame acts as a
/// pointer-position update for drop-target hit testing, not as a second press:
/// at the centre of a target tile it shows the hybrid preview, and a `PRESS` at a
/// new point moves the preview there. `MOVE` frames never move the lifted item
/// or hover anything. A `RELEASE` drops at the pointer's current hit-test point
/// and the tiles merge (dropped in empty space left of the first tile it merges
/// into the nearest tile; dropped at the source tile's own position with no
/// `PRESS` elsewhere it returns unchanged). A `DRAG` then a lone `RELEASE` does
/// not merge, and `PRESS`, `PRESS`, `RELEASE` without a `DRAG` neither lifts nor
/// previews. A `PRESS` held about 1.5 s on a hybrid splits it while still pressed.
/// A literal `PRESS` then `RELEASE` tap toggled the preset-menu
/// button only 0 times in 10 at a 20 ms gap and 1 in 10 at 80 ms (3 in 3 at
/// 0.9 s), so two frames that close together are not a reliable tap; the single
/// `TAP` frame is.
///
/// All five types are exposed as raw frames with these literal names, each as
/// measured: `TAP` is a click; `DRAG` is a self-contained drag (knobs and pads 1:1,
/// list fling, lifts Modes tiles, a long vertical swipe on the Grid opens Gig View
/// up or I/O Settings down); `PRESS` is touch-down (held about 0.6 s or more it
/// detaches Grid blocks, breaks Modes hybrids and assigns a parameter value to the
/// current scene; a further `PRESS` while an item is held moves the drop target);
/// `RELEASE` is touch-up (its coordinates are ignored for buttons, and it is
/// needed to end a held press that opened a dialog, otherwise the next `TAP` is
/// swallowed); `MOVE` had no observed effect in any encoding; coordinate-less
/// frames act at (0, 0). Hazards of `PRESS` and `RELEASE`: scene assignment on a
/// parameter, a stuck press that blocks later taps, and the delete bar that is
/// live while an item is held.
///
/// A host that sends a lone `PRESS` leaves a pointer DOWN on the unit until a
/// `RELEASE` ends it; see `device::touch_guard`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenMouseType {
    /// Wire value 0. Touch-DOWN on CorOS 4.1.0 (observed).
    Press,
    /// Wire value 1. Touch-UP on CorOS 4.1.0 (observed); completes a press.
    Release,
    /// Wire value 2. No visible effect in any variant tried on CorOS 4.1.0
    /// (observed); purpose unknown.
    Move,
    /// Wire value 3. A complete one-frame tap.
    Tap,
    /// Wire value 4. A complete one-frame pointer drag from `(x, y)` to
    /// `(to_x, to_y)`: it begins a held drag (touch-down plus initial move). A
    /// knob follows it linearly, a list flings, and on an item tile it lifts the
    /// item (`to_x` and `to_y` are then ignored).
    Drag,
}

impl ScreenMouseType {
    /// Every type, in wire-value order.
    pub const ALL: [Self; 5] = [
        Self::Press,
        Self::Release,
        Self::Move,
        Self::Tap,
        Self::Drag,
    ];

    /// The lowercase name used on the gateway: `press`, `release`, `move`, `tap`, `drag`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Press => "press",
            Self::Release => "release",
            Self::Move => "move",
            Self::Tap => "tap",
            Self::Drag => "drag",
        }
    }

    /// Parse a gateway name; exactly the five lowercase names are accepted.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    fn wire(self) -> pa::remote_control_mouse::Type {
        match self {
            Self::Press => pa::remote_control_mouse::Type::Press,
            Self::Release => pa::remote_control_mouse::Type::Release,
            Self::Move => pa::remote_control_mouse::Type::Move,
            Self::Tap => pa::remote_control_mouse::Type::Tap,
            Self::Drag => pa::remote_control_mouse::Type::Drag,
        }
    }
}

/// Build ONE remote-control UPDATE frame carrying exactly the mouse type asked
/// for, with no frame before it and none after it. `to` is the drag
/// destination and is carried only by [`ScreenMouseType::Drag`]; every other
/// type leaves `to_x` and `to_y` at zero. [`DeviceOperation::ScreenMouseFrame`]
/// validates its arguments; this builder does not.
pub fn screen_mouse_frame(
    kind: ScreenMouseType,
    x: f32,
    y: f32,
    to: Option<(f32, f32)>,
) -> OutboundMessage {
    let (to_x, to_y) = if kind == ScreenMouseType::Drag {
        to.unwrap_or((0.0, 0.0))
    } else {
        (0.0, 0.0)
    };
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_REMOTE_CONTROL,
        pa::RemoteControlMessage {
            action: pa::message_action::Enum::Update as i32,
            mouse: Some(pa::RemoteControlMouse {
                x,
                y,
                r#type: kind.wire() as i32,
                to_x,
                to_y,
            }),
            ..Default::default()
        },
    )
}

/// Build a tap at `(x, y)` in screen pixels: exactly ONE frame, the protocol's
/// own `TAP` (mouse type 3, wire bytes `18 03`), with no touch-down or touch-up
/// frame around it.
///
/// Observed on CorOS 4.1.0 (2026-10-06, direct USB): the lone `TAP` frame
/// toggled the preset-menu button at (764, 23) 10 times in 10 and also pressed a
/// dialog check button at (743, 30), and left no row highlighted afterwards.
///
/// This replaces a two-frame tap that sent `RELEASE` then `PRESS`. Under the
/// measured semantics (`PRESS` is touch-down, `RELEASE` is touch-up; see
/// [`ScreenMouseType`]) that was an up that did nothing followed by a down that
/// was never released, so every tap left a stuck pressed pointer on the unit.
/// The next gesture's `RELEASE` completed it, which is why those taps appeared
/// to work (10 in 10 at both 20 ms and 80 ms) and why rows kept a stale pressed
/// look.
pub fn screen_tap(x: f32, y: f32) -> [OutboundMessage; 1] {
    [screen_mouse_frame(ScreenMouseType::Tap, x, y, None)]
}

/// Build the two frames of a press and hold at `(x, y)`: `PRESS` (wire type 0,
/// touch-down) then `RELEASE` (wire type 1, touch-up), which a host sends
/// `profile::REMOTE_HOLD_INTERVAL_MS` apart. The protocol has no long-press
/// frame, so the gap between the two is the whole gesture. The order is the
/// literal one; the earlier order was `RELEASE` then `PRESS`, which is not a
/// long press at all.
///
/// What a hold does depends on what is under it. Observed on CorOS 4.1.0
/// (2026-10-06, direct USB) with these same two frame types sent by hand, not
/// with this builder (observed, not vendor facts):
///
/// - **Grid block detaches** when the `PRESS` is held at least about 0.6 s
///   (0.15, 0.3 and 0.5 s did not; 0.7, 0.9 and 1.1 s did; the threshold lies
///   between 0.5 and 0.7 s). Measured.
/// - **Modes hybrid tile breaks** at about 1.5 s. That is the value that was
///   tried; the floor was not pinned.
/// - **A parameter control** (the Tempo knob or the Tempo LED switch) ASSIGNS
///   THE VALUE TO THE CURRENT SCENE at about 0.9 s then `RELEASE` (a scene badge
///   appears; on an already assigned parameter a dialog offers CANCEL and
///   UNASSIGN). Also a value that was tried; the floor was not pinned. On a
///   toggle even `PRESS` then `RELEASE` gaps of 0.0 s and 0.05 s assigned
///   instead of toggling; only the single `TAP` frame toggled it.
///
/// **The 1200 ms the gateway waits is a host choice**, not a measured
/// threshold. It is above the Grid detach threshold (about 0.6 s) and above the
/// 0.9 s that assigned a parameter, but BELOW the 1.5 s that was used to break a
/// Modes hybrid, whose floor was not pinned, so a 1200 ms hold is not shown to
/// break one. The gateway's own `long_tap_screen` (these two frames 1200 ms
/// apart) has not been run on hardware. HAZARD: a hold, or any `PRESS` then `RELEASE` pair, on a parameter
/// control can create scene assignments; use [`screen_tap`] for a plain click.
///
/// If the `RELEASE` cannot be sent the pointer is left down; see
/// [`DeviceOperation::abort_release_frame`] and `device::touch_guard`.
pub fn screen_hold(x: f32, y: f32) -> [OutboundMessage; 2] {
    [
        screen_mouse_frame(ScreenMouseType::Press, x, y, None),
        screen_mouse_frame(ScreenMouseType::Release, x, y, None),
    ]
}

/// Build one touchscreen swipe (a scroll fling) for the QC remote-control
/// protocol: exactly ONE remote-control UPDATE frame, mouse type DRAG, carrying
/// both endpoints (`x`, `y` and `to_x`, `to_y`).
///
/// There is deliberately no touch-down (`PRESS`) frame before it and no touch-up
/// (`RELEASE`) frame after it. Observed on a Quad Cortex
/// running CorOS 4.1.0 (2026-10-06, direct USB, 800x480 screen, measured on the
/// preset menu popup and one Settings content pane):
///
/// - the lone DRAG frame scrolls the list reliably (30 of 30 alternating
///   scrolls) and leaves no row highlighted, because no press ever lands on a
///   row;
/// - the earlier three-frame sequence (`RELEASE`, DRAG, `PRESS`, 20 ms apart)
///   did not scroll at that pace, and its final `PRESS` was a touch-down that
///   was never released: it left a stuck pressed pointer on the end-point row,
///   and the next gesture's `RELEASE` then activated that row (CPU Monitor,
///   Modes Configuration, Swap Scene A and Create New were opened that way).
///   Spaced 0.4 s to 0.6 s apart it scrolled but left the begin and end rows
///   highlighted; at 20 ms the end frame cut the unit's own drag animation short.
///
/// The frame is a true pointer drag whose effect depends on the control under it
/// (observed on CorOS 4.1.0, 2026-10-06). On the Tempo dialog's knob it is linear,
/// one BPM per pixel (a DRAG from (125, 325) to (125, 275) took the preset tempo
/// from 120 to 170 and the reverse drag restored 120). In a scrolling list the
/// unit derives a velocity from the short gesture and flings: the menu content
/// moved about 8 times the finger distance for 10 to 30 px.
///
/// The scroll distance is a fling that depends on the finger distance only (the
/// pixels between the two endpoints), the same in both directions: 8 px or less
/// does not scroll; 10 px moves about 60 px of content, 15 px about 120, 20 px
/// about 165-180, 25 px about 205, 30 px about 255, 40 px about 300 and 50 px or
/// more runs to the end of the list. The device protocol carries both endpoints
/// in this one dedicated DRAG message, so hosts must not approximate a swipe
/// with disconnected taps.
pub fn screen_drag(x: f32, y: f32, to_x: f32, to_y: f32) -> [OutboundMessage; 1] {
    [screen_mouse_frame(
        ScreenMouseType::Drag,
        x,
        y,
        Some((to_x, to_y)),
    )]
}

/// Fewest interpolated MOVE frames a stepped screen drag may carry.
pub const SCREEN_DRAG_MIN_STEPS: u32 = 2;
/// Most interpolated MOVE frames a stepped screen drag may carry.
pub const SCREEN_DRAG_MAX_STEPS: u32 = 40;
/// Interpolated MOVE frames of a stepped screen drag when the caller names none.
pub const SCREEN_DRAG_DEFAULT_STEPS: u32 = 18;
/// Longest start dwell, in milliseconds, a stepped screen drag may request.
pub const SCREEN_DRAG_MAX_HOLD_MS: u32 = 1500;
/// Start dwell, in milliseconds, of a stepped screen drag when the caller names none.
pub const SCREEN_DRAG_DEFAULT_HOLD_MS: u32 = 200;
/// Dwell, in milliseconds, at the end point before the touch is released, so
/// the surface can settle. It does not depend on the start dwell.
pub const SCREEN_DRAG_END_HOLD_MS: u32 = 300;
/// Longest total duration, in milliseconds, of a stepped screen drag, measured
/// from the first frame to the last.
pub const SCREEN_DRAG_MAX_DURATION_MS: u64 = 5000;

const fn frames_for_ms(milliseconds: u32) -> u32 {
    let interval = profile::REMOTE_DRAG_INTERVAL_MS as u32;
    milliseconds.div_ceil(interval)
}

/// Zero-displacement MOVE frames sent at the start point: `hold_ms` rounded up
/// to whole frame intervals.
pub const fn screen_drag_hold_frames(hold_ms: u32) -> u32 {
    frames_for_ms(hold_ms)
}

/// Zero-displacement MOVE frames sent at the end point:
/// [`SCREEN_DRAG_END_HOLD_MS`] rounded up to whole frame intervals.
pub const fn screen_drag_end_frames() -> u32 {
    frames_for_ms(SCREEN_DRAG_END_HOLD_MS)
}

/// Total frames of a stepped screen drag: the touch-down, the start dwell, the
/// interpolated moves, the end dwell and the touch-up.
pub const fn screen_drag_frame_count(steps: u32, hold_ms: u32) -> u32 {
    1 + screen_drag_hold_frames(hold_ms) + steps + screen_drag_end_frames() + 1
}

/// Milliseconds from the first frame to the last of a stepped screen drag.
pub const fn screen_drag_duration_ms(steps: u32, hold_ms: u32) -> u64 {
    (screen_drag_frame_count(steps, hold_ms) as u64 - 1) * profile::REMOTE_DRAG_INTERVAL_MS
}

/// The longest permitted stepped drag must stay inside its duration cap.
const _: () = assert!(
    screen_drag_duration_ms(SCREEN_DRAG_MAX_STEPS, SCREEN_DRAG_MAX_HOLD_MS)
        <= SCREEN_DRAG_MAX_DURATION_MS
);
/// A stepped drag must be paced slower than a screen tap's two frames, or it is
/// not the slower gesture it exists to be.
const _: () = assert!(profile::REMOTE_DRAG_INTERVAL_MS > profile::REMOTE_GESTURE_INTERVAL_MS);

/// Build a touchscreen drag as a stream of MOVE frames, for hosts that pace
/// every frame at `profile::REMOTE_DRAG_INTERVAL_MS`. It is the shape of a held
/// finger sliding across the panel, unlike the single DRAG frame of
/// [`screen_drag`].
///
/// **Inert (observed on CorOS 4.1.0, 2026-10-06):** `MOVE` had no observed
/// effect in any encoding, so this stream does not scroll or move anything; see
/// [`DeviceOperation::ScreenDragSteps`]. The gateway does not send it.
/// [`screen_drag`] (one `DRAG` frame) is the unit's own drag.
///
/// Frame order, all remote-control UPDATE messages (message 72):
///
/// 1. touch-down at `(x, y)` - wire type `PRESS`;
/// 2. [`screen_drag_hold_frames`]`(hold_ms)` MOVE frames at `(x, y)`;
/// 3. `steps` MOVE frames interpolated linearly from the start to the end
///    point, the last of them exactly at `(to_x, to_y)`;
/// 4. [`screen_drag_end_frames`]`()` MOVE frames at `(to_x, to_y)`;
/// 5. touch-up at `(to_x, to_y)` - wire type `RELEASE`.
///
/// No frame carries a DRAG type or a destination: this is the stream of
/// positions a held finger would produce. The function does not validate its
/// arguments; [`DeviceOperation::ScreenDragSteps`] does, and is the supported
/// entry point.
pub fn screen_drag_steps(
    x: f32,
    y: f32,
    to_x: f32,
    to_y: f32,
    steps: u32,
    hold_ms: u32,
) -> Vec<OutboundMessage> {
    let frame = remote_mouse_frame;
    let mut frames = Vec::with_capacity(screen_drag_frame_count(steps, hold_ms) as usize);
    // The labels are literal (observed on CorOS 4.1.0, 2026-10-06; see
    // ScreenMouseType): PRESS is the touch-down that begins the touch and
    // RELEASE is the touch-up that ends it. Before 2026-10-06 this stream began
    // with RELEASE and ended with PRESS, which left a pointer down.
    frames.push(frame(x, y, pa::remote_control_mouse::Type::Press));
    for _ in 0..screen_drag_hold_frames(hold_ms) {
        frames.push(frame(x, y, pa::remote_control_mouse::Type::Move));
    }
    for step in 1..=steps {
        let (step_x, step_y) = if step == steps {
            (to_x, to_y)
        } else {
            let fraction = f64::from(step) / f64::from(steps);
            (
                (f64::from(x) + (f64::from(to_x) - f64::from(x)) * fraction) as f32,
                (f64::from(y) + (f64::from(to_y) - f64::from(y)) * fraction) as f32,
            )
        };
        frames.push(frame(step_x, step_y, pa::remote_control_mouse::Type::Move));
    }
    for _ in 0..screen_drag_end_frames() {
        frames.push(frame(to_x, to_y, pa::remote_control_mouse::Type::Move));
    }
    frames.push(screen_drag_release(to_x, to_y));
    frames
}

fn remote_mouse_frame(x: f32, y: f32, r#type: pa::remote_control_mouse::Type) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_REMOTE_CONTROL,
        pa::RemoteControlMessage {
            action: pa::message_action::Enum::Update as i32,
            mouse: Some(pa::RemoteControlMouse {
                x,
                y,
                r#type: r#type as i32,
                ..Default::default()
            }),
            ..Default::default()
        },
    )
}

/// The touch-up frame of a gesture that ends at `(to_x, to_y)`: wire type
/// `RELEASE`. It is byte-identical to the last frame [`screen_drag_steps`] and
/// [`screen_hold`] build, so a host that stops mid-gesture can send it alone to
/// avoid leaving the pointer down.
pub(super) fn screen_drag_release(to_x: f32, to_y: f32) -> OutboundMessage {
    remote_mouse_frame(to_x, to_y, pa::remote_control_mouse::Type::Release)
}

impl DeviceOperation {
    /// The frame a host sends once, best effort, if it stops this operation
    /// after the touch-down was delivered but before the touch-up was, so the
    /// device is not left with a pointer down. It is a `RELEASE`, never a
    /// `PRESS`: a `PRESS` would itself be a stuck touch.
    ///
    /// `Some` for the two operations that press and later release,
    /// [`Self::ScreenHold`] (a `RELEASE` at the held point) and
    /// [`Self::ScreenDragSteps`] (a `RELEASE` at the end point). `None` for
    /// everything else: a tap ([`Self::ScreenTap`]) and a swipe
    /// ([`Self::ScreenDrag`]) are one frame that begins no held touch, and a raw
    /// mouse frame is one frame whose held press the host's touch guard tracks
    /// instead (see `device::touch_guard`).
    ///
    /// A host sends it only when a frame after the touch-down failed (so the
    /// touch-down is known to have been accepted), never when the touch-down
    /// itself failed (nothing is known to be pressed), and only on the same
    /// session the gesture began on. For a two-frame hold the failed frame can
    /// only be the touch-up itself, so this is one second attempt at the
    /// identical `RELEASE`; a `RELEASE` with nothing pending was observed to do
    /// nothing on a menu button or row, which is what makes that second attempt
    /// safe when the first one did arrive. It is never sent twice.
    pub fn abort_release_frame(&self) -> Option<OutboundMessage> {
        match self {
            Self::ScreenDragSteps { to_x, to_y, .. } => Some(screen_drag_release(*to_x, *to_y)),
            Self::ScreenHold { x, y } => Some(screen_drag_release(*x, *y)),
            _ => None,
        }
    }
}
