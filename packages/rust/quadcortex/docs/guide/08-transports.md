# Transports: what the crate provides and what the host provides

What the crate gives you for the USB side, and what your host program must
supply. The short version: the crate decides *what* goes on the wire and *when*,
and never touches the wire itself.

## The boundary

| The crate provides | The host provides |
| --- | --- |
| Report framing in both directions, in two native layouts ([`TransportRuntime::encode_reports`](crate::transport::TransportRuntime::encode_reports), [`push_report`](crate::transport::TransportRuntime::push_report)) | Enumerating and opening the device, matching [`profile::VENDOR_ID`](crate::protocol::profile::VENDOR_ID) and [`SUPPORTED_PRODUCT_IDS`](crate::protocol::profile::SUPPORTED_PRODUCT_IDS) |
| Handshake attempts, their pacing and their budget; reconnect and backoff policy; keepalive scheduling | The actual writes and reads, on whatever threads and queues you use |
| The pacing rule for multi-report writes ([`report_batch_delay_ms`](crate::transport::TransportRuntime::report_batch_delay_ms)) | The sleeps that implement it |
| Staged startup and the seed ([`SessionDriver`](crate::session::SessionDriver)) | Submitting each batch on **one serialized writer** and reporting accepted or failed |
| Session binding: tickets, fences, epochs | A monotonic clock in milliseconds, and a fresh opaque session id per attempt |
| Verification and delivery bookkeeping | Classifying each OS write result as delivered, expected stall, or failed ([`ReportOutcome`](crate::device::delivery::ReportOutcome)) |
| MIDI control-change *plans* for the physical-control methods ([`plan_host_midi`](crate::device::request::plan_host_midi)) | A MIDI output bound to the same unit, and sending the plan |

The crate has no I/O and no platform-specific code. Everything you can run in this
guide runs in a plain test process for exactly that reason.

## The two native layouts

Native USB APIs disagree about what a HID report looks like to the caller. The
crate encodes and accepts both, selected by
[`ReportLayout`](crate::transport::ReportLayout):

- `ReportIdPrefixed`: the complete 129-byte report, with the report-ID byte.
- `BodyOnly`: the 128-byte data stage without the report ID.

On the inbound side you do not choose. `push_report` accepts a 129-byte report
whose first byte is the inbound report ID, or a 128-byte body. The source comment
says why both exist: Android interrupt reads expose the 128-byte body, while
`hidapi` exposes the report-ID-prefixed form. A body whose first byte happens to
equal the report ID is not confused with the other layout, because **length**, not
byte value, selects it.

```rust
use quadcortex::protocol::framing::{encode, IN_REPORT_ID};
use quadcortex::transport::TransportRuntime;

let payload = vec![0x5A_u8; 300];
let reports: Vec<Vec<u8>> = encode(7, &payload)
    .into_iter()
    .map(|report| {
        let mut report = report.to_vec();
        report[0] = IN_REPORT_ID; // as the device would deliver it
        report
    })
    .collect();

let feed = |bodies: bool| -> Result<(u16, Vec<u8>), Box<dyn std::error::Error>> {
    let mut transport = TransportRuntime::new(0);
    for report in &reports {
        let raw: &[u8] = if bodies { &report[1..] } else { report }; // 128 or 129 bytes
        if let Some(frame) = transport.push_report(raw)? {
            return Ok((frame.message_type, frame.payload));
        }
    }
    Err("no frame".into())
};

// Either layout assembles the same message.
assert_eq!(feed(false)?, (7, payload.clone()));
assert_eq!(feed(true)?, (7, payload));
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Stamping reads, and why epochs exist

A host usually reads on one thread and processes on another. Reports sit in a
queue between them, and a session can be replaced while some are still in it.

- Give the library the **time each report was read**, from the same monotonic
  clock as everything else, with
  [`push_report_at`](crate::transport::TransportRuntime::push_report_at). The
  returned frame carries [`IngressTime`](crate::transport::IngressTime) for its
  first and last reports. Reply matching and backups use it to prove a frame
  arrived *after* the request; a frame with no timestamp cannot confirm a read
  ([`ResponseExpectation::observe_received`](crate::device::correlation::ResponseExpectation::observe_received)).
- Give the library a **report epoch**,
  [`push_report_in_epoch`](crate::transport::TransportRuntime::push_report_in_epoch),
  or let the owner stamp it with
  [`capture_report_epoch`](crate::device::owner::SessionRuntime::capture_report_epoch).
  An epoch delimits a byte stream, including partial frames. When the epoch
  changes, the assembler resets and a continuation report cannot start a new
  frame; it waits for a report with the FIRST flag.

```rust
use quadcortex::device::transport::IngressTime;
use quadcortex::protocol::framing::{encode, IN_REPORT_ID};
use quadcortex::transport::TransportRuntime;

let reports: Vec<Vec<u8>> = encode(7, &[0x11; 300])
    .into_iter()
    .map(|report| {
        let mut report = report.to_vec();
        report[0] = IN_REPORT_ID;
        report
    })
    .collect();
assert_eq!(reports.len(), 3);

let mut transport = TransportRuntime::new(0);

// Epoch 1 delivers the first report of a frame, then the reader is replaced.
assert!(transport.push_report_in_epoch(&reports[0], 10, 1)?.is_none());

// Epoch 2 sees only the tail. It cannot complete a frame it did not start.
assert!(transport.push_report_in_epoch(&reports[1], 11, 2)?.is_none());
assert!(transport.push_report_in_epoch(&reports[2], 12, 2)?.is_none());

// A whole frame in the new epoch assembles, with its read times attached.
let mut finished = None;
for (index, report) in reports.iter().enumerate() {
    finished = transport.push_report_in_epoch(report, 20 + index as u64, 2)?;
}
let frame = finished.expect("a complete frame in the current epoch");
assert_eq!(frame.report_count, 3);
assert_eq!(
    frame.ingress,
    Some(IngressTime { first_report_at_ms: 20, last_report_at_ms: 22 })
);
# Ok::<(), quadcortex::protocol::framing::FrameError>(())
```

## Writes: serialize, pace, classify

The rules the library states for the write side:

- **One serialized writer.** The session driver's doc says hosts "submit it on
  their serialized native writer", and the delivery journal requires reports of one
  frame to start in order and one at a time. Do not interleave reports of two
  frames.
- **Pace multi-report frames.** After every group of
  `profile::HID_REPORT_BATCH_SIZE` reports, yield for
  `profile::HID_REPORT_BATCH_DELAY_MS`. The runtime returns the delay; it does
  not sleep. See the snippet in [the state tutorial](crate::guide::read_state).
- **Encode once.** [`encode_reports`](crate::transport::TransportRuntime::encode_reports)
  encodes a message exactly once and adapts only its outer shape.
- **Tell the journal what happened.** Report each write as
  [`Delivered`](crate::device::delivery::ReportOutcome::Delivered),
  [`ExpectedStatusStall`](crate::device::delivery::ReportOutcome::ExpectedStatusStall)
  or [`Failed`](crate::device::delivery::ReportOutcome::Failed). The middle one is
  documented as "the stall the QC's USB status is expected to produce"; it
  *counts as delivered* and is tallied separately. A write that fails any other
  way stops the transfer with
  [`DeliveryStop::TransportFailure`](crate::device::delivery::DeliveryStop::TransportFailure).
  Which OS result corresponds to which outcome depends on your USB stack, and is
  **not established here**.
- A terminal reader failure is different from a transient read error. Report it
  with [`terminal_read_failed`](crate::transport::TransportRuntime::terminal_read_failed)
  so the runtime does not suggest retrying on the same handle.

## Not everything is HID: host MIDI

A few physical-control methods are sent as **MIDI Control Change** instead of HID:
footswitch presses, tap tempo and the Looper X controls
([`PERFORMANCE_MIDI_METHODS`](crate::device::generated_gateway::PERFORMANCE_MIDI_METHODS)).
The library maps the method to a controller number and value; the host sends it on
a MIDI output that belongs to the same unit.
[`plan_host_midi`](crate::device::request::plan_host_midi) is that mapping.

```rust
use quadcortex::device::request::{is_host_midi_method, plan_host_midi};
use quadcortex::protocol::profile;
use serde_json::json;

assert!(is_host_midi_method("device.tapTempo"));
assert!(!is_host_midi_method("device.setTempo"));

let plan = plan_host_midi("device.tapTempo", &json!({ "expectedMode": "STOMP" }))?;
assert_eq!(plan.controller, profile::TAP_TEMPO_CONTROLLER);
assert_eq!(plan.value, profile::MIDI_PRESSED_VALUE);

// Footswitch index 2 maps onto the base controller plus the index.
let plan = plan_host_midi("device.pressFootswitch", &json!({ "index": 2, "expectedMode": "SCENE" }))?;
assert_eq!(plan.controller, profile::FOOTSWITCH_BASE_CONTROLLER + 2);

// A method that does not use host MIDI is an error, not a silent no-op.
assert!(plan_host_midi("device.setTempo", &json!({})).is_err());
# Ok::<(), String>(())
```

Which MIDI endpoint belongs to which unit is the host's problem. The repository's
Windows MIDI crate documents binding MIDI to the same device container as the HID
session and never falling back to a similarly named output; that is its policy,
not something this crate enforces.

## Windows-only and Android pieces

`quadcortex` has none. The repository contains separate crates that implement the
host side for two platforms. They are not published, are not part of this crate's
stability statement, and are described here only so you know where to read a
working host.

| Crate (in this repository) | Role |
| --- | --- |
| `packages/rust/quadcortex-windows` | Windows USB/HID transport: opening the device by vendor and product id, a read lane and a write lane on dedicated threads, the handshake retry loop, and a diagnostics ring buffer. Its manifest pins the `hidapi` crate to an exact version with the pure-Rust Windows backend, and explains in a comment that a `Sync` implementation is sound only for that version's internals. |
| `packages/rust/qc-windows-midi` | The Windows MIDI output used for the host-MIDI methods above. |
| `packages/rust/qc-windows-process` | Windows process-liveness checks. |
| `packages/rust/quadcortex-android` | A JNI binding through which the Android app uses the shared library; the repository README describes the app as talking to the device over USB through it. |
| `services/device-broker` | The single-owner broker that holds the exclusive device connection on Windows. It is linked into the app's shipped `qc-device-broker.exe`, which also runs the notification icon and the background MCP host. |

Read `quadcortex-windows/src/usb.rs` for a complete connect loop, read lane and
write path around [`SessionRuntime`](crate::device::owner::SessionRuntime). It is
one host's solution to the problems on this page, not a specification.

> **What to verify before relying on this.**
>
> - Exclusive access matters: the repository's Windows design routes everything
>   through one broker that holds the single device handle. Whether your platform
>   lets two processes share the unit, and what the unit does if they try, is
>   **not established here**.
> - The pacing constants and report sizes are in the generated profile. Changing
>   them is a protocol-compatibility decision, not a tuning knob.
> - Nothing in this guide ran against a USB stack. The layout and epoch snippets
>   above prove the library's assembly logic only.
