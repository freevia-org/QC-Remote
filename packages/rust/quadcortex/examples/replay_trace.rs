//! Replay a recorded HID report stream through the frame assembler.
//!
//! The trace is the synthetic, sanitized session used by this crate's own
//! tests: it holds no device identity and no preset content. Each frame lists
//! the raw reports as hex; [`FrameAssembler`] turns them back into logical
//! messages exactly as a host would while reading from a live device. The last
//! frame spans several reports and carries filler bytes rather than a real
//! message, so it reassembles correctly but does not decode as its type.
//!
//! Run with `cargo run -p quadcortex --example replay_trace`.

use quadcortex::framing::FrameAssembler;
use quadcortex::message_registry::{decode_any, message_name};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Trace {
    name: String,
    frames: Vec<TraceFrame>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TraceFrame {
    direction: String,
    reports_hex: Vec<String>,
}

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("valid hex"))
        .collect()
}

fn main() {
    let trace: Trace =
        serde_json::from_str(include_str!("../tests/fixtures/sanitized-session-v1.json"))
            .expect("the bundled fixture is valid JSON");
    println!(
        "trace {:?}: {} recorded frame(s)",
        trace.name,
        trace.frames.len()
    );

    let mut assembler = FrameAssembler::new();
    for recorded in &trace.frames {
        for report in &recorded.reports_hex {
            // `push` returns `Some` once the last report of a frame arrives.
            let Some((message_type, payload)) = assembler
                .push(from_hex(report))
                .expect("the fixture holds valid reports")
            else {
                continue;
            };
            let decoded = match decode_any(message_type, &payload) {
                Some(Ok(_)) => "decodes",
                Some(Err(_)) => "body does not decode as its type",
                None => "undeclared type",
            };
            println!(
                "{:>8} type {message_type:>2} {:<18} {:>3} payload byte(s), {decoded}",
                recorded.direction,
                message_name(message_type).unwrap_or("?"),
                payload.len(),
            );
        }
    }
}
