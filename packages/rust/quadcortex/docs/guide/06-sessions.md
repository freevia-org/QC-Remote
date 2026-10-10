# Sessions, handshake, suspension and recovery

How a connection starts, how the library keeps it alive, what happens when the
unit asks the host to stand down, and how to recover. This is the part that makes
a long-running host correct. The offline snippets drive the real state machines
with synthetic replies; one compile-only sketch shows the call order a host
follows.

Prerequisite: [Concepts](crate::guide::concepts).

## Two state machines, composed by one owner

Two machines are involved. They answer different questions.

| Machine | Question it answers | Type |
| --- | --- | --- |
| Connection policy | Should I retry the connection, send the next handshake attempt, send a keepalive, or give up on this link? | [`TransportRuntime`](crate::transport::TransportRuntime) with its [`SessionPhase`](crate::transport::SessionPhase) |
| Startup lifecycle | What is the next protocol message to send, and has the unit reached a state where I can act on it? | [`SessionDriver`](crate::session::SessionDriver) with its [`DeviceStartupPhase`](crate::device::initialization::DeviceStartupPhase) |

[`SessionRuntime`](crate::device::owner::SessionRuntime) owns one of each (plus
the state reducer, the delivery journal, the suspension gate and the backup
entry) and keeps them consistent. Its `transport`, `state` and `lifecycle` fields
are public, but its own doc warns that they "do not confer ownership of a second
connection or independent state". Treat `SessionRuntime` as the thing you lock
once per transition.

## Connection policy: `TransportRuntime`

Every method takes the host's monotonic `now_ms`. The runtime never reads a
clock and never sleeps.

| `SessionPhase` | Meaning |
| --- | --- |
| `Disconnected` | No device open, automatic reconnection off. |
| `Searching` | No device open; the host retries when [`reconnect_due`](crate::transport::TransportRuntime::reconnect_due). |
| `Handshaking` | The transport is open; reset-comms handshake attempts are in progress. |
| `Syncing` | The handshake completed; state is not yet synchronized. |
| `Ready` | The handshake completed and state is synchronized. |

The policy's constants live in the profile and in `transport`:
`RECONNECT_INTERVAL_MS`, `HANDSHAKE_ATTEMPT_TIMEOUT_MS`, `HANDSHAKE_TIMEOUT_MS`,
`KEEPALIVE_INTERVAL_MS`, and
[`RECONNECT_BACKOFF_CAP_MS`](crate::transport::RECONNECT_BACKOFF_CAP_MS).

```rust
use prost::Message;
use quadcortex::protocol::profile;
use quadcortex::protocol::proto::cortex_protobuf_v2 as pa;
use quadcortex::transport::{
    reconnect_backoff_ms, ReportLayout, SessionPhase, TransportRuntime, RECONNECT_BACKOFF_CAP_MS,
};

let mut transport = TransportRuntime::new(0);
assert_eq!(transport.phase(), SessionPhase::Searching);
assert!(transport.reconnect_due(0), "the first attempt is due at once");

// The host opens the device (not shown), then tells the runtime.
transport.reconnect_attempted(0);
transport.transport_opened(0);
assert_eq!(transport.phase(), SessionPhase::Handshaking);

// Attempt 1. The host supplies a fresh opaque session id for every attempt.
let first = transport.next_handshake_write(0, "session-a").expect("attempt 1 is due");
assert_eq!(first.attempt, 1);
assert_eq!(first.request_id(), 1); // the request id is the attempt number
assert_eq!(first.message.message_type, 52); // ResetCommsBuffers
assert_eq!(first.layout, ReportLayout::ReportIdPrefixed);

// No second attempt while the first one's reply window is open.
assert!(transport.awaiting_handshake_reply(1));
assert!(transport.next_handshake_write(1, "session-b").is_none());

// A reply counts only if it echoes this attempt's session id.
let reply = |id: &str| {
    pa::ResetCommsBuffersMessage {
        session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(id.into())),
        ..Default::default()
    }
    .encode_to_vec()
};
assert!(!first.matches_reply(&reply("someone-else")));
assert!(first.matches_reply(&reply("session-a")));

// When the window closes, attempt 2 is due. Without a caller-chosen layout
// the runtime alternates between the two native layouts by attempt parity.
let second = transport
    .next_handshake_write(profile::HANDSHAKE_ATTEMPT_TIMEOUT_MS, "session-b")
    .expect("attempt 2 is due");
assert_eq!((second.attempt, second.layout), (2, ReportLayout::BodyOnly));

// The whole handshake has a budget.
assert!(!transport.handshake_timed_out(profile::HANDSHAKE_TIMEOUT_MS - 1));
assert!(transport.handshake_timed_out(profile::HANDSHAKE_TIMEOUT_MS));

// Once the handshake completes, a keepalive falls due. take_keepalive reserves
// the next deadline at once, so polling it in a loop cannot send two.
transport.handshake_completed(6_000, false);
assert_eq!(transport.phase(), SessionPhase::Syncing);
let ping = transport.take_keepalive(6_000).expect("a keepalive is due");
assert_eq!(ping.message_type, 32);
assert!(transport.take_keepalive(6_500).is_none());
assert!(transport.take_keepalive(6_000 + profile::KEEPALIVE_INTERVAL_MS).is_some());

// Two failed reads in a row mean the link is lost; a success resets the count.
assert!(!transport.read_failed());
transport.read_succeeded();
assert!(!transport.read_failed());
assert!(transport.read_failed());

// Connect attempts that opened the device and then failed back off, capped.
assert_eq!(reconnect_backoff_ms(1), profile::RECONNECT_INTERVAL_MS);
assert_eq!(reconnect_backoff_ms(2), 2 * profile::RECONNECT_INTERVAL_MS);
assert_eq!(reconnect_backoff_ms(40), RECONNECT_BACKOFF_CAP_MS);
```

Notes from the doc comments:

- The alternating layout is "an intentional, accepted probing heuristic". A host
  that knows its native API's layout passes it with
  [`next_handshake_write_with_layout`](crate::transport::TransportRuntime::next_handshake_write_with_layout).
- [`disconnect`](crate::transport::TransportRuntime::disconnect) takes a
  `recover` flag: `true` goes to `Searching` with the next attempt one reconnect
  interval away, `false` goes to `Disconnected` with no automatic retry.
  [`connect_failed`](crate::transport::TransportRuntime::connect_failed) applies
  the backoff after an attempt that opened the device but did not complete;
  [`request_reconnect`](crate::transport::TransportRuntime::request_reconnect)
  is the operator's "try now" and clears it.
- A transport open that fails is not the same as a connect that opens and then
  fails. Only the second feeds the backoff.

## Startup lifecycle: `SessionDriver`

After the transport is open, the library runs a **staged** startup. The driver
decides what to send; the host only writes it and reports whether the write was
accepted. The stages, in order:

| `DeviceStartupPhase` | The driver sends | It advances when |
| --- | --- | --- |
| `SessionValidating` | ResetCommsBuffers carrying your session id | The reply echoes that session id. Any other id ends the session as `Invalid(SessionMismatch)`. |
| `VersionValidating` | A Version READ | A Version UPDATE whose device type is the Quad Cortex. Another device type is `Invalid(UnsupportedDevice)` (a device type of `1` is only *believed* to be a compact variant of the unit; see the [limits register](crate::guide::limits)); an explicit "version invalid" flag is `Invalid(IncompatibleControllerVersion)`. |
| `Disconnected` | Unsubscribe CPU load, then Connection(false) | The host reports that batch accepted; the driver then enters `Building`. |
| `Building` | Connection(true) and a ModelRepo READ | A parsed, non-empty model catalog. An unparsable or empty catalog fails the session. |
| `Initializing` | A ModuleStats READ | A non-empty ModuleStats UPDATE. |
| `Booting` | The boot burst of reads (see [`boot_initialization`](crate::commands::boot_initialization)) | The Updater UPDATE arrives. |
| `Connected` | A system-time sync, then the seed reads | See below. |
| `Invalid`, `Failed` | Terminal. | Never. |

The order is "behavioral protocol", in the words of the code, and the batches are
kept apart on purpose; for example the building writes "must not be merged into
the later subscription burst". Do not reorder or merge what
[`next_batch`](crate::session::SessionDriver::next_batch) gives you.

After `Connected`, a **seed** completes the picture. The driver first queues the
system-time sync, which must be submitted before any seed read. Its value is
*believed* to be UTC milliseconds since the Unix epoch, and what the unit does
with it is not established (see the [limits register](crate::guide::limits)). Then it waits for
the unit to publish the current preset
([`INITIAL_SYNC_TIMEOUT_MS`](crate::protocol::profile::INITIAL_SYNC_TIMEOUT_MS)),
requests it if it does not, and reads whichever of the required types it has not
yet seen: setlist position, scene, mode, master volume and the dirty flag
([`REQUIRED_SEED_TYPES`](crate::device::initialization::REQUIRED_SEED_TYPES)).
[`synchronization`](crate::session::SessionDriver::synchronization) becomes
`Some(true)` when the preset and every required type were observed, and
`Some(false)` if the seed ended on a timeout. A host passes that value to
[`TransportRuntime::synchronization_completed`](crate::transport::TransportRuntime::synchronization_completed),
which moves the transport from `Syncing` to `Ready`, or keeps it in `Syncing`.

> The exact messages in each batch are this library's policy: they are what the
> library sends. That a unit accepts them is a property of firmware this guide
> does not establish. The doc on
> [`DeviceStartupError::RebuildLoop`](crate::device::initialization::DeviceStartupError::RebuildLoop)
> records one case where a unit kept announcing a disconnect and never answered
> the model-catalog read; the driver gives up after
> [`MAX_STARTUP_REBUILDS`](crate::device::initialization::MAX_STARTUP_REBUILDS)
> rebuilds rather than waiting forever.

The driver is driven by four calls:

- [`next_batch(now_ms, unix_time_ms)`](crate::session::SessionDriver::next_batch):
  pull the next [`LifecycleBatch`](crate::session::LifecycleBatch). One batch is
  outstanding at a time. Only the system-time sync uses the wall clock.
- [`dispatched(&ticket, outcome)`](crate::session::SessionDriver::dispatched):
  report that the writer accepted the batch, or failed. A failure is terminal. A
  duplicate or old-session callback returns `false` and changes nothing.
- [`observe(type, payload, now_ms)`](crate::session::SessionDriver::observe): feed
  each inbound message.
- [`is_current(&ticket)`](crate::session::SessionDriver::is_current): check before
  *each message* of a batch you are writing, because a rebuild can revoke the rest
  of it.

```rust
use prost::Message;
use quadcortex::device::initialization::{DeviceStartupError, DeviceStartupPhase};
use quadcortex::protocol::proto::cortex_protobuf_v2 as pa;
use quadcortex::session::{DispatchOutcome, SessionDriver, SessionFailure};

let echo = |id: &str| {
    pa::ResetCommsBuffersMessage {
        session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(id.into())),
        ..Default::default()
    }
    .encode_to_vec()
};

let mut driver = SessionDriver::start(1, "session-1".into(), 0);

// The reset-comms handshake is the first batch. Only one is outstanding.
let reset = driver.next_batch(0, 0).expect("queued by start");
assert_eq!(reset.messages[0].message_type, 52);
assert!(driver.next_batch(1, 0).is_none());
assert!(driver.dispatched(&reset.ticket, DispatchOutcome::Accepted));
assert!(!driver.dispatched(&reset.ticket, DispatchOutcome::Accepted), "a duplicate is ignored");

// The reply echoes our session id, so startup moves on to the Version READ.
driver.observe(52, &echo("session-1"), 10);
assert_eq!(driver.phase(), DeviceStartupPhase::VersionValidating);
let version = driver.next_batch(10, 0).expect("the Version read");
assert_eq!(version.messages[0].message_type, 10);

// A reply that echoes somebody else's session id ends a session for good.
let mut other = SessionDriver::start(1, "session-2".into(), 0);
other.next_batch(0, 0);
other.observe(52, &echo("not-ours"), 10);
assert_eq!(
    other.failure(),
    Some(&SessionFailure::Invalid(DeviceStartupError::SessionMismatch))
);
assert!(!other.is_active());
assert!(other.next_batch(10, 0).is_none(), "a failed driver hands out nothing");
```

`examples/startup_walkthrough.rs` runs the complete sequence, from the first
handshake message to a synchronized session, against synthetic replies, and prints
each batch. It is the quickest way to see all the stages in order.

```text
cargo run -p quadcortex --example startup_walkthrough
```

### Deadlines

Staged startup has one readiness budget,
`profile::READY_WAIT_TIMEOUT_MS`, measured from `start`. It is re-armed by a
genuine device-announced rebuild and stops applying once the session reaches
`Connected`. A reply that arrives *after* the deadline does not rescue a startup:
it fails with [`SessionFailure::StartupTimeout`](crate::session::SessionFailure::StartupTimeout),
carrying the phase it was stuck in.

## Sessions, epochs and stale work

Every failure above is local to one `SessionDriver`. The reason a host can be
long-lived is that work is **bound to the session it was started in**:

- [`SessionDriver::epoch`](crate::session::SessionDriver::epoch) is the session id
  plus a revision. A device-announced rebuild (a Connection(false) after startup
  began) advances the revision and discards queued work.
- A [`LifecycleTicket`](crate::session::LifecycleTicket) is bound to the session
  and revision it was issued under, so an old callback cannot be mistaken for the
  current batch.
- The owner stamps every inbound frame with an
  [`ObservationTicket`](crate::device::owner::ObservationTicket). After a session
  ends, [`observe_state`](crate::device::owner::SessionRuntime::observe_state)
  returns `Ok(None)` for such a ticket instead of applying it.

```rust
use prost::Message;
use quadcortex::device::owner::SessionRuntime;
use quadcortex::protocol::proto::cortex_protobuf_v2 as pa;
use quadcortex::transport::SessionPhase;

let mut owner = SessionRuntime::new(0);
let connection = owner.opened(0)?;
owner.next_handshake_write(0, "session-1", None).expect("attempt 1 is due");

let scene = pa::SceneMessage {
    action: pa::message_action::Enum::Update as i32,
    selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(3)),
    ..Default::default()
}
.encode_to_vec();

// A ticket stamped while this session was current is accepted and applied.
let ticket = owner.capture_observation(connection)?;
assert!(owner.accepts_observation(&ticket));
assert!(owner.observe_state(&ticket, 13, &scene)?.is_some());
assert_eq!(owner.state.snapshot().active_scene, 3);

// The link drops. The owner stops outstanding deliveries, clears the session's
// state, and the transport goes back to searching.
owner.disconnect(100, true);
assert_eq!(owner.transport.phase(), SessionPhase::Searching);
assert!(!owner.accepts_connection(connection));
assert!(!owner.accepts_observation(&ticket));
assert!(owner.observe_state(&ticket, 13, &scene)?.is_none(), "a late frame is ignored");
assert_eq!(owner.state.snapshot().active_scene, 0, "session state does not survive");
# Ok::<(), String>(())
```

## Suspension: the unit asks the host to stand down

The unit can send a **SuspendConnection** message asking the host to stop driving
it. The module doc describes what follows: "While suspended, the unit locks its
screen and volume knob and stops answering requests." The owner therefore
**refuses to put any client request on the wire** until the suspension ends.

What is and is not refused, from the doc comments:

- Refused: a new client request. The error starts with
  [`DEVICE_SUSPENDED_MARKER`](crate::device::owner::DEVICE_SUSPENDED_MARKER) so you
  can classify it without parsing the rest, and says the request "was not sent".
- Not refused: the keepalive, the startup lifecycle, and the backup this host
  started, which is "the very transfer the suspension exists for".
- Refused with different wording: another frame of a request whose earlier frame
  already reached the device. Its outcome is then genuinely unknown, and the
  message says "the device effect may be uncertain".

The suspension ends on either of two device signals within a session:
SuspendConnection with `suspend = false`, or a GeneralSettings message reporting
the screen lock released after it was seen engaged. **Nothing ends it on a
timer**, by design: sending into a unit whose screen is still locked is the
failure the gate exists to prevent. A resume that never arrives leaves the gate up
until the session is reset, and the diagnostics say how long it has been up.

At the end of a backup the module doc says the unit releases the screen lock
about a millisecond *before* it sends the resume. The release lifts the gate; the
resume that follows still belongs to that suspension. It is recorded on the ended
suspension ([`EndedSuspension::resume_reason`](crate::device::owner::EndedSuspension::resume_reason)),
its reason is still checked for a resynchronization, and it is not counted as a
stray resume. A resume that matches no suspension of its session is counted in
`unmatched_resumes`.

Only a suspension with the BACKUP reason has been observed on hardware, per the
module doc. The other reasons the schema names (factory reset, settings reset),
and any reason this build does not know, are handled defensively: the owner assumes
cached state is invalid and asks the host for a full resynchronization through
[`take_suspension_resync`](crate::device::owner::SessionRuntime::take_suspension_resync).

```rust
use prost::Message;
use quadcortex::device::owner::{SessionRuntime, SuspensionEnd, DEVICE_SUSPENDED_MARKER};
use quadcortex::protocol::proto::cortex_protobuf_v2 as pa;

let mut owner = SessionRuntime::new(0);
let connection = owner.opened(0)?;
owner.next_handshake_write(0, "session-1", None).expect("attempt 1 is due");

let frame = |suspend: bool| {
    pa::SuspendConnectionMessage {
        suspend: Some(pa::suspend_connection_message::Suspend::Suspend(suspend)),
        reason: Some(pa::suspend_connection_message::ReasonOneOf::Reason(
            pa::suspend_connection_message::Reason::Backup as i32,
        )),
        ..Default::default()
    }
    .encode_to_vec()
};
let observe = |owner: &mut SessionRuntime, payload: &[u8]| {
    let ticket = owner.capture_observation(connection)?;
    owner.observe_state(&ticket, 53, payload).map(|_| ())
};

// Before any suspension a request is admitted.
assert!(owner.plan_operation(1_000, 3_000).is_ok());

// The device suspends the host (synthetic frame; reason BACKUP).
observe(&mut owner, &frame(true))?;
assert!(owner.is_suspended());
let refusal = owner.plan_operation(1_100, 3_000).unwrap_err();
assert!(refusal.starts_with(DEVICE_SUSPENDED_MARKER));
assert!(refusal.contains("the request was not sent"));
assert_eq!(owner.suspension().active.as_ref().map(|s| s.reason.as_str()), Some("BACKUP"));
assert_eq!(owner.suspension().refused_requests, 1);

// The device resumes. Requests are admitted again, and a BACKUP resume does
// not ask for a resynchronization.
observe(&mut owner, &frame(false))?;
assert!(!owner.is_suspended());
assert!(owner.plan_operation(1_200, 3_000).is_ok());
assert_eq!(owner.take_suspension_resync(), None);
let ended = owner.suspension().last_ended.expect("the last suspension is recorded");
assert_eq!(ended.ended_by, SuspensionEnd::DeviceResumed);
assert_eq!(ended.resume_reason.as_deref(), Some("BACKUP"));
# Ok::<(), String>(())
```

## Recovery

What to do, by symptom. Every row is a behaviour of the types above.

| You observe | Do this |
| --- | --- |
| `read_failed()` returns `true` (two failed reads in a row) | Treat the link as lost: `owner.disconnect(now, true)`, then retry when `reconnect_due`. |
| `terminal_read_failed()` (the OS reader can no longer continue) | Same, without retrying on the same handle. |
| `handshake_timed_out()` | Close the device, call `disconnect(now, true)`, then `connect_failed(now)` so the next attempt backs off. |
| `SessionDriver::failure()` is `Some(_)` | The driver does not recover from a failure. Tear down and start a new session with a fresh session id. |
| `StartupTimeout`, or `Invalid(_)` | As above. `Invalid` records why the device or session was rejected; see [`DeviceStartupError`](crate::device::initialization::DeviceStartupError). |
| A ticket is no longer accepted | The session was replaced. Discard work bound to it; do not re-issue it into the new session. |
| The refusal starts with `DEVICE_SUSPENDED_MARKER` | Wait for the device to resume. The refusal text itself says that if the unit's screen is no longer locked, resetting the device session clears the gate. |
| `take_suspension_resync()` returns a reason | Perform a full session reset and let startup rebuild state. |
| The operator asks to reconnect now | `request_reconnect(now)`: it clears the failure backoff. |

A reset **clears session state on purpose**: staged publications, the preset
workflow, the backup entry, retained catalogs, in-flight verifications, queued
deliveries and the state snapshot. Do not expect anything session-scoped to
survive one.

## The call order, as a host loop

The sketch below is the shape of a connect loop around the owner. It is
**compiles but was not run against a device**. It needs a real implementation of
the `HidLink` seam, an open device, and a unit that answers. It is a sketch of
call order, not a tested implementation: it omits timeouts for individual reads,
telemetry, and the threading a real host needs. For one complete host, read the
repository's `quadcortex-windows` crate; see [Transports](crate::guide::transports).

```rust,no_run
use quadcortex::commands::OutboundMessage;
use quadcortex::device::owner::SessionRuntime;
use quadcortex::device::session::DispatchOutcome;
use quadcortex::protocol::profile::MESSAGE_TYPE_MODEL_REPO;
use quadcortex::state::parse_model_repo;
use quadcortex::transport::ReportLayout;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The host's side of the USB/HID boundary. You implement this.
trait HidLink {
    fn write_report(&mut self, report: &[u8]) -> std::io::Result<()>;
    fn read_report(&mut self, timeout_ms: u32) -> std::io::Result<Option<Vec<u8>>>;
    fn sleep_ms(&mut self, milliseconds: u64);
}

type BoxError = Box<dyn std::error::Error>;

fn write_message<L: HidLink>(
    link: &mut L,
    owner: &SessionRuntime,
    message: &OutboundMessage,
    layout: ReportLayout,
) -> Result<(), BoxError> {
    let reports = owner.transport.encode_reports_with_session(message, layout)?;
    for (index, report) in reports.iter().enumerate() {
        link.write_report(report)?;
        if let Some(delay) = quadcortex::transport::TransportRuntime::report_batch_delay_ms(index + 1) {
            link.sleep_ms(delay);
        }
    }
    Ok(())
}

/// Send whatever the lifecycle has ready, one identified batch at a time.
fn drive_lifecycle<L: HidLink>(
    link: &mut L,
    owner: &mut SessionRuntime,
    layout: ReportLayout,
    now_ms: u64,
) -> Result<(), BoxError> {
    let unix_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    while let Some(batch) = owner.lifecycle.as_mut().and_then(|l| l.next_batch(now_ms, unix_ms)) {
        for message in &batch.messages {
            // A rebuild can revoke the rest of a batch while we are writing it.
            let current = owner.lifecycle.as_ref().is_some_and(|l| l.is_current(&batch.ticket));
            if !current {
                break;
            }
            if let Err(error) = write_message(link, owner, message, layout) {
                if let Some(lifecycle) = owner.lifecycle.as_mut() {
                    lifecycle.dispatched(&batch.ticket, DispatchOutcome::Failed);
                }
                return Err(error);
            }
        }
        if let Some(lifecycle) = owner.lifecycle.as_mut() {
            lifecycle.dispatched(&batch.ticket, DispatchOutcome::Accepted);
        }
    }
    Ok(())
}

/// Run the handshake and startup until the session is synchronized or fails.
/// Returns the connection number to use for later reads.
fn connect<L: HidLink>(
    link: &mut L,
    owner: &mut SessionRuntime,
    clock: &Instant,
    fresh_session_id: impl Fn() -> String,
) -> Result<u64, BoxError> {
    let now = || clock.elapsed().as_millis() as u64;
    let connection = owner.opened(now())?;
    let mut layout = ReportLayout::ReportIdPrefixed;
    loop {
        if owner.transport.handshake_timed_out(now()) {
            return Err("handshake timed out".into());
        }
        // One attempt per reply window; the owner installs a fresh lifecycle.
        if let Some(attempt) = owner.next_handshake_write(now(), fresh_session_id(), Some(layout)) {
            layout = attempt.layout;
        }
        drive_lifecycle(link, owner, layout, now())?;

        if let Some(report) = link.read_report(100)? {
            // Stamp the read before anything else touches it.
            let epoch = owner.capture_report_epoch(connection)?;
            if let Some((frame, ticket)) = owner.push_native_report(connection, epoch, &report, now())? {
                if frame.message_type == MESSAGE_TYPE_MODEL_REPO {
                    if owner.queue_model_repo(ticket.clone(), frame.payload.clone()) {
                        if let Some(work) = owner.take_model_repo() {
                            let catalog = parse_model_repo(&work.payload).map_err(|e| e.to_string());
                            owner.complete_model_repo(&work.ticket, catalog, now())?;
                        }
                    }
                } else {
                    owner.observe_lifecycle(&ticket, frame.message_type, &frame.payload, now());
                    owner.observe_state(&ticket, frame.message_type, &frame.payload)?;
                }
            }
        }

        // The driver reports its first synchronization result exactly once.
        let change = owner.lifecycle.as_mut().and_then(|l| l.take_synchronization_change());
        if let Some(synchronized) = change {
            owner.transport.synchronization_completed(now(), synchronized);
            return Ok(connection);
        }
        if let Some(failure) = owner.lifecycle.as_ref().and_then(|l| l.failure()) {
            return Err(format!("startup failed: {failure:?}").into());
        }
        // Not shown: send `owner.transport.take_keepalive(now())` once connected,
        // drain `take_state_publications`, honour cancellation.
    }
}
```

> **What to verify before relying on this.**
>
> - The stage order, the batches and the timeouts are the library's policy. They
>   are covered by the crate's own tests with synthetic replies. Whether a given
>   firmware answers each stage within the budgets is not established here.
> - After the first synchronization result the session keeps running: further
>   state, keepalives and rebuilds are still your loop's job. The sketch stops at
>   the first result.
> - The suspension behaviours above were observed on hardware only for the BACKUP
>   reason. Treat the other reasons as handled defensively, not as understood.
