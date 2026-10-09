What the library's typed errors mean, what to do about each, and how to find out
what it quietly did *not* act on. Every row below is a behaviour of the types it
names; nothing here is a statement about firmware.

## Read the type before the message

Errors come in two kinds, and the difference matters for how you handle them.

- **Typed errors** are enums you can match: [`FrameError`](crate::protocol::framing::FrameError),
  [`CommandEncodeError`](crate::commands::CommandEncodeError),
  [`StateDecodeError`](crate::state::StateDecodeError),
  [`ResponseDecodeError`](crate::protocol::responses::ResponseDecodeError),
  [`SessionFailure`](crate::session::SessionFailure),
  [`DeviceStartupError`](crate::device::initialization::DeviceStartupError),
  [`ReadObservation`](crate::device::read::ReadObservation) and
  [`BackupAction`](crate::backup::BackupAction).
- **Message strings.** The planners and the session owner return `Result<_, String>`
  with a human-readable reason. Treat these as text for people. The one marker the
  library documents as stable for classification is
  [`DEVICE_SUSPENDED_MARKER`](crate::device::owner::DEVICE_SUSPENDED_MARKER), the
  prefix of every suspension refusal. Do not parse the rest.

## Framing and decoding

| You see | It means | What to do |
| --- | --- | --- |
| `FrameError::ShortReport`, `InvalidLength` | A report is shorter than its three-byte prefix, longer than 129 bytes, or declares more data than it holds. The transport also returns `InvalidLength` for a 129-byte inbound report whose first byte is not the inbound report ID. | Check that your reader returns exactly one whole report per call, with the layout you think it has. |
| `MissingFirst`, `MissingLast`, `InvalidSequence` | A batch handed to `decode_reports` is not exactly one logical frame. | Use `FrameAssembler` for a live stream, which resynchronizes on FIRST. |
| `MissingTrailer` | The assembled message is under 8 bytes, so it has no envelope trailer. | A message under 8 bytes cannot be a real envelope. Check your reader. |
| `TooLarge` | The message spans more reports or bytes than the 1 MiB frame limit. | Treat the stream as corrupt; reset the assembler (it already did) and re-synchronize. |
| `MessageTypeOutOfRange(n)` | The trailer's 32-bit type does not fit `u16`; `n` is the raw value. | Use `push_frame` or `decode_reports` to keep the whole envelope. It is also tallied (below). |
| `UnsupportedEncryptedMessageType` | The frame's encrypted flag is set, or you tried to send a type from the cloud-account, backup-forwarding, log-forwarding, cloud-backup or license families. This build cannot encrypt or decrypt. | Do not use those families. Nothing in this crate can handle them, and the library will not send them in the clear. |
| `FrameError::Compression(_)` | The frame's compressed flag was set but the body would not inflate, or it exceeded that message type's inflation ceiling. | Count it, drop it. The limit is per type; see [`max_inflated_bytes_for`](crate::protocol::compression::max_inflated_bytes_for). |
| `decode_any` returns `None` | The type is not declared by the protocol. | A newer firmware may have added one. See the tally below. |
| `decode_any` returns `Some(Err(_))` | A declared type whose body did not decode. | Keep the raw bytes and report a decoding bug. |
| `StateDecodeError::Protobuf` | The body did not parse as the message for that type. | As above. |
| `StateDecodeError::UnexpectedAction`, `InvalidNormalizedValue`, `InvalidGridCoordinate` | The message was well formed but carried an action or value outside its domain. | The decoder refuses to guess. Log the type and keep the snapshot as it was. |

## Commands and planners

| You see | It means | What to do |
| --- | --- | --- |
| `CommandEncodeError::UnsupportedValue { field, value }` | A value outside the supported range or set. | Fix the argument. The `field` names what was rejected. |
| `CommandEncodeError::SignedFieldOverflow` | A value does not fit the signed 32-bit protobuf field. | Same. |
| "`expectedX` is required to guard `device.y`" | You omitted a required guard. | Supply it from freshly observed state. See [Sending commands safely](crate::guide::send_commands). |
| "... changed on the Quad Cortex ... Refresh and retry." | A guard is stale: the unit's state moved since you looked. | Re-read state and re-plan from the new snapshot. Do not copy the *new* value into the guard just to make it pass; that defeats the guard. |
| "... must be true" for a `confirm*` field | An operation that needs an explicit confirmation did not get one. | Obtain the confirmation from the person in your own application, then set the field. |
| "QC write target changed after planning; refresh before starting a new operation" | The owner re-checked a plan's captured state at the native attempt and it had moved. | Re-plan. A plan is not valid across a change to its target. |
| "QC command budget expired before native admission" | The plan's deadline passed before it was dispatched. | Re-plan with a fresh budget. |
| "QC session changed ..." or "... session changed during ..." | The session the plan was made in no longer exists. | Discard it. A ticket from one session is never valid in another. |
| "No active QC session for command admission" | The owner has no active lifecycle. | Connect first, and check `is_synchronized` before planning writes. |

## Sessions

| You see | It means | What to do |
| --- | --- | --- |
| `SessionFailure::Invalid(SessionMismatch)` | The reset reply echoed a different session id. | Use a fresh session id per attempt, and match *this* attempt's reply. |
| `Invalid(UnsupportedDevice)`, `Invalid(IncompatibleControllerVersion)` | The Version reply named another device type, or reported the host version as invalid. | The device or host version was rejected; see [`DeviceStartupError`](crate::device::initialization::DeviceStartupError). |
| `Protocol(RebuildLoop)` | The unit kept announcing a disconnect during startup, past `MAX_STARTUP_REBUILDS`. | Tear down and redo the whole handshake with a fresh open. |
| `Protocol(ModelRepositoryDataError)` | The model catalog was unparsable or empty. | Capture the raw ModelRepo frame yourself (the library does not keep it) and report it. |
| `StartupTimeout(phase)` | Startup did not reach `Connected` in the readiness budget. `phase` is where it was stuck. | Look at which stage's reply never arrived. |
| `Dispatch` | Your writer reported a failed submission or a terminal transport failure. | A failed submission is terminal and never replayed. |
| `QueueLimit` | More work queued than the driver allows. | Your writer is not keeping up; the driver fails rather than grow without bound. |
| Suspension refusal | See [Sessions](crate::guide::sessions). | Wait for the unit to resume. |

## Reads, writes and backups

| You see | It means | What to do |
| --- | --- | --- |
| `ReadObservation::Expired` | The deadline passed, even if a matching reply arrived. | Retry the read. Late means failed. |
| `ReadObservation::Rejected` | A reply matched type and id but could not be projected. | The read is still pending; a later valid reply can complete it. |
| `DeliveryStatus::Cancelled` | Stopped before any report started; nothing reached the unit. | Safe to re-plan. |
| `DeliveryStatus::Indeterminate` | Stopped after a report started. | **Do not assume** the unit received nothing or everything. Re-read state before deciding. |
| `GatewayVerificationAction::TimedOut` | No observation newer than the write satisfied the predicate in time. | Not proof of failure. Re-read state. |
| `BackupAction::Failed(reason)` | The overall deadline, the first-chunk window or the stall window expired. The reason includes a payload-free summary of what arrived. | The export is not replayed. Start a new backup if you still want one. |
| `ResponseDecodeError::OversizedBackup`, `InvalidBackupDocument` | The assembled document passed 32 MiB, or was not a complete JSON backup. | The transfer is terminal. |

## "Nothing happened": the inbound tally

A message can arrive and go nowhere. The library counts every way that can happen,
per message type, in [`InboundTelemetry`](crate::inbound_telemetry::InboundTelemetry).
Read it with [`SessionRuntime::inbound_telemetry`](crate::device::owner::SessionRuntime::inbound_telemetry),
which merges the transport's half (envelope outcomes) with the state decoder's half
(dispatch outcomes). A host compares
[`inbound_telemetry_revision`](crate::device::owner::SessionRuntime::inbound_telemetry_revision)
against the last value it published, so an idle link costs nothing.

| Counter in [`InboundTelemetrySnapshot`](crate::inbound_telemetry::InboundTelemetrySnapshot) | The frame |
| --- | --- |
| `undeclared_types` | had a type the protocol does not declare, keyed by the raw trailer value |
| `decode_failures` | was a declared type whose body did not decode |
| `encrypted_rejections` | had its encrypted flag set |
| `compressed`, `inflate_failures` | had its compressed flag set, and whether it then inflated |
| `no_state_handler_types` | decoded fine but the *state decoder* has no handler for it |
| `device_trailer_bytes` | tallies the two device bytes of every frame, per type |
| `unacted_frames` | counts frames nothing anywhere could use: undeclared, undecodable, encrypted, or uninflatable |
| `abandoned_partial_frames` | was lost part-way through assembly, cut off by a new FIRST report or a native ingress epoch change (never assembled, so not in `frames`) |
| `orphaned_continuation_reports` | was a continuation report that arrived with no message open, and was ignored |
| `framing_errors` | was a report the framing layer rejected (malformed, past the frame-size limit, or shorter than its trailer) |
| `last_unacted` | one line describing the most recent one |

Read `no_state_handler_types` carefully. The doc says it "is narrower than 'nothing
in the host did'": the settings, session, catalog and backup lanes read types the
state decoder ignores, so a type appearing there is not automatically lost.

```rust
use quadcortex::commands::keepalive;
use quadcortex::protocol::framing::{encode_frame, Frame, FrameError, IN_REPORT_ID};
use quadcortex::state::StateDecoder;
use quadcortex::transport::TransportRuntime;

/// One inbound report carrying an arbitrary envelope.
fn inbound(message_type: u32, payload: &[u8], encrypted: bool, compressed: bool) -> Vec<u8> {
    let mut report = encode_frame(&Frame {
        message_type,
        payload: payload.to_vec(),
        encrypted,
        compressed,
        device_bytes: [0, 0],
    })[0]
        .to_vec();
    report[0] = IN_REPORT_ID;
    report
}

let mut transport = TransportRuntime::new(0);

// A type the protocol does not declare is delivered, but counted.
assert!(transport.push_report(&inbound(73, &[8, 1], false, false))?.is_some());
// An encrypted frame is refused by name.
assert_eq!(
    transport.push_report(&inbound(58, &[8, 1], true, false)),
    Err(FrameError::UnsupportedEncryptedMessageType)
);
// A compressed body that is not gzip fails to inflate.
assert!(matches!(
    transport.push_report(&inbound(51, &[0x1f, 0x8b, 0x00], false, true)),
    Err(FrameError::Compression(_))
));

let tally = transport.inbound_telemetry().snapshot();
assert_eq!(tally.frames, 3);
assert_eq!(tally.undeclared_types[&73], 1);
assert_eq!(tally.encrypted_rejections[&58], 1);
assert_eq!(tally.inflate_failures[&51], 1);
assert_eq!(tally.unacted_frames, 3);
println!("last unacted: {:?}", tally.last_unacted);

// The decoder's half: a declared, decodable type it has no handler for.
let ping = keepalive();
let mut decoder = StateDecoder::new();
assert!(decoder.decode(ping.message_type, &ping.payload)?.is_empty());
assert_eq!(decoder.inbound_telemetry().snapshot().no_state_handler_types[&32], 1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## The usual mistakes

These are the ones the library's own doc comments warn about.

- **Reusing a session id.** The id must be fresh for every physical open and
  handshake attempt. An old one lets a stale reply look current.
- **Not draining state publications.** `take_state_publications` empties a
  bounded queue. If it overflows, the owner fails the lifecycle so that a lost
  update cannot pass for an intact stream.
- **Ingesting the model catalog inline.** `DeviceStateRuntime::ingest` refuses type
  51 on purpose. Parse it off your receive path and install it.
- **Taking delivery for acknowledgement.** `Completed` means the transport took the
  reports, nothing more.
- **Planning writes before the session is synchronized.** `plan_operation` only
  requires an active lifecycle; check `is_synchronized` first.
- **Two request-id counters.** Draw ids from `SessionDriver::reserve_request_id`.
- **Letting traffic postpone a backup keepalive.** Poll `BackupRuntime::advance` on
  its own timer.
- **Retrying a persistent write.** The library never replays one. If a result is
  uncertain, read the catalog before deciding.
- **Copying a fresh value into a stale guard.** A guard exists to catch a change;
  rewriting it to the new value turns the check off.
