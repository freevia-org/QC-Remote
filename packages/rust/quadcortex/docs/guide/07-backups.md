# Backups

How to export a device backup with the library, and exactly what the caller must
do for it to work. The library plans, validates and assembles; **you** write the
bytes, run the clock, send the keepalive and keep the file.

Prerequisites: [Concepts](crate::guide::concepts) and
[Sessions](crate::guide::sessions).

## What exists, and what does not

- **Export is implemented.** [`commands::create_local_backup`](crate::commands::create_local_backup)
  builds the one request, and [`BackupRuntime`](crate::backup::BackupRuntime)
  turns the unit's reply stream into one JSON document.
- **Restore is not.** Neither the command builders nor the backup runtime contain
  a request that sends a backup back to a unit. The reply frames can carry
  `can_apply_backup` and `applied_backup` flags, and
  [`BackupFrameShape`](crate::protocol::responses::BackupFrameShape) reports them,
  but nothing in this crate acts on them. The doc on that type notes that the
  schema declares upload and download actions which this crate never sends.
- **Cloud backups are out of scope.** The cloud-backup message family rides on an
  encrypted trailer this build does not implement, and is rejected by name.

## How the stream behaves, per the library's docs

- You send **one** request. It carries no request id.
- The reply is an **uncorrelated stream**. The assembler's own comment says current
  firmware does not echo a request id on these replies, which is why a new client
  can inherit the tail of a transfer an earlier session started.
- Each chunk is a LocalBackup (type 40) `UPDATE` carrying a piece of JSON text and
  an explicit last-chunk flag. A frame without the flag, or without data, is
  ignored.
- Chunks that arrive **before** the start of a JSON document (the text must begin
  with `{`) are counted and discarded, never spliced in. That is how a stale tail
  is told apart from the transfer you asked for.
- The document is complete on the chunk whose last-chunk flag is set. It must
  parse as JSON whose `type` is `"backup"` and `creator` is `"quad"`, or the
  transfer fails.
- The document is capped at [`BACKUP_MAXIMUM_DOCUMENT_BYTES`](crate::protocol::profile::BACKUP_MAXIMUM_DOCUMENT_BYTES)
  (32 MiB).

## The runtime: `BackupRuntime`

A transfer is expressed entirely in your monotonic milliseconds.
[`BackupRuntime::start`](crate::backup::BackupRuntime::start) takes the time and an
overall budget. The profile's first-chunk window
(`BACKUP_FIRST_CHUNK_TIMEOUT_MS`) and stall window
(`BACKUP_STREAM_STALL_TIMEOUT_MS`) are clamped so they never extend past that
budget. The profile also defines `BACKUP_TOTAL_TIMEOUT_MS`, which a host can pass
as the overall budget.

| Call | Use it to |
| --- | --- |
| [`absorb(now_ms, payload)`](crate::backup::BackupRuntime::absorb) | Feed one LocalBackup payload. Returns a [`BackupProgress`](crate::backup::BackupProgress); its `document` is `Some` exactly once, on the finishing chunk. |
| [`advance(now_ms)`](crate::backup::BackupRuntime::advance) | Poll the clock. Returns a [`BackupAction`](crate::backup::BackupAction): `Wait`, `Keepalive`, or `Failed(reason)`. |
| [`cancel()`](crate::backup::BackupRuntime::cancel) | Stop locally. Terminal. |
| [`terminal_state()`](crate::backup::BackupRuntime::terminal_state) | `Complete`, `Cancelled` or `Failed`, once the transfer has ended. |

Three rules are written into the runtime, and you should plan around them:

- **A failed or cancelled transfer is never replayed.** "Cancellation is local and
  terminal. It never emits a second uncorrelated LocalBackup request and never
  permits a partial document to be resumed." A stall discards the partial
  document and says "the export request was not replayed".
- **A malformed or oversized chunk is terminal.** The transfer ends in `Failed`.
- **Completion is returned exactly once.** Persisting the document is yours.

### What the keepalive requirement means for you

`BackupAction::Keepalive` means: send the dedicated keepalive
([`commands::keepalive`](crate::commands::keepalive)) now. The doc on that variant
adds the instruction that matters: "Busy inbound traffic must not defer it."

In practice:

1. **Poll `advance` on your own timer**, independent of reads. The keepalive
   deadline is computed from the clock alone. Receiving a chunk does not move it,
   so a loop that only checks `advance` when the link goes idle will not send one
   while chunks are arriving.
2. **Send exactly one keepalive per `Keepalive` action.** The runtime reserves the
   next deadline when it returns the action.
3. **Do not substitute the connection's own keepalive scheduler.**
   [`TransportRuntime`](crate::transport::TransportRuntime) has a separate
   keepalive that a host can defer (`defer_keepalive`). The backup's is the one
   inbound traffic cannot defer. How the two interact on a real unit is **not
   established here**; the library only says that this one must not be postponed
   by traffic.

Why the unit needs it during a backup is also not established here. The library
states the requirement; it does not explain the device's side of it.

```rust
use prost::Message;
use quadcortex::backup::{BackupAction, BackupRuntime, BackupTerminalState};
use quadcortex::protocol::profile;
use quadcortex::protocol::proto::cortex_protobuf_v2 as pa;

/// A synthetic LocalBackup chunk: UPDATE, a piece of text, and the last flag.
fn chunk(text: &str, last: bool) -> Vec<u8> {
    pa::LocalBackupMessage {
        action: pa::message_action::Enum::Update as i32,
        backup_json: Some(pa::local_backup_message::BackupJson::BackupJson(text.into())),
        is_last_chunk: Some(pa::local_backup_message::IsLastChunk::IsLastChunk(last)),
        ..Default::default()
    }
    .encode_to_vec()
}

let mut backup = BackupRuntime::start(0, profile::BACKUP_TOTAL_TIMEOUT_MS);

// The keepalive comes from the clock alone, and traffic does not defer it.
assert_eq!(backup.advance(profile::KEEPALIVE_INTERVAL_MS - 1), BackupAction::Wait);
assert_eq!(backup.advance(profile::KEEPALIVE_INTERVAL_MS), BackupAction::Keepalive);

// A stale fragment from an earlier transfer arrives first. It is counted, not used.
let progress = backup.absorb(1_100, &chunk("...tail of an older transfer", false))?;
assert!(!progress.started);
assert_eq!(progress.ignored_prefix_chunks, 1);

// The document starts here.
let hash = "0".repeat(64);
let progress = backup.absorb(1_200, &chunk(r#"{"type":"backup","creator":"quad","#, false))?;
assert!(progress.started && progress.document.is_none());
// The keepalive that fell due at 2_000 is still due after a chunk arrives at
// 2_500: traffic does not defer it.
backup.absorb(2_500, &chunk(r#""payload":"AAAA","#, false))?;
assert_eq!(backup.advance(2_500), BackupAction::Keepalive);
assert_eq!(backup.advance(2_600), BackupAction::Wait);

let finishing = format!(r#""payload_hash":"{hash}"}}"#);
let progress = backup.absorb(2_700, &chunk(&finishing, true))?;
let document = progress.document.expect("the last chunk completes the document");
assert!(document.starts_with(r#"{"type":"backup""#));
assert_eq!(backup.terminal_state(), Some(BackupTerminalState::Complete));
// A finished transfer accepts nothing further.
assert!(backup.absorb(2_800, &chunk("{", false)).is_err());
# Ok::<(), String>(())
```

The stall and first-chunk windows end a transfer with a described failure:

```rust
use quadcortex::backup::{BackupAction, BackupRuntime, BackupTerminalState};
use quadcortex::protocol::profile;

// Nothing ever starts a document: the first-chunk window expires.
let mut backup = BackupRuntime::start(0, profile::BACKUP_TOTAL_TIMEOUT_MS);
match backup.advance(profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS) {
    BackupAction::Failed(reason) => assert!(reason.contains("no JSON document start arrived")),
    other => panic!("expected a failure, got {other:?}"),
}
assert_eq!(backup.terminal_state(), Some(BackupTerminalState::Failed));

// Cancellation is local and terminal.
let mut cancelled = BackupRuntime::start(0, profile::BACKUP_TOTAL_TIMEOUT_MS);
cancelled.cancel();
assert_eq!(cancelled.terminal_state(), Some(BackupTerminalState::Cancelled));
assert_eq!(cancelled.advance(10_000), BackupAction::Wait, "a terminal runtime does nothing");
```

## Validate and name the result

When the document completes, hand it to
[`finalize_device_backup`](crate::device::request::finalize_device_backup). It
checks the name with [`validate_backup_name`](crate::device::request::validate_backup_name)
(1 to 64 visible characters; it refuses an out-of-domain name rather than
truncating it), checks that the document is a JSON object with `type` `"backup"`,
`creator` `"quad"`, a non-empty Base64 `payload` and a 64-hex-digit
`payload_hash`, and returns it with its `name` set. The hash is checked for shape
only. **It is not recomputed**; integrity beyond that is not checked here.

Validate the name **before** you start the transfer. The doc on
`BACKUP_NAME_MAX_CHARACTERS` records why: a host that truncated at a different
length let a name pass its own check, run the whole transfer, and only then fail.

```rust
use quadcortex::device::request::{finalize_device_backup, validate_backup_name};

assert!(validate_backup_name("Gig backup").is_ok());
assert!(validate_backup_name("   ").is_err());
assert!(validate_backup_name(&"x".repeat(65)).is_err());

let raw = format!(
    r#"{{"type":"backup","creator":"quad","payload":"AAAA","payload_hash":"{}"}}"#,
    "ab".repeat(32)
);
let document = finalize_device_backup(&raw, "Gig backup")?;
assert_eq!(document["name"], "Gig backup");

// A document with the wrong creator is refused.
assert!(finalize_device_backup(r#"{"type":"backup","creator":"x"}"#, "n").is_err());
# Ok::<(), String>(())
```

## At the owner: bound to a session and to the send

[`SessionRuntime`](crate::device::owner::SessionRuntime) wraps one
`BackupRuntime` so that a backup is tied to the session and to the moment the
request actually went out:

- [`begin_backup(&mut operation, now)`](crate::device::owner::SessionRuntime::begin_backup)
  marks an [`OperationTicket`](crate::device::owner::OperationTicket) as the
  backup and starts the runtime with that ticket's remaining budget. A second
  backup while one is running is refused, and so is replaying an older ticket.
- [`consume_backup`](crate::device::owner::SessionRuntime::consume_backup) feeds a
  chunk only if its observation ticket is current **and** the request has been
  dispatched **and** the chunk's first report was read at or after the time the
  request's first report started. Anything else returns `Ok(None)` and is
  recorded as an owner rejection with a reason such as `request-not-dispatched`
  or `ingress-before-dispatch`. This is how a chunk from before the request, or
  from an older session, is kept out of the document.
- [`advance_backup`](crate::device::owner::SessionRuntime::advance_backup) is the
  owner's `advance`.
- A device suspension does not refuse the backup this host started; see
  [Sessions](crate::guide::sessions).
- [`claim_backup_recovery`](crate::device::owner::SessionRuntime::claim_backup_recovery)
  claims "an intentional host recovery exactly once, after a valid document", at
  the host's serialized transport teardown rather than when file persistence is
  merely scheduled. It starts no I/O. Why a host recovers the session after a
  backup is not stated in the library beyond that doc, and is not established
  here.

```rust
use prost::Message;
use quadcortex::backup::BackupAction;
use quadcortex::commands::create_local_backup;
use quadcortex::device::delivery::ReportOutcome;
use quadcortex::device::owner::SessionRuntime;
use quadcortex::device::transport::IngressTime;
use quadcortex::protocol::profile::MESSAGE_TYPE_LOCAL_BACKUP;
use quadcortex::protocol::proto::cortex_protobuf_v2 as pa;
use quadcortex::transport::{ReportLayout, TransportRuntime};

fn chunk(text: &str, last: bool) -> Vec<u8> {
    pa::LocalBackupMessage {
        action: pa::message_action::Enum::Update as i32,
        backup_json: Some(pa::local_backup_message::BackupJson::BackupJson(text.into())),
        is_last_chunk: Some(pa::local_backup_message::IsLastChunk::IsLastChunk(last)),
        ..Default::default()
    }
    .encode_to_vec()
}

// A synthetic session; see "Sessions" for how a real one starts.
let mut owner = SessionRuntime::new(0);
let connection = owner.opened(0)?;
owner.next_handshake_write(0, "session-1", None).expect("attempt 1 is due");

// Admit the backup as one operation with a generous budget.
let mut operation = owner.plan_operation(0, 180_000)?;
owner.begin_backup(&mut operation, 0)?;

// Send the single request through the delivery journal.
let request = create_local_backup();
assert_eq!(request.message_type, MESSAGE_TYPE_LOCAL_BACKUP);
let reports = TransportRuntime::encode_reports(&request, ReportLayout::ReportIdPrefixed)?;
let dispatch = owner.prepare_dispatch(Some(&operation), None, 100)?;
let delivery = owner.prepare_delivery(&dispatch, request.message_type, reports.len(), 100)?;
owner.delivery_admitted(&delivery)?;
for index in 0..reports.len() {
    owner.begin_native_report(&delivery, index, 100)?;
    owner.finish_native_report(&delivery, index, ReportOutcome::Delivered, 101)?;
}

let read_at = |owner: &mut SessionRuntime, at_ms: u64| {
    owner.capture_observation_with_ingress(
        connection,
        Some(IngressTime { first_report_at_ms: at_ms, last_report_at_ms: at_ms }),
    )
};

// A chunk read before the request went out cannot belong to it: ignored.
let early = read_at(&mut owner, 50)?;
assert!(owner.consume_backup(&operation, &early, &chunk(r#"{"type":"#, false), 150)?.is_none());

// Chunks read after the request are absorbed.
let hash = "0".repeat(64);
let first = read_at(&mut owner, 200)?;
let progress = owner
    .consume_backup(&operation, &first, &chunk(r#"{"type":"backup","creator":"quad","#, false), 201)?
    .expect("absorbed");
assert!(progress.started);

let last = read_at(&mut owner, 300)?;
let finishing = format!(r#""payload":"AAAA","payload_hash":"{hash}"}}"#);
let progress = owner
    .consume_backup(&operation, &last, &chunk(&finishing, true), 301)?
    .expect("absorbed");
assert!(progress.document.is_some());

// The owner's clock-driven action, then the one-time recovery claim.
assert_eq!(owner.advance_backup(&operation, 302)?.0, BackupAction::Wait);
owner.claim_backup_recovery(&operation)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

> **What to verify before relying on this.**
>
> - Every chunk in these snippets is synthetic. They show how the runtime treats
>   the shapes the code describes; they are not captures of a unit's backup
>   stream, and the guide's authors did not export a backup from a device while
>   writing it.
> - The document the library validates is a JSON wrapper around a base64
>   `payload`. What is inside the payload is **not interpreted** by this crate and
>   is not established here.
> - The timeouts in the profile (180 s total, 60 s to the first chunk, 15 s stall)
>   are host policy, not device guarantees. Pass your own total to `start` if
>   your link is slower.
