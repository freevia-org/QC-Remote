# Tutorial: sending commands safely

How to change something on the unit without acting on stale information. The
library has four levels of protection; this page climbs them one at a time. The
snippets all run offline against synthetic state.

Prerequisites: [Concepts](crate::guide::concepts) and
[the state tutorial](crate::guide::read_state).

## The four levels

| Level | API | What it protects against | What it does **not** do |
| --- | --- | --- | --- |
| 0. Builders | [`commands::select_scene`](crate::commands::select_scene) and the other free functions | Nothing. They encode what you pass. | Validate. |
| 1. Typed encode | [`DeviceCommand::try_encode`](crate::commands::DeviceCommand::try_encode), [`DeviceOperation::try_encode`](crate::commands::DeviceOperation::try_encode) | Values outside the range the protocol or this API supports. | Check the device's current state. |
| 2. Planners | [`plan_gateway_write`](crate::device::request::plan_gateway_write), [`plan_preset_mutation`](crate::device::request::plan_preset_mutation) | Writing against state that changed since you last looked; missing confirmations. | Bind the plan to a session. |
| 3. Owner | [`SessionRuntime`](crate::device::owner::SessionRuntime) | A plan outliving its session or its target; refused sends during a device suspension; unrecorded delivery. | Prove the unit acted. Nothing in the library does that on its own; see [Step 6](#6-after-the-send-delivery-is-not-acknowledgement). |

The low-level builders and `framing` encode any message. The crate README says
it plainly: sending arbitrary protocol messages to a device can mutate persistent
state, and higher-level callers should use the guarded operations. Use level 0
for analysis and tests, and levels 2 and 3 for anything that touches a unit.

## 1. Level 1: typed commands check their range

[`DeviceCommand`](crate::commands::DeviceCommand) is the set of live
operations every host supports, and
[`DeviceOperation`](crate::commands::DeviceOperation) is the larger set, some
of which expand to several ordered messages. Both have a `try_encode` that
validates first. The unchecked encoder is private on purpose; a `compile_fail`
test in the crate keeps it that way.

```rust
use quadcortex::commands::{CommandEncodeError, DeviceCommand, DeviceOperation};

// In range: encodes to one message.
let message = DeviceCommand::SelectScene(7).try_encode()?;
assert_eq!(message.message_type, 13);

// Out of range: a typed error instead of bytes. Scenes are 0..=7.
assert_eq!(
    DeviceCommand::SelectScene(8).try_encode(),
    Err(CommandEncodeError::UnsupportedValue { field: "scene", value: "8".into() })
);

// Tempo is 40..=240 BPM; volume is the wire float 0.0..=1.0 and must be finite.
assert!(DeviceCommand::SetTempo(300).try_encode().is_err());
assert!(DeviceCommand::SetMasterVolume(f32::NAN).try_encode().is_err());

// An operation may expand to several messages, validated as a whole.
let messages = DeviceOperation::SetGlobalBypass { cab: Some([true; 4]), ir: None }.try_encode()?;
assert!(!messages.is_empty());
# Ok::<(), CommandEncodeError>(())
```

`examples/command_tour.rs` runs a longer tour of commands and operations, showing
for each its message type, payload size, how many reports it needs, and the error
a bad value produces.

```text
cargo run -p quadcortex --example command_tour
```

### Operations that send more than one frame

Every frame these operations send has the shape the desktop application sends for
one UI edit. Sending several fields in one call is a host convenience: the desktop
application has no multi-field action. The frames go out in the order below, and a
failure between frames can leave a partial state. The validation domains, the
stale-state guards and the wait for the unit's published echo are host policy, not
frames the desktop application sends.

| Operation | Frames, in order |
| --- | --- |
| Input, output and USB port settings | One type-3 UPDATE per supplied field: input level, impedance, input type, ground lift; output level, ground lift, mute; USB level, headphone source, dry/wet. |
| Output pairing | One type-3 UPDATE per supplied couple: outputs 1/2, then outputs 3/4. |
| Global EQ band and output | One type-38 UPDATE per supplied control: gain, frequency, Q, type, enabled; output level, outputs 1/2, outputs 3/4. |
| Tempo and metronome fields | One type-33 UPDATE per supplied field, time signature first, then LED, volume, running, pan, subdivision, sound, routing, and the beat cells ascending. |
| Block footswitch assignment | A type-1 Grid DELETE of the previous assignment, then a type-1 Grid UPDATE of the new one. A fresh assignment is the UPDATE only; an unassignment is the DELETE only. |
| Expression bypass | A type-1 Grid UPDATE of the assignment, then a type-1 Grid UPDATE of the info frame. Whether the desktop application also sends the info frame when clearing is not established; hardware verification pending. |
| Global bypass | One complete type-9 UPDATE per supplied family, Cab then IR. |
| Show tuner | A type-27 UPDATE, a type-6 enable-meter UPDATE and a type-6 READ, the desktop application's own three frames. |
| Move block | One type-12 GridMove frame, sent without the desktop application's field-3 sidecar (see "Hardware verification pending" in the limits register). |
| MIDI Out and preset-load MIDI Out | One type-8 UPDATE carrying all 12 message slots. |

## 2. Level 2: planners and expected-state guards

The planners work in the gateway vocabulary: a method name such as
`device.setTempo`, and a JSON `params` object. For every method that changes
something, certain **expected-state** arguments are *required*, and the planner
compares them with the snapshot you pass in. They answer "is the world still the
way I last saw it?":

| Method (examples) | Required guards |
| --- | --- |
| `device.selectScene`, `device.setStompLabel` and many others | `expectedPresetName` (the preset must still have this name); some add `expectedLabel`, `expectedColor` |
| `device.setTempo` | `expectedTempo`, `expectedPresetName` |
| `device.setMasterVolume` | `expectedValue` |
| `device.toggleBypass` | `expectedModelId`, `expectedBypassed`, `expectedScene`, `expectedPresetName` |
| `device.setParameter` | `expectedModelId`, `expectedValue`, `expectedScene`, `expectedPresetName` |
| `device.addBlock` | `expectedModelId` (must be `null`: it targets an empty cell), `expectedPresetName` |

The authoritative table is the `match` at the top of
[`plan_gateway_write`](crate::device::request::plan_gateway_write); the lists
above are examples, not a complete reference. A guard may also be
`expectedPresetIdentity`, an opaque token from the session owner (see step 6).

A missing guard is an error, not a default. A stale guard is an error that names
what changed. A guard is a precondition on **observed state**; it is not an
authorization, and it is checked against the snapshot you hand over, so a stale
snapshot gives a stale answer.

```rust
use prost::Message;
use quadcortex::commands::{set_tempo, DeviceCommand};
use quadcortex::device::request::{plan_gateway_write, GatewayVerification, PlannedWrite};
use quadcortex::device::state_runtime::DeviceStateRuntime;
use quadcortex::protocol::proto::{binary_preset, cortex_protobuf_v2 as pa, BinaryPreset};
use serde_json::json;

// A synthetic current preset named "Demo" at 100 BPM.
let mut runtime = DeviceStateRuntime::new();
let preset = pa::RecallPresetMessage {
    action: pa::message_action::Enum::Update as i32,
    preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
        name: Some(binary_preset::Name::Name("Demo".into())),
        tempo: Some(binary_preset::Tempo::Tempo(100)),
        ..Default::default()
    })),
    ..Default::default()
}
.encode_to_vec();
runtime.ingest(15, &preset)?;
// The plain integer `tempo` field is read as BPM here; that reading is an
// assumption, not an observation.
assert_eq!(runtime.snapshot().tempo, 100);

// 1. No guards: refused, and the error names the first one missing.
let refused = plan_gateway_write("device.setTempo", &json!({ "bpm": 120 }), Some(runtime.snapshot()));
assert_eq!(refused.unwrap_err(), "expectedTempo is required to guard device.setTempo");

// 2. Guards that match what we observed: a plan.
let params = json!({ "bpm": 120, "expectedTempo": 100, "expectedPresetName": "Demo" });
let plan = plan_gateway_write("device.setTempo", &params, Some(runtime.snapshot()))?;
assert_eq!(plan.write, PlannedWrite::HidCommand(DeviceCommand::SetTempo(120)));
assert_eq!(plan.verification, GatewayVerification::Tempo { bpm: 120 });

// 3. The unit's tempo changes (say, from its own screen) before we send.
let echo = set_tempo(90);
runtime.ingest(echo.message_type, &echo.payload)?;
let stale = plan_gateway_write("device.setTempo", &params, Some(runtime.snapshot()));
assert_eq!(
    stale.unwrap_err(),
    "Tempo changed on the Quad Cortex: expected 100, but 90 is active. Refresh and retry."
);
# Ok::<(), Box<dyn std::error::Error>>(())
```

The plan's `verification` is a predicate over the snapshot. Step 6 uses it.

Two notes about the method names:

- The public method list is
  [`generated_gateway::METHODS`](crate::device::generated_gateway::METHODS);
  [`is_mutation_method`](crate::device::generated_gateway::is_mutation_method)
  says which of them change the device, and
  [`validate_params`](crate::device::generated_gateway::validate_params) checks a
  `params` object against the declared fields before planning.

## 3. Confirmations for surprising operations

Some operations are guarded by an explicit acknowledgement instead of, or as well
as, an expected-state argument. The planner refuses unless the field is the JSON
boolean `true`; a missing or non-boolean value counts as "not confirmed".

| Field | Operation | What the planner's own message says it protects |
| --- | --- | --- |
| `confirmTunerActivation` | `device.setTunerInput`, `setTunerMute`, `setTunerReference`, `setTunerMeter` | Each of these is "believed to engage the tuner invisibly"; changing the mute "may silence every output". |
| `confirmOverwrite` | `device.savePresetAs`, `device.copyPreset` | Writing into an occupied slot. `savePresetAs` requires the field to be a boolean and refuses an occupied slot unless it is `true`; `copyPreset` requires it to be `true`. |
| `confirmRename` | `device.renameCurrentPreset` | Renaming a stored preset. |

Deleting a preset has no confirmation flag in the planner; its guard is
`expectedName`, and it must equal the name the library currently holds for that
slot. Whether a human approved a deletion is outside the library: confirm with
the person in your own application before you call it.

```rust
use quadcortex::device::request::plan_gateway_write;
use serde_json::json;

let unconfirmed = plan_gateway_write("device.setTunerMute", &json!({ "muted": true }), None);
assert_eq!(
    unconfirmed.unwrap_err(),
    "Changing tuner mute is believed to engage the tuner invisibly and may silence every output; \
     confirmTunerActivation must be true."
);

let confirmed = plan_gateway_write(
    "device.setTunerMute",
    &json!({ "muted": true, "confirmTunerActivation": true }),
    None,
)?;
println!("{}", confirmed.detail);
# Ok::<(), String>(())
```

## 4. Persistent writes: what the code classifies

The library distinguishes **live edits to the working preset** from **persistent
workflows** that rewrite stored files:

- A live edit (`device.setParameter`, `device.toggleBypass`, scene and block
  edits, and so on) changes the working preset on the unit.
  [`GatewaySnapshot::dirty`](crate::device::GatewaySnapshot::dirty) reports
  whether the working preset has been edited since it was loaded or saved, and
  several planners refuse to proceed while it is set: renaming a stored preset
  and pressing a PRESET-mode footswitch each tell you to save or discard first.
  These are library policies that protect unsaved work, not frames the desktop
  application sends. Copying a preset does not read or change the working
  preset and does not refuse on `dirty`.
- A persistent preset or setlist workflow is planned by
  [`plan_preset_mutation`](crate::device::request::plan_preset_mutation):
  `device.savePresetAs`, `renameCurrentPreset`, `copyPreset`, `movePreset`
  (a move or a swap), `deletePreset`, `createSetlist`, `deleteSetlist` and
  `duplicateSetlist`. They return a
  [`PresetMutationPlan`](crate::device::request::PresetMutationPlan) of ordered
  stages. Each stage is confirmed before the next is sent, and the
  plan's own doc says the catalog policy "never permits write replay": a
  persistent write is not retried. The planner also refuses to write into the
  Factory Library.
- Two of those workflows are a single desktop file frame each. `renameCurrentPreset`
  plans one File UPDATE addressed by the preset's opaque file key, and
  `copyPreset` plans one File COPY naming the source by its file key and the
  destination by folder and slot index. Neither plans a recall, a save or a
  restore, and `copyPreset` takes no expected-state arguments because it does not
  touch the working preset. The request id is stamped when the frame is sent. The
  catalog read after the frame, the exact-name refusal for a destination that
  already holds the copied name, and the `dirty` refusal for a rename are the
  library's own verification and safety policy, not part of the desktop frame.
  Hardware verification of these two direct frames is pending: no run exists
  since the library began stamping request ids on file mutations. If one reports
  a failure, read the library back instead of sending it again.
- The four tuner writes (`device.setTunerInput`, `setTunerMute`,
  `setTunerReference`, `setTunerMeter`) and `device.setDeviceName` each send
  **one frame and nothing else**: `08 01 18 <id>`, `08 01 28 00|01`,
  `08 01 25 <f32>`, `08 01 30 00|01` and `08 01 7a <length> <name>`. The desktop
  application sends the tuner frames and never reads any of them back, so the
  library does not either: the planner carries no readback for them, and a host
  returns `accepted_unverified` once the frame is delivered. That is not a
  confirmed change; to see what the unit holds, read the tuner settings
  yourself. To restore audio send `setTunerMute` with `muted: false`; there is
  no separate restore operation. The device name is a **device extension**: the
  desktop application has no sender for it, so its frame is derived from the wire
  schema, and whether the unit applies it or keeps it across a restart is **not
  established** (hardware verification is pending). That a tuner write engages
  the tuner invisibly is *believed*, from an undated measurement, which is why
  the confirmation flag above exists; the flag is host policy and not a desktop
  frame.
- `device.undo` and `device.redo` send one UndoRedo frame (`08 01 28 01` and
  `08 01 30 01`, the frames the desktop application sends) and no read after it.
  A host does not refresh its cached preset: whether the unit pushes state after
  an undo is **not established** (hardware verification is pending), so re-read
  the preset yourself if you need it. The `expectedPresetIdentity` guard is
  host policy.
- Device-wide settings (general settings, I/O ports, global EQ, mode cycle, tempo
  mode, device name, and others) are sent as "global" writes with no draft
  stage. Whether the unit stores each one immediately is **not established
  here**, apart from the tuner mute preference, which the planner's own text
  calls persistent. Treat them as configuration of the unit, not as edits you can
  discard.

```rust
use prost::Message;
use quadcortex::device::request::plan_preset_mutation;
use quadcortex::device::state_runtime::DeviceStateRuntime;
use quadcortex::protocol::proto::{binary_preset, cortex_protobuf_v2 as pa, BinaryPreset};
use quadcortex::state::{PresetFileListing, PresetFolderListing};
use serde_json::json;

const SETLIST: &str = "/media/p4/Presets/Demo Setlist";

let mut runtime = DeviceStateRuntime::new();
let preset = pa::RecallPresetMessage {
    action: pa::message_action::Enum::Update as i32,
    preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
        name: Some(binary_preset::Name::Name("Demo".into())),
        ..Default::default()
    })),
    ..Default::default()
}
.encode_to_vec();
let position = pa::SetlistPositionMessage {
    action: pa::message_action::Enum::Update as i32,
    folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(SETLIST.into())),
    position: Some(pa::setlist_position_message::Position::Position(8)),
    ..Default::default()
}
.encode_to_vec();
runtime.ingest(15, &preset)?;
runtime.ingest(2, &position)?;

// Slot 9 of the same setlist already holds a preset called "Other".
runtime.ingest_preset_listing(PresetFolderListing {
    order: None,
    key: SETLIST.into(),
    name: "Demo Setlist".into(),
    file_type: Some(0),
    is_factory: false,
    is_plugin: false,
    files: vec![PresetFileListing {
        position: 9,
        name: "Other".into(),
        product_key: Some(format!("{SETLIST}/Other.pb")),
        instrument_present: true,
        ..Default::default()
    }],
});

let save = |slot: u32, confirm_overwrite: bool| {
    plan_preset_mutation(
        "device.savePresetAs",
        &json!({
            "expectedPosition": 8, "expectedPresetName": "Demo",
            "setlistKey": SETLIST, "position": slot, "name": "Demo v2",
            "confirmOverwrite": confirm_overwrite,
        }),
        Some(runtime.snapshot()),
        runtime.preset_library(),
    )
};

// An empty slot needs no overwrite confirmation (the field must still be a boolean).
let plan = save(10, false)?;
assert_eq!(plan.stages.len(), 1);
assert_eq!(plan.detail, "Saved and verified Demo v2");

// An occupied slot is refused until the caller says "overwrite".
assert!(save(9, false).unwrap_err().contains("explicit overwrite confirmation is required"));
assert!(save(9, true).is_ok());
# Ok::<(), Box<dyn std::error::Error>>(())
```

> **What to verify before relying on this.** The planners decide *what to send*
> and *how to check it*. They cannot know what the unit does with a persistent
> write beyond what its catalog later reports. The retry and settle times in the
> plan (for example the settle period after a save) are host policy generated
> into the profile; they are not firmware guarantees.

## 5. Request ids on writes

Reads carry the request id you give them (see
[Step 2 of the state tutorial](crate::guide::read_state)). Writes are different:

- The scene and tempo builders carry **no** request id. Such a write is confirmed
  from state the unit pushes, which is why a plan carries a `verification`
  predicate wherever device state can confirm the write.
- Preset recall, bank navigation and reload send the desktop application's one
  SetlistPosition UPDATE with **no** request id, like `updateSetlist`
  ([`setlist_position`](crate::commands::setlist_position)). Completion is
  observed as a newer position revision, never from an echoed id; that observation
  names the address only, so the result is `accepted_unverified` with an address
  observation and never claims the loaded preset's contents.
  Hardware verification pending: a recall without a request id completes and
  verifies.
- Mutations of the file catalog (create or delete a setlist, move a preset)
  are stamped with an id at the final send, from the same counter as everything
  else, so two writers never reuse an id
  ([`SessionDriver::stamp_file_mutation_request_id`](crate::session::SessionDriver::stamp_file_mutation_request_id)).
  An id already present is preserved.
- The keepalive carries the fixed id `0` and does not draw from the counter.

```rust
use quadcortex::commands::{create_setlist, keepalive};
use quadcortex::protocol::wire::request_id;
use quadcortex::session::SessionDriver;

let mut driver = SessionDriver::start(7, "demo-session".into(), 0);
let first = driver.reserve_request_id();
let second = driver.reserve_request_id();
assert!(second > first, "one counter; never reused");

let mut message = create_setlist("Live".into());
assert_eq!(request_id(message.message_type, &message.payload), None);
let stamped = driver.stamp_file_mutation_request_id(&mut message)?;
assert_eq!(request_id(message.message_type, &message.payload), stamped);
// Stamping again keeps the id it already has.
assert_eq!(driver.stamp_file_mutation_request_id(&mut message)?, stamped);

let ping = keepalive();
assert_eq!(request_id(ping.message_type, &ping.payload), Some(0));
# Ok::<(), prost::DecodeError>(())
```

## 6. After the send, delivery is not acknowledgement

Planning and sending are not the end. The owner records each frame it let out in
a bounded **delivery journal**, and the journal's own doc is blunt: "Completion
describes the host transport policy, not device acknowledgement." A frame can be
`Completed` (every report was handed to the transport), `Cancelled` (stopped
before any report started, so nothing reached the device) or `Indeterminate`
(stopped after a report started, so whether the unit received any is unknown).

The journal also has a documented outcome for "the stall the QC's USB status is
expected to produce": it counts as delivered and is tallied separately. See
[`ReportOutcome::ExpectedStatusStall`](crate::device::delivery::ReportOutcome::ExpectedStatusStall).

The snippet is the whole owner-level path, offline: observe state, plan a guarded
write, admit it, watch the guard *refuse* it because the target changed, re-plan,
deliver it, and only then use the plan's predicate to see whether the state moved.

Check
[`is_synchronized`](crate::device::owner::SessionRuntime::is_synchronized) before
planning writes in a real host. This synthetic session never completes startup, and
[`plan_operation`](crate::device::owner::SessionRuntime::plan_operation) itself
requires only an *active* lifecycle, so the snippet skips that check.
[Sessions](crate::guide::sessions) explains startup.

```rust
use prost::Message;
use quadcortex::commands::set_tempo;
use quadcortex::device::delivery::{DeliveryStatus, ReportOutcome};
use quadcortex::device::owner::SessionRuntime;
use quadcortex::device::request::{plan_gateway_write, PlannedWrite};
use quadcortex::protocol::proto::{binary_preset, cortex_protobuf_v2 as pa, BinaryPreset};
use quadcortex::transport::{ReportLayout, TransportRuntime};
use serde_json::json;

const SETLIST: &str = "/media/p4/Presets/Demo Setlist";

// A synthetic session: opened, with its lifecycle installed by the first
// handshake attempt. No device is involved.
let mut owner = SessionRuntime::new(0);
let connection = owner.opened(0)?;
owner.next_handshake_write(0, "demo-session", None).expect("the first attempt is due");

let observe = |owner: &mut SessionRuntime, message_type: u16, payload: &[u8]| {
    let ticket = owner.capture_observation(connection)?;
    owner.observe_state(&ticket, message_type, payload).map(|_| ())
};

let preset = pa::RecallPresetMessage {
    action: pa::message_action::Enum::Update as i32,
    preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
        name: Some(binary_preset::Name::Name("Demo".into())),
        tempo: Some(binary_preset::Tempo::Tempo(100)),
        ..Default::default()
    })),
    ..Default::default()
}
.encode_to_vec();
let position = pa::SetlistPositionMessage {
    action: pa::message_action::Enum::Update as i32,
    folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(SETLIST.into())),
    position: Some(pa::setlist_position_message::Position::Position(8)),
    ..Default::default()
}
.encode_to_vec();
observe(&mut owner, 15, &preset)?;
observe(&mut owner, 2, &position)?;

// The owner issues an opaque identity for "this preset, in this session".
let identity = owner.caller_preset_identity()?;
assert!(identity.starts_with("qc-preset-v1."));

// Plan, bind the plan to the session and the state it read, and admit it.
let plan_and_admit = |owner: &mut SessionRuntime, expected_tempo: u32, now_ms: u64| {
    let params = json!({
        "bpm": 120, "expectedTempo": expected_tempo,
        "expectedPresetName": "Demo", "expectedPresetIdentity": identity,
    });
    let plan = plan_gateway_write("device.setTempo", &params, owner.current_preset())?;
    let context = owner.context_for_gateway_write("device.setTempo", &params, &plan.write, now_ms)?;
    let operation = owner.plan_operation_in_context(&context, now_ms, 3_000)?;
    Ok::<_, String>((plan, operation))
};
let (plan, operation) = plan_and_admit(&mut owner, 100, 1_000)?;

// Before we send, the unit's tempo changes. The owner re-checks the guards at
// the native attempt and refuses.
let echo = set_tempo(90);
observe(&mut owner, echo.message_type, &echo.payload)?;
let refused = owner.prepare_dispatch(Some(&operation), None, 1_100);
assert!(refused.unwrap_err().contains("Tempo changed on the Quad Cortex"));

// Re-plan from the fresh state, then send for real.
let (plan, operation) = plan_and_admit(&mut owner, 90, 1_200)?;
let PlannedWrite::HidCommand(command) = &plan.write else { unreachable!("setTempo is a command") };
let message = command.clone().try_encode()?;
let reports = TransportRuntime::encode_reports(&message, ReportLayout::ReportIdPrefixed)?;

let dispatch = owner.prepare_dispatch(Some(&operation), None, 1_210)?;
let delivery = owner.prepare_delivery(&dispatch, message.message_type, reports.len(), 1_210)?;
owner.delivery_admitted(&delivery)?;
for index in 0..reports.len() {
    owner.begin_native_report(&delivery, index, 1_211)?;
    // ... the host hands reports[index] to its USB handle here ...
    owner.finish_native_report(&delivery, index, ReportOutcome::Delivered, 1_212)?;
}
let record = owner.require_delivery_complete(&delivery)?;
assert_eq!(record.status, DeliveryStatus::Completed);

// Completed delivery is not proof that the unit changed. Only state does that.
assert!(!plan.verification.matches(owner.current_preset().unwrap(), None));
let confirmed_by_device = set_tempo(120); // stands in for the unit's pushed echo
observe(&mut owner, confirmed_by_device.message_type, &confirmed_by_device.payload)?;
assert!(plan.verification.matches(owner.current_preset().unwrap(), None));
# Ok::<(), Box<dyn std::error::Error>>(())
```

For a host that wants the library to run the wait, there is a verification
sequencer: [`GatewayVerificationRuntime::advance`](crate::device::request::GatewayVerificationRuntime::advance)
returns *wait*, *refresh* (send this read), *verified* or *timed out*, from the
policy in [`gateway_write_verification_policy`](crate::device::request::gateway_write_verification_policy).
It never decides that a write landed without an observation newer than the write.

> **What to verify before relying on this.**
>
> - A *verified* result means a state observation matched the predicate, newer
>   than the write. A *timed out* result is not proof the write failed.
> - Several writes are **realtime**: they return as soon as their bytes reach the
>   transport and let the pushed state reconcile later
>   ([`gateway_write_is_realtime`](crate::device::request::gateway_write_is_realtime)).
>   For those, "sent" and "settled" are different moments.
> - A suspended device refuses requests; see
>   [Sessions](crate::guide::sessions). A refusal for a request that never left is
>   worded differently from a partial send whose outcome is uncertain.
