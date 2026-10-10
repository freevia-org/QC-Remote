# Quick start, offline

Five short, runnable steps that need no device: build a command, frame it,
reassemble reports, decode a recorded trace, and decode state. Every snippet on
this page runs under `cargo test --doc`.

Wire-level APIs are reached through [`quadcortex::protocol`](crate::protocol).
The same modules also exist at the crate root for compatibility; new code should
use the `protocol` paths.

## 1. Build a command and look at its bytes

[`commands`](crate::commands) turns an intent into an
[`OutboundMessage`](crate::commands::OutboundMessage): a message type and a
protobuf payload. Nothing is sent.

```rust
use quadcortex::commands::select_scene;
use quadcortex::protocol::framing::{
    self, FLAG_FIRST, FLAG_LAST, OUT_REPORT_ID, REPORT_SIZE, TRAILER_SIZE,
};

let command = select_scene(3);
assert_eq!(command.message_type, 13); // "Scene"
assert_eq!(command.payload, [0x08, 0x01, 0x18, 0x03]); // action UPDATE, scene 3

// One logical message becomes one or more fixed-size reports.
let reports = framing::encode(command.message_type, &command.payload);
assert_eq!(reports.len(), 1);

let report = &reports[0];
assert_eq!(report.len(), REPORT_SIZE); // 129 bytes
assert_eq!(report[0], OUT_REPORT_ID); // host -> device
assert_eq!(report[1] as usize, command.payload.len() + TRAILER_SIZE);
assert_eq!(report[2], FLAG_FIRST | FLAG_LAST); // the only report is both
assert_eq!(&report[3..7], &command.payload[..]);
assert_eq!(&report[7..11], &13_u32.to_le_bytes()); // trailer: message type
assert_eq!(&report[11..15], &[0, 0, 0, 0]); // encrypted, compressed, device bytes
```

`select_scene` does not validate its argument. Validation is the job of the
typed layer described in [section 5](crate::guide::send_commands).

## 2. Reassemble reports into a message

A host reads reports one at a time. [`FrameAssembler`](crate::protocol::framing::FrameAssembler)
returns `Some((message_type, payload))` when the last report of a message
arrives, and `None` until then. Reports from the device carry report ID `0x01`,
so the snippet rewrites the first byte of reports it encoded for the other
direction.

```rust
use quadcortex::protocol::framing::{encode, FrameAssembler, FrameError, IN_REPORT_ID};

// 300 payload bytes plus the 8-byte trailer need three reports of at most 126.
let payload = vec![0xAB_u8; 300];
let reports = encode(7, &payload);
assert_eq!(reports.len(), 3);

let mut assembler = FrameAssembler::new();
let mut finished = None;
for (index, report) in reports.iter().enumerate() {
    let mut inbound = report.to_vec();
    inbound[0] = IN_REPORT_ID;
    let result = assembler.push(inbound)?;
    if index + 1 < reports.len() {
        assert!(result.is_none(), "incomplete until the LAST report");
    } else {
        finished = result;
    }
}
assert_eq!(finished, Some((7, payload)));

// A malformed report is an error, and it resets the assembler.
assert_eq!(assembler.push(vec![0x01]), Err(FrameError::ShortReport));
# Ok::<(), FrameError>(())
```

Behaviour worth knowing, all from the doc comments on `FrameAssembler`:

- A continuation report that arrives before any report with the FIRST flag is
  ignored (`Ok(None)`). A host can attach to the stream between two reports of a
  message, so this is normal, not an error.
- A report with the FIRST flag discards any partial message and starts again.
- Any error resets the assembler.
- `push` returns a `u16` message type. To keep the whole envelope, including the
  compressed flag and the device bytes, use
  [`push_frame`](crate::protocol::framing::FrameAssembler::push_frame) instead.

When you hold a complete batch of reports,
[`decode_reports`](crate::protocol::framing::decode_reports) does the same in one
call and says exactly what is wrong with a bad batch:

```rust
use quadcortex::protocol::framing::{decode_reports, encode, FrameError, FLAG_LAST};

let mut reports: Vec<Vec<u8>> = encode(7, &[1, 2, 3]).into_iter().map(Vec::from).collect();
assert_eq!(decode_reports(&reports)?.payload, [1, 2, 3]);

reports[0][2] &= !FLAG_LAST; // strip the LAST flag from the only report
assert_eq!(decode_reports(&reports), Err(FrameError::MissingLast));
# Ok::<(), FrameError>(())
```

## 3. Name and decode a message

[`message_registry`](crate::protocol::message_registry) knows every message type
the protocol declares. `decode_any` returns `None` for a type that is not
declared, which is how an unknown push is told apart from a corrupt one (that
is `Some(Err(_))`).

```rust
use quadcortex::commands::select_scene;
use quadcortex::protocol::message_registry::{decode_any, message_name, DecodedMessage};
use quadcortex::protocol::proto::cortex_protobuf_v2::scene_message::SelectedScene;

let command = select_scene(5);
assert_eq!(message_name(command.message_type), Some("Scene"));

let Some(Ok(DecodedMessage::Scene(scene))) = decode_any(command.message_type, &command.payload)
else {
    panic!("a Scene payload decodes as a Scene message");
};
assert_eq!(scene.selected_scene, Some(SelectedScene::SelectedScene(5)));

assert!(decode_any(0xFFFF, &[]).is_none()); // not a declared type
```

Request ids are read with [`wire::request_id`](crate::protocol::wire::request_id),
which knows which message types keep the id in protobuf field 1 rather than
field 2:

```rust
use quadcortex::protocol::wire::{request_id, REQUEST_ID_FIELD_ONE_TYPES};

// A Scene reply in the usual shape: request id in field 2.
assert_eq!(request_id(13, &[0x08, 0x01, 0x10, 0x2a]), Some(42));
// A Connection message keeps its request id in field 1 and a flag in field 2;
// reading field 2 would return the flag. 49 is in the field-one table.
assert!(REQUEST_ID_FIELD_ONE_TYPES.contains(&49));
assert_eq!(request_id(49, &[0x08, 0x2a, 0x10, 0x01]), Some(42));
// No id present: None. Callers then match on message type alone.
assert_eq!(request_id(13, &[0x08, 0x01]), None);
```

## 4. Decode a recorded trace

The crate ships a **synthetic, sanitized** trace for its own tests:
`tests/fixtures/sanitized-session-v1.json`. It holds no device identity and no
preset content; the test that guards it fails if fields such as a serial number
or a preset name appear. Each entry records a direction, a message type, the raw
reports as hex, and the expected payload.

Replaying a trace is the same loop as a live read: feed each report to the
assembler and look at what comes out.

```rust
use quadcortex::protocol::framing::FrameAssembler;
use quadcortex::protocol::message_registry::{decode_any, message_name};

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("valid hex"))
        .collect()
}

// The fixture is read from inside the crate. For your own capture, load the
// file you recorded; the shape of the loop does not change.
let fixture = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/sanitized-session-v1.json"
));
let trace: serde_json::Value = serde_json::from_str(fixture)?;
assert_eq!(trace["privacy"]["synthetic"], true);

let mut assembler = FrameAssembler::new();
let mut seen = Vec::new();
for recorded in trace["frames"].as_array().expect("frames") {
    for report in recorded["reportsHex"].as_array().expect("reports") {
        let hex = report.as_str().expect("hex string");
        if let Some((message_type, payload)) = assembler.push(from_hex(hex))? {
            let decodes = matches!(decode_any(message_type, &payload), Some(Ok(_)));
            println!(
                "{:>8} type {message_type:>2} {:<8} {:>3} byte(s), decodes: {decodes}",
                recorded["direction"].as_str().unwrap_or("?"),
                message_name(message_type).unwrap_or("?"),
                payload.len(),
            );
            seen.push((message_type, decodes));
        }
    }
}

// Version READ out, Scene update in, and a multi-report frame of filler bytes.
// The last one reassembles correctly but is not a real ModelRepo body, so it
// does not decode as its declared type.
assert_eq!(seen, [(10, true), (13, true), (51, false)]);
# Ok::<(), Box<dyn std::error::Error>>(())
```

The runnable form of this is `examples/replay_trace.rs`.

> **What a trace does and does not tell you.** The fixture exercises the codec
> in both directions. It says nothing about USB or HID transfer behaviour, and
> it is synthetic: it is not a capture of a unit.

## 5. Decode state

[`StateDecoder`](crate::state::StateDecoder) is the stateful half of the library.
It keeps what it has learned (active scene, current preset, model catalog) and
turns each message into zero or more
[`StateUpdate`](crate::state::StateUpdate) values, which serialize to a small
camelCase JSON contract with a `kind` discriminant.

The command builders produce the same protobuf messages the decoder reads, so
the quickest offline test feeds one into the other:

```rust
use quadcortex::commands::{select_scene, set_tempo};
use quadcortex::state::StateDecoder;

let mut decoder = StateDecoder::new();

let scene = select_scene(3);
let updates = decoder.decode(scene.message_type, &scene.payload)?;
assert_eq!(updates.len(), 1);
let json = serde_json::to_value(&updates[0])?;
assert_eq!(json["kind"], "scene");
assert_eq!(json["activeScene"], 3);

// A tempo write is a sparse Grid update; the decoder reports it as "tempo".
let tempo = set_tempo(120);
let updates = decoder.decode(tempo.message_type, &tempo.payload)?;
assert_eq!(updates[0].kind, "tempo");
assert_eq!(updates[0].tempo, Some(120));
# Ok::<(), Box<dyn std::error::Error>>(())
```

> **What this proves.** It proves the decoder's logic on the library's own
> messages. It does not prove that a unit pushes a scene change in exactly the
> shape `select_scene` builds. For messages the decoder has no handler for, it
> returns an empty list and counts the type in its inbound telemetry (see
> [Troubleshooting](crate::guide::troubleshooting)), so silence is not a
> statement that nothing arrived.

`examples/state_decoder.rs` is the runnable form of this step. The next section
goes further: a whole current preset, a model catalog, and a snapshot to watch.
