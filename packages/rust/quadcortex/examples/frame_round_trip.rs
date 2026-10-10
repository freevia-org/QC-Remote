//! Build a typed command, frame it into HID reports, and decode it again.
//!
//! This runs entirely offline: nothing is sent to a device. It shows the three
//! layers a host composes when it talks to a Quad Cortex:
//!
//! 1. [`quadcortex::commands`] builds the protobuf payload for an action.
//! 2. [`quadcortex::framing`] splits the payload into 128-byte HID report bodies
//!    and reassembles them.
//! 3. [`quadcortex::message_registry`] names and decodes any declared message
//!    type.
//!
//! Run with `cargo run -p quadcortex --example frame_round_trip`.

use quadcortex::commands::{select_scene, set_tempo, OutboundMessage};
use quadcortex::framing::{decode, encode, REPORT_SIZE};
use quadcortex::message_registry::{decode_any, message_name, DecodedMessage};

/// The first line of a message's debug form, shortened for display.
fn summary(message: &DecodedMessage) -> String {
    let text = format!("{message:?}");
    match text.char_indices().nth(110) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text,
    }
}

fn round_trip(label: &str, command: &OutboundMessage) {
    // Outbound: one logical message becomes one or more fixed-size HID reports.
    let reports = encode(command.message_type, &command.payload);
    println!(
        "{label}: type {} ({}), {} payload bytes -> {} report(s) of {REPORT_SIZE} bytes",
        command.message_type,
        message_name(command.message_type).unwrap_or("undeclared"),
        command.payload.len(),
        reports.len(),
    );

    // Inbound: a host reassembles reports back into the logical message.
    let reports: Vec<Vec<u8>> = reports.into_iter().map(Vec::from).collect();
    let (message_type, payload) = decode(&reports).expect("a freshly encoded frame decodes");
    assert_eq!(message_type, command.message_type);
    assert_eq!(payload, command.payload);

    // Any declared message type decodes into a typed protobuf value.
    match decode_any(message_type, &payload) {
        Some(Ok(message)) => println!("  decoded as {}", summary(&message)),
        Some(Err(error)) => println!("  declared type, but the body is malformed: {error}"),
        None => println!("  type {message_type} is not declared by the protocol"),
    }
}

fn main() {
    round_trip("select scene 3", &select_scene(3));
    round_trip("set tempo 120 BPM", &set_tempo(120));
}
