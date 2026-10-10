# Testing your code without a device

How to test code that uses `quadcortex` without a unit on the desk. The library
never reads a clock and never opens a handle, so its behaviour can be driven by
bytes and numbers you choose.

Everything on this page runs under `cargo test --doc`. None of it touches
hardware, and none of it is evidence about firmware behaviour. It tests *your*
code against the library's documented behaviour.

## What the crate gives you to test with

| Resource | What it is | Use it for |
| --- | --- | --- |
| `tests/fixtures/sanitized-session-v1.json` | A **synthetic, sanitized** trace: three frames (one multi-report) with their raw reports in hex, a privacy block saying it is synthetic, and the expected payloads. | Exercising a frame reader or decoder. See [section 3](crate::guide::offline_quick_start). |
| `tests/replay.rs` | The crate's own test that replays that fixture, checks the privacy flags, and fails if a field such as a serial number, token, credential, preset name or device name appears in it. | A pattern to copy for your own fixtures. |
| `examples/` | Runnable offline programs. | Reading, or adapting into tests. |
| `device-test-fixtures/` | Shared synthetic input that this repository's Android adapter and device broker include in their own tests: a Rust helper that builds protocol-shaped state, and a JSON file of directory-workflow scenarios. Its header calls it deterministic protocol-shaped test data, "not a captured device session". It is shipped in the crate but is **not compiled by the crate's own tests**. | Reading a worked example of scripting a session; do not depend on its API. |
| The crate's unit tests | Several hundred in-source tests, plus property tests with `proptest` and a committed regression file. | The best documentation of an edge case: look for a test named after the behaviour. |

Run the crate's own suite, and its examples, from the repository root:

```text
cargo test -p quadcortex --locked
cargo build -p quadcortex --examples --locked
```

## 1. Pin exact bytes

A frame has one correct encoding. Pin it, so that a change to your builder, or to
a library upgrade, cannot slip through. The expected bytes here are written out by
hand from the report layout in [Concepts](crate::guide::concepts).

```rust
use quadcortex::commands::select_scene;
use quadcortex::protocol::framing::{decode_reports, encode, REPORT_SIZE};

let command = select_scene(3);
let reports = encode(command.message_type, &command.payload);

// report ID, data length (4 payload + 8 trailer), FIRST|LAST, the payload,
// then the trailer: message type 13 as u32, four zero bytes.
let mut expected = vec![0x02, 0x0c, 0xc0, 0x08, 0x01, 0x18, 0x03, 0x0d, 0, 0, 0, 0, 0, 0, 0];
expected.resize(REPORT_SIZE, 0);
assert_eq!(reports[0].as_slice(), expected.as_slice());

// And back again.
let frame = decode_reports(&[expected])?;
assert_eq!((frame.message_type, frame.payload.as_slice()), (13, &command.payload[..]));
# Ok::<(), quadcortex::protocol::framing::FrameError>(())
```

## 2. Control time

Every runtime takes `now_ms` as an argument. A test for a timeout therefore does
not sleep; it passes a number.

```rust
use quadcortex::device::initialization::DeviceStartupPhase;
use quadcortex::protocol::profile;
use quadcortex::session::{SessionDriver, SessionFailure};

let mut driver = SessionDriver::start(1, "session-1".into(), 100);
let batch = driver.next_batch(100, 0).expect("the reset-comms batch");
assert_eq!(driver.phase(), DeviceStartupPhase::SessionValidating);

// One millisecond before the readiness budget: still healthy.
driver.tick(100 + profile::READY_WAIT_TIMEOUT_MS - 1);
assert!(driver.failure().is_none());

// At the budget: the session fails, and the batch it issued is revoked.
driver.tick(100 + profile::READY_WAIT_TIMEOUT_MS);
assert_eq!(
    driver.failure(),
    Some(&SessionFailure::StartupTimeout(DeviceStartupPhase::SessionValidating))
);
assert!(!driver.is_current(&batch.ticket));
```

The same holds for `GatewayReadRuntime` and `ResponseExpectation` deadlines,
`BackupRuntime` windows and `RequestDeadline`. A test of the *late reply
after expiry* case is one line.

## 3. Script a fake device and run your host loop against it

A host loop is: write what the lifecycle has ready, read what comes back, feed it
to the owner. If your own link is a trait, as in this guide's `HidLink` seam, a
test can swap in a **scripted device** that reassembles what you write and queues
canned replies. The snippet below drives the real
[`SessionRuntime`](crate::device::owner::SessionRuntime) over real reports for the
first stages of startup.

The replies are synthetic and minimal. They prove your loop sends batches in the
library's order, frames and reassembles correctly, and moves the lifecycle on. They
do not prove a unit behaves like the script.

```rust
use prost::Message;
use quadcortex::device::initialization::DeviceStartupPhase;
use quadcortex::device::owner::SessionRuntime;
use quadcortex::device::session::DispatchOutcome;
use quadcortex::protocol::framing::{encode, FrameAssembler, IN_REPORT_ID};
use quadcortex::protocol::proto::cortex_protobuf_v2 as pa;
use quadcortex::transport::ReportLayout;
use std::collections::VecDeque;

/// A scripted stand-in for the unit. It reassembles what the host writes and
/// queues canned replies, as inbound reports.
#[derive(Default)]
struct ScriptedDevice {
    assembler: FrameAssembler,
    inbound: VecDeque<Vec<u8>>,
    written_types: Vec<u16>,
}

impl ScriptedDevice {
    fn write_report(&mut self, report: &[u8]) {
        let Some((message_type, payload)) = self.assembler.push(report.to_vec()).expect("valid report")
        else {
            return; // not the last report of its message yet
        };
        self.written_types.push(message_type);
        match message_type {
            // ResetCommsBuffers: echo the session id back.
            52 => {
                let request = pa::ResetCommsBuffersMessage::decode(payload.as_slice()).expect("decodes");
                self.reply(52, request.encode_to_vec());
            }
            // Version READ: answer as a Quad Cortex that accepts the host version.
            10 => {
                let version = pa::VersionMessage {
                    action: pa::message_action::Enum::Update as i32,
                    device_type: Some(pa::version_message::DeviceTypeOneOf::DeviceType(
                        pa::version_message::DeviceType::Qc as i32,
                    )),
                    cortex_control_version_valid: Some(
                        pa::version_message::CortexControlVersionValid::CortexControlVersionValid(true),
                    ),
                    ..Default::default()
                };
                self.reply(10, version.encode_to_vec());
            }
            _ => {}
        }
    }

    fn reply(&mut self, message_type: u16, payload: Vec<u8>) {
        for report in encode(message_type, &payload) {
            let mut report = report.to_vec();
            report[0] = IN_REPORT_ID;
            self.inbound.push_back(report);
        }
    }
}

let layout = ReportLayout::ReportIdPrefixed;
let mut owner = SessionRuntime::new(0);
let mut device = ScriptedDevice::default();
let connection = owner.opened(0)?;
owner.next_handshake_write(0, "session-1", Some(layout)).expect("attempt 1 is due");

let mut now = 0;
for _turn in 0..4 {
    // 1. Send whatever the lifecycle has ready, one identified batch at a time.
    while let Some(batch) = owner.lifecycle.as_mut().and_then(|l| l.next_batch(now, 0)) {
        for message in &batch.messages {
            for report in owner.transport.encode_reports_with_session(message, layout)? {
                device.write_report(&report);
            }
        }
        owner.lifecycle.as_mut().expect("lifecycle").dispatched(&batch.ticket, DispatchOutcome::Accepted);
    }
    // 2. Read what the device queued and feed it to the owner.
    while let Some(report) = device.inbound.pop_front() {
        now += 10;
        let epoch = owner.capture_report_epoch(connection)?;
        if let Some((frame, ticket)) = owner.push_native_report(connection, epoch, &report, now)? {
            owner.observe_lifecycle(&ticket, frame.message_type, &frame.payload, now);
        }
    }
}

// The batches left in the order the library defines: reset, version read,
// the disconnect pair, then the building pair (ModelRepo read last).
assert_eq!(device.written_types, [52, 10, 26, 49, 49, 51]);
let lifecycle = owner.lifecycle.as_ref().expect("lifecycle");
assert_eq!(lifecycle.phase(), DeviceStartupPhase::Building);
assert!(lifecycle.failure().is_none());
# Ok::<(), Box<dyn std::error::Error>>(())
```

The scripted device stops answering at the model-catalog read, so the lifecycle
waits in `Building`: exactly what a unit that never answered would produce, which
makes it a convenient place to test your timeout handling.
`examples/startup_walkthrough.rs` extends the same idea to the whole sequence.

## 4. Test the failure paths on purpose

Awkward cases are hard to provoke on a unit on demand. Synthetic input produces
them every time:

- **Truncated and reordered reports.** Feed `FrameAssembler` a continuation with no
  FIRST, a short report, an oversized declared length. Each has a defined result
  ([`FrameError`](crate::protocol::framing::FrameError)).
- **Declared type, bad body.** `decode_any(type, garbage)` returns `Some(Err(_))`;
  an undeclared type returns `None`.
- **A frame the owner must not trust.** Encrypted flag set, compressed body that
  will not inflate, or a type past `u16`. See
  [Troubleshooting](crate::guide::troubleshooting).
- **Late and duplicate callbacks.** Report a batch `dispatched` twice, or after a
  rebuild; observe a state frame with an old ticket. All return "ignored", not an
  error ([Sessions](crate::guide::sessions)).
- **A suspension in the middle of a write.** Feed the SuspendConnection frame and
  watch the owner refuse the next frame of a multi-report request.

## 5. Keep fixtures clean

If you record your own traces to replay:

- **Record at the report level.** Direction, a monotonic timestamp and the raw
  report bytes as hex are enough to rebuild everything above. The crate's fixture
  format is a model. The library does not record traces for you.
- **A capture of a unit is personal data.** Messages can carry device and preset
  names, serial numbers and library contents. Strip or synthesize them before a
  trace leaves your machine or enters a repository.
- **Add a guard test.** The crate's replay test asserts that a fixture says it is
  synthetic and does not contain forbidden field names. A test like that fails
  loudly when someone pastes a real capture in.
- **Say what a fixture is.** Mark synthetic data as synthetic, in the file. A
  hand-built frame that decodes proves your code and the decoder agree. It does
  not prove the unit sends that frame.

## 6. Inspect what the library chose not to act on

A frame can be assembled and still be unusable. The transport and the state
decoder keep per-connection tallies of every such outcome, so a test can assert
that a scripted frame was counted, not silently lost. That is the subject of
[Troubleshooting](crate::guide::troubleshooting).
