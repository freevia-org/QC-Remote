# Tutorial: read the current preset and watch state updates

A step-by-step walk through the job most programs start with: get the unit's
current preset and then follow its state as it changes. Each step names the type
involved and says what it does. Steps 1 to 6 run offline against synthetic
messages built by the snippets. Step 7 shows the live loop and is
compile-only.

Prerequisite: [Concepts](crate::guide::concepts).

## Step 1. Know where the preset comes from

You do not normally *request* the current preset to learn what it is. Two things
populate it:

- **Startup asks for it.** The staged startup sequence includes a READ of the
  RecallPreset message (type 15) in its boot burst
  ([`commands::boot_initialization`](crate::commands::boot_initialization)). If
  the preset has not arrived within
  [`INITIAL_SYNC_TIMEOUT_MS`](crate::protocol::profile::INITIAL_SYNC_TIMEOUT_MS),
  the post-boot seed issues the bare
  [`read_current_preset`](crate::commands::read_current_preset) (`08 03`, no request id)
  ([`InitializationRuntime`](crate::device::initialization::InitializationRuntime)).
- **The unit pushes it.** When a different preset is recalled, the unit publishes
  new position and preset messages unasked.

So "read the current preset" really means: reduce every inbound message into a
state model, and ask for a refresh only when you need one. The preset **content**
is reduced by the state decoder from the type-15 message. The read,
[`device.currentPreset`](crate::device::request::plan_gateway_read), is the bare
boot READ and decodes only an acknowledgement
([`CurrentPresetAck`](crate::protocol::responses::CurrentPresetAck)); its reply is
matched by type and arrival, not by an echoed request id. See the doc on
[`decode_current_preset_ack`](crate::protocol::responses::decode_current_preset_ack).
Reads whose replies do echo an id (favorites, recents) are the ones shown below.

## Step 2. Plan the read

[`plan_gateway_read`](crate::device::request::plan_gateway_read) turns a read
method name into a [`GatewayReadPlan`](crate::device::request::GatewayReadPlan):
the operation to send, the message type of the reply, a timeout and a projection
that decodes the reply. The method names (`device.currentPreset`,
`device.generalSettings`, and so on) are the shared gateway vocabulary listed in
[`generated_gateway::METHODS`](crate::device::generated_gateway::METHODS).

You supply the **request id**. Reserve it from the session's own sequence
([`SessionDriver::reserve_request_id`](crate::session::SessionDriver::reserve_request_id))
so that you never run a second counter that could reuse an id still in flight.
The snippet uses a fixed id for readability.

```rust
use quadcortex::device::request::plan_gateway_read;
use quadcortex::transport::{ReportLayout, TransportRuntime};
use serde_json::json;

let request_id = 42;
let plan = plan_gateway_read("device.favorites", &json!({}), request_id)?;
assert_eq!(plan.response_type, 20); // the reply is a RecentsFavorites message
assert_eq!(plan.timeout_ms, 20_000);
assert_eq!(plan.projection.expected_request_id(), Some(42));

// The operation encodes to one or more messages, validated on the way out.
let messages = plan.operation.clone().try_encode()?;
assert_eq!(messages.len(), 1);

// Adapt the outer shape to what the host's USB API accepts: the full
// 129-byte report, or only the 128-byte body.
let full = TransportRuntime::encode_reports(&messages[0], ReportLayout::ReportIdPrefixed)?;
let body = TransportRuntime::encode_reports(&messages[0], ReportLayout::BodyOnly)?;
assert_eq!((full[0].len(), body[0].len()), (129, 128));
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Step 3. Write the reports, with pacing

The library does not write. It does say how fast: after every group of ten
reports, yield for a fixed delay
([`TransportRuntime::report_batch_delay_ms`](crate::transport::TransportRuntime::report_batch_delay_ms),
backed by `profile::HID_REPORT_BATCH_SIZE` and `HID_REPORT_BATCH_DELAY_MS`). The
sleep itself is yours. The snippet below defines the small seam a host would
implement for its USB handle, and runs a message through it against an
in-memory recorder. **No implementation of that seam for a real device exists in
this crate**; what a real one involves is in
[Transports](crate::guide::transports).

```rust
use quadcortex::commands::{select_scene, OutboundMessage};
use quadcortex::transport::{ReportLayout, TransportRuntime};

/// The host's side of the USB/HID boundary. The library never provides this.
trait HidLink {
    fn write_report(&mut self, report: &[u8]) -> std::io::Result<()>;
    fn sleep_ms(&mut self, milliseconds: u64);
}

fn send<L: HidLink>(link: &mut L, message: &OutboundMessage) -> Result<(), Box<dyn std::error::Error>> {
    let reports = TransportRuntime::encode_reports(message, ReportLayout::ReportIdPrefixed)?;
    for (index, report) in reports.iter().enumerate() {
        link.write_report(report)?;
        // `index + 1` is one-based: a delay is due after the 10th, 20th, ...
        if let Some(delay) = TransportRuntime::report_batch_delay_ms(index + 1) {
            link.sleep_ms(delay);
        }
    }
    Ok(())
}

/// An in-memory link that records what a host would have written.
#[derive(Default)]
struct Recorder {
    reports: usize,
    slept_ms: u64,
}
impl HidLink for Recorder {
    fn write_report(&mut self, _report: &[u8]) -> std::io::Result<()> {
        self.reports += 1;
        Ok(())
    }
    fn sleep_ms(&mut self, milliseconds: u64) {
        self.slept_ms += milliseconds;
    }
}

let mut link = Recorder::default();
send(&mut link, &select_scene(1))?;
assert_eq!((link.reports, link.slept_ms), (1, 0)); // one report: no pause

// A 1,520-byte payload plus the 8-byte trailer needs 13 reports of at most
// 126 bytes, so there is one pause, after the tenth.
let big = OutboundMessage { message_type: 4, payload: vec![0; 1_520] };
let mut link = Recorder::default();
send(&mut link, &big)?;
assert_eq!(link.reports, 13);
assert_eq!(link.slept_ms, quadcortex::protocol::profile::HID_REPORT_BATCH_DELAY_MS);
# Ok::<(), Box<dyn std::error::Error>>(())
```

> **Sending is not the same as the unit having acted.** The library's own
> delivery journal says so in its first line: completion "describes the host
> transport policy, not device acknowledgement"
> ([`device::delivery`](crate::device::delivery)). [Section 5](crate::guide::send_commands)
> returns to this.

## Step 4. Receive reports and match the reply

Reports come back one at a time. [`TransportRuntime::push_report`](crate::transport::TransportRuntime::push_report)
accepts either native layout (129 bytes with the report ID, or the 128-byte body),
assembles the frame, and returns an
[`InboundFrame`](crate::transport::InboundFrame): the message type, the payload
**already inflated** if it arrived compressed, how many reports it took, and the
envelope's device bytes. Use
[`push_report_at`](crate::transport::TransportRuntime::push_report_at) to stamp
the read time in your clock; that timestamp lets later code prove a reply
arrived *after* a request.

[`GatewayReadRuntime`](crate::device::read::GatewayReadRuntime) holds the
expectation for one planned read: the reply type, the request id the projection
requires, a deadline, and the decoder.
[`observe`](crate::device::read::GatewayReadRuntime::observe) offers it each
frame and answers with a [`ReadObservation`](crate::device::read::ReadObservation).

The snippet builds a synthetic reply, passes it through both, and shows that an
unrelated frame and a wrong request id do not complete the read.

```rust
use prost::Message;
use quadcortex::device::read::{GatewayReadRuntime, ReadObservation};
use quadcortex::device::request::plan_gateway_read;
use quadcortex::protocol::framing::{encode, IN_REPORT_ID};
use quadcortex::protocol::proto::cortex_protobuf_v2 as pa;
use quadcortex::transport::TransportRuntime;
use serde_json::json;

/// A synthetic RecentsFavorites UPDATE that echoes `request_id` and lists one
/// favorite preset named `name`. Built by this snippet; it is not captured traffic.
fn favorites_reply(name: &str, request_id: u64) -> Vec<u8> {
    pa::RecentsFavoritesMessage {
        action: pa::message_action::Enum::Update as i32,
        is_favorites: true,
        r#type: pa::recents_favorites_message::RecentFavoriteType::Preset as i32,
        request_id: Some(pa::recents_favorites_message::RequestId::RequestId(request_id)),
        items: vec![pa::RecentsFavoritesItem {
            name: name.into(),
            folder_key: "My Presets".into(),
            ..Default::default()
        }],
        ..Default::default()
    }
    .encode_to_vec()
}

/// Reports as the device would deliver them: same layout, inbound report ID.
fn inbound_reports(message_type: u16, payload: &[u8]) -> Vec<Vec<u8>> {
    encode(message_type, payload)
        .into_iter()
        .map(|report| {
            let mut report = report.to_vec();
            report[0] = IN_REPORT_ID;
            report
        })
        .collect()
}

let plan = plan_gateway_read("device.favorites", &json!({}), 42)?;
let started_at = 1_000; // host monotonic milliseconds
let read = GatewayReadRuntime::new(plan.response_type, plan.projection, started_at, plan.timeout_ms);

let mut transport = TransportRuntime::new(0);
let mut feed = |message_type: u16, payload: Vec<u8>, now_ms: u64| {
    let mut outcome = ReadObservation::Unrelated;
    for report in inbound_reports(message_type, &payload) {
        if let Some(frame) = transport.push_report_at(&report, now_ms).expect("valid reports") {
            outcome = read.observe(frame.message_type, &frame.payload, now_ms);
        }
    }
    outcome
};

// An unrelated message type does not complete the read.
assert_eq!(feed(13, vec![0x08, 0x01, 0x18, 0x02], 1_100), ReadObservation::Unrelated);
// Right type, wrong request id: still not the awaited reply.
assert_eq!(feed(20, favorites_reply("Demo", 41), 1_200), ReadObservation::Unrelated);
// The correlated reply completes it, projected to the public JSON shape.
match feed(20, favorites_reply("Demo", 42), 1_300) {
    ReadObservation::Matched { value } => assert_eq!(value["entries"][0]["name"], "Demo"),
    other => panic!("expected a match, got {other:?}"),
}
// The same reply after the deadline is a timeout, whatever it says.
assert_eq!(feed(20, favorites_reply("Demo", 42), 1_000 + 20_000), ReadObservation::Expired);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Three properties to rely on, all from the doc comments on these types:

- A reply is matched on **type and id together**; with an id requested, a reply
  without it, or with another, is `Unrelated`.
- A matching reply that does not decode is `Rejected`, and the read stays
  pending, so a later valid reply can still complete it within the original
  budget.
- Decoding does not extend the budget. `observe_with_clock` samples the clock
  again after decoding, so a slow decoder cannot validate an expired reply.

## Step 5. Reduce messages into state

[`DeviceStateRuntime`](crate::device::state_runtime::DeviceStateRuntime) is the
reducer. It wraps a [`StateDecoder`](crate::state::StateDecoder), keeps a
[`GatewaySnapshot`](crate::device::GatewaySnapshot) up to date from the
decoder's updates, and retains preset listings and settings publications.
[`ingest`](crate::device::state_runtime::DeviceStateRuntime::ingest) takes one
assembled message and returns a
[`StateObservation`](crate::device::state_runtime::StateObservation) with the
normalized updates it produced.

Block names and categories come from the unit's model catalog (message type 51).
Parsing it is expensive, so `ingest` **refuses** that type: parse the catalog
with [`parse_model_repo`](crate::state::parse_model_repo), preferably off your
receive thread, and install it with
[`install_model_catalog`](crate::device::state_runtime::DeviceStateRuntime::install_model_catalog).

```rust
use prost::Message;
use quadcortex::device::state_runtime::DeviceStateRuntime;
use quadcortex::protocol::proto::{
    binary_preset, chain, cortex_protobuf_v2 as pa, model, BinaryPreset, Chain, Model,
};
use quadcortex::state::parse_model_repo;

const MODEL_REPO: u16 = 51;
const RECALL_PRESET: u16 = 15;
const SETLIST_POSITION: u16 = 2;

// 1. A synthetic model catalog with one model, id 7.
let catalog_message = pa::ModelRepoMessage {
    action: pa::message_action::Enum::Update as i32,
    model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
        br#"<Models><Category name="Delay"><Model id="7" name="Echo" /></Category></Models>"#
            .to_vec(),
    )),
    ..Default::default()
}
.encode_to_vec();

let mut runtime = DeviceStateRuntime::new();
assert!(runtime.ingest(MODEL_REPO, &catalog_message).is_err(), "parse it off-lane");
runtime.install_model_catalog(parse_model_repo(&catalog_message)?);
assert_eq!(runtime.model_count(), 1);

// 2. A synthetic preset: one block, model 7, in row 0, column 2.
let preset_message = pa::RecallPresetMessage {
    action: pa::message_action::Enum::Update as i32,
    preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
        name: Some(binary_preset::Name::Name("Demo".into())),
        chains: vec![Chain {
            row: Some(chain::Row::Row(0)),
            models: vec![Model {
                hash: Some(model::Hash::Hash(7)),
                column: Some(model::Column::Column(2)),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    })),
    ..Default::default()
}
.encode_to_vec();

// 3. A synthetic setlist position: slot 8 of a setlist folder.
let position_message = pa::SetlistPositionMessage {
    action: pa::message_action::Enum::Update as i32,
    folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(
        "/media/p4/Presets/Demo Setlist".into(),
    )),
    position: Some(pa::setlist_position_message::Position::Position(8)),
    ..Default::default()
}
.encode_to_vec();

assert!(!runtime.snapshot().has_preset);
let observation = runtime.ingest(RECALL_PRESET, &preset_message)?;
runtime.ingest(SETLIST_POSITION, &position_message)?;

// The observation lists what the message changed, in the JSON-ready form.
assert_eq!(observation.states.len(), 1);
assert_eq!(observation.states[0].kind, "preset");

// The snapshot is the retained current state.
let snapshot = runtime.snapshot();
assert!(snapshot.has_preset);
assert_eq!(snapshot.preset_name, "Demo");
assert_eq!(snapshot.setlist_name, "Demo Setlist");
assert_eq!(snapshot.preset_position, 8);
assert_eq!(snapshot.preset_location, "2A"); // derived label: position 8 is bank 2, letter A
assert_eq!(snapshot.blocks.len(), 1);
assert_eq!(snapshot.blocks[0].name, "Echo"); // resolved from the catalog
assert_eq!((snapshot.blocks[0].row, snapshot.blocks[0].column), (0, 2));
# Ok::<(), Box<dyn std::error::Error>>(())
```

The fields of the snapshot, in the order a program usually needs them:

| Field | Meaning, per its doc comment |
| --- | --- |
| `has_preset` | A current preset has been observed in this session. |
| `preset_name`, `preset_location`, `preset_position`, `setlist_key`, `setlist_name` | The address of the active preset. `preset_position` is the unit's zero-based slot; `preset_location` is a label this library derives from it (`position / 8 + 1` and the letter `A` plus `position % 8`). The name is `Unsaved` when the preset has none. |
| `active_scene`, `scenes`, `scene_colors` | Active scene index `0..=7` and the scene labels and colours, for display. An unset label shows as `Scene A` to `Scene H`, which is the library's text, and a colour is `#rrggbb` with the stored alpha byte dropped. |
| `scene_labels_raw`, `scene_colors_argb` | The same labels and colours exactly as the device stored them, once a preset has reported them. |
| `mode`, `raw_mode` | The footswitch mode as `PRESET`, `SCENE`, `STOMP` or `HYBRID`, and the unmodified wire value. A value outside the nine the library names reads `UNKNOWN`; the wire schema declares no mode enum, so the names are the library's. |
| `blocks`, `routes` | The grid: each [`GridBlock`](crate::state::GridBlock) with its model, name, position and bypass state, and each row's input and output route. A route keeps its raw ids and the wire enum name of each (`input_wire`, `output_wire`); an id the row did not carry is `None`, not `0`. |
| `tempo`, `master_volume` | Preset tempo in BPM, derived from the normalized wire value as `40 + 200 * value` (believed, not verified, for the preset tempo), and master volume as a whole percentage `0..=100`, which is the wire float `0.0..=1.0` times 100, rounded. No physical level is established for the master volume. |
| `dirty` | The working preset has been edited since it was loaded or saved. |
| `unobserved` | The fields among `mode`, `tempo`, `tempo_led_enabled`, `master_volume`, `dirty` and `preset_location` that still hold the library's placeholder because no device message has set them yet. Each leaves the list when a message sets it. A placeholder is not something the device said. |
| `position_revision`, `address_revision`, `preset_revision`, `full_preset_revision` | Counters used to prove that a *new* observation arrived after a write; skipped when serialized. |

A snapshot is an observation, not a transaction. The owner's own doc says its
current preset "is a cache, not proof of a particular write or a correlated
readback".

## Step 6. Watch updates

Each later message returns its own updates. The kind decides which part of the
snapshot changes; the snapshot applies them for you inside `ingest`, and you get
the same list back to forward to your own UI or log.

```rust
use prost::Message;
use quadcortex::device::state_runtime::DeviceStateRuntime;
use quadcortex::protocol::proto::cortex_protobuf_v2 as pa;

const SCENE: u16 = 13;
const MASTER_VOLUME: u16 = 17;
const PRESET_DIRTY: u16 = 34;

let mut runtime = DeviceStateRuntime::new();

let scene = pa::SceneMessage {
    action: pa::message_action::Enum::Update as i32,
    selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(5)),
    ..Default::default()
}
.encode_to_vec();
let dirty = pa::PresetDirtyMessage {
    action: pa::message_action::Enum::Update as i32,
    is_dirty: true,
    ..Default::default()
}
.encode_to_vec();
let volume = pa::MasterVolumeMessage {
    action: pa::message_action::Enum::Update as i32,
    volume: Some(pa::master_volume_message::Volume::Volume(0.57)),
    ..Default::default()
}
.encode_to_vec();

for (message_type, payload) in [(SCENE, scene), (PRESET_DIRTY, dirty), (MASTER_VOLUME, volume)] {
    let observation = runtime.ingest(message_type, &payload)?;
    for update in &observation.states {
        // The camelCase JSON contract, one object per update.
        println!("{}", serde_json::to_string(update)?);
    }
}

let snapshot = runtime.snapshot();
assert_eq!(snapshot.active_scene, 5);
assert!(snapshot.dirty);
assert_eq!(snapshot.master_volume, 57); // 0.57 on the wire, 57 after the library's times-100 rounding
# Ok::<(), Box<dyn std::error::Error>>(())
```

`examples/state_watcher.rs` runs a longer version of steps 5 and 6 and prints each
update as JSON:

```text
cargo run -p quadcortex --example state_watcher
```

Two behaviours to expect, both visible in the decoder's doc comments:

- A message the decoder has no handler for produces an **empty** list, which
  looks the same as a handled message with nothing to report. The decoder counts
  such types in its inbound telemetry so the difference is recoverable; see
  [Troubleshooting](crate::guide::troubleshooting).
- A decode failure is an `Err` from `ingest`, not a fabricated successful state
  observation. An ordinary malformed message does not replace the last good
  state. However, invalidated retained folds explicitly withdraw dependent
  cached evidence: sparse Grid resource overflow retires the cached preset and
  requires a real full-preset refresh; model-preset fold invalidation retires
  that catalog's authority until the session resets.

## Step 7. Doing it live

Offline, steps 2 to 6 run in one process. Live, they run on two lanes: a reader
that turns reports into frames, and everything else. The library gives you the
session owner,
[`SessionRuntime`](crate::device::owner::SessionRuntime), to tie them together
under one lock. A frame read from the device takes this path:

1. [`capture_report_epoch`](crate::device::owner::SessionRuntime::capture_report_epoch)
   stamps the read before any queueing, so the owner can later discard reports
   from a replaced session.
2. [`push_native_report`](crate::device::owner::SessionRuntime::push_native_report)
   assembles the frame and returns it with an
   [`ObservationTicket`](crate::device::owner::ObservationTicket).
3. [`observe_lifecycle`](crate::device::owner::SessionRuntime::observe_lifecycle)
   lets the startup lifecycle see the frame. A device-announced rebuild changes
   the session epoch, so check
   [`accepts_observation`](crate::device::owner::SessionRuntime::accepts_observation)
   afterwards and treat a stale ticket as "the session was replaced".
4. [`observe_state`](crate::device::owner::SessionRuntime::observe_state) reduces
   the frame into the snapshot and stages a state publication.
5. Drain [`take_state_publications`](crate::device::owner::SessionRuntime::take_state_publications)
   regularly. The queue is bounded; if it overflows, the owner fails the
   lifecycle rather than let a lost update pass for an intact stream.
6. The model catalog (type 51) takes the
   [`queue_model_repo`](crate::device::owner::SessionRuntime::queue_model_repo),
   [`take_model_repo`](crate::device::owner::SessionRuntime::take_model_repo),
   [`complete_model_repo`](crate::device::owner::SessionRuntime::complete_model_repo)
   route instead, so parsing can happen off the lock.

The snippet below is that loop for a read that is already planned. It is
**compiles but was not run against a device**: it needs a connected unit, a
completed handshake ([Sessions](crate::guide::sessions)) and a real implementation
of the `HidLink` seam, none of which exist in this crate.

```rust,no_run
use quadcortex::commands::OutboundMessage;
use quadcortex::device::owner::SessionRuntime;
use quadcortex::device::read::{GatewayReadRuntime, ReadObservation};
use quadcortex::device::request::plan_gateway_read;
use quadcortex::protocol::profile::MESSAGE_TYPE_MODEL_REPO;
use quadcortex::state::parse_model_repo;
use quadcortex::transport::{ReportLayout, TransportRuntime};
use serde_json::json;

/// The host's side of the USB/HID boundary. You implement this.
trait HidLink {
    fn write_report(&mut self, report: &[u8]) -> std::io::Result<()>;
    fn read_report(&mut self, timeout_ms: u32) -> std::io::Result<Option<Vec<u8>>>;
    fn sleep_ms(&mut self, milliseconds: u64);
}

fn send<L: HidLink>(link: &mut L, message: &OutboundMessage) -> Result<(), Box<dyn std::error::Error>> {
    let reports = TransportRuntime::encode_reports(message, ReportLayout::ReportIdPrefixed)?;
    for (index, report) in reports.iter().enumerate() {
        link.write_report(report)?;
        if let Some(delay) = TransportRuntime::report_batch_delay_ms(index + 1) {
            link.sleep_ms(delay);
        }
    }
    Ok(())
}

/// Refresh the current preset and wait for the correlated acknowledgement.
/// `connection` is the number `SessionRuntime::opened` returned for this open.
fn refresh_current_preset<L: HidLink>(
    link: &mut L,
    owner: &mut SessionRuntime,
    connection: u64,
    request_id: u64,
    now_ms: impl Fn() -> u64,
) -> Result<(), Box<dyn std::error::Error>> {
    // Only refresh once startup has finished; see "Sessions" for what
    // `is_synchronized` means.
    if !owner.is_synchronized() {
        return Err("the session is not synchronized yet".into());
    }

    let plan = plan_gateway_read("device.currentPreset", &json!({}), request_id)?;
    let read = GatewayReadRuntime::new(plan.response_type, plan.projection.clone(), now_ms(), plan.timeout_ms);
    for message in plan.operation.clone().try_encode()? {
        send(link, &message)?;
    }

    let epoch = owner.capture_report_epoch(connection)?;
    loop {
        if read.remaining_ms(now_ms()) == 0 {
            return Err("timed out waiting for the current preset".into());
        }
        let Some(report) = link.read_report(100)? else { continue };
        let Some((frame, ticket)) = owner.push_native_report(connection, epoch, &report, now_ms())? else {
            continue; // frame not complete yet, or a stale reader
        };

        if frame.message_type == MESSAGE_TYPE_MODEL_REPO {
            // Parse off the lock in a real host; shown inline for brevity.
            if owner.queue_model_repo(ticket.clone(), frame.payload.clone()) {
                if let Some(work) = owner.take_model_repo() {
                    let catalog = parse_model_repo(&work.payload).map_err(|error| error.to_string());
                    owner.complete_model_repo(&work.ticket, catalog, now_ms())?;
                }
            }
            continue;
        }
        owner.observe_lifecycle(&ticket, frame.message_type, &frame.payload, now_ms());
        if !owner.accepts_observation(&ticket) {
            return Err("the session was replaced while waiting".into());
        }
        owner.observe_state(&ticket, frame.message_type, &frame.payload)?;
        for _publication in owner.take_state_publications()? {
            // Forward to your UI or log; draining keeps the bounded queue empty.
        }

        match read.observe(frame.message_type, &frame.payload, now_ms()) {
            ReadObservation::Matched { .. } => break,
            ReadObservation::Expired => return Err("the read expired".into()),
            ReadObservation::Unrelated | ReadObservation::Rejected { .. } => {}
        }
    }
    // The state the refresh produced is now in `owner.current_preset()`.
    Ok(())
}
```

> **What to verify before relying on this.**
>
> - Which messages the unit pushes unasked, and when, is established here only by
>   the startup sequence the library sends and the types its decoder handles. It
>   is not a description of firmware behaviour.
> - The snapshot reflects what the decoder has been given. If your reader drops a
>   report, the frame stream has a gap. The transport reports a frame it cannot
>   assemble as a [`FrameError`](crate::protocol::framing::FrameError) and resets
>   assembly. The report header carries only a length and the FIRST and LAST
>   flags, and the trailer has no checksum, so a dropped middle report is not
>   detectable from the bytes alone. Keep the read lane lossless.
> - `device.currentPreset` timeouts and the retry cadence are host policy in the
>   profile, not device guarantees.
> - Not every planned read is known to be answered. The docs on
>   [`DeviceDiagnostics`](crate::protocol::responses::DeviceDiagnostics) record
>   that current firmware has not been observed to answer the diagnostics request
>   (it times out), so none of its counters has been observed. The docs
>   *believe* the processing, heap and DMA figures to be percentages; that is a
>   hypothesis, and the meanings of the status words, error flags, buffer
>   indices and dropped counts are not established (see the
>   [limits register](crate::guide::limits)). `device.diagnostics` is planned like any other read, a read of it
>   may end as `Expired`.
