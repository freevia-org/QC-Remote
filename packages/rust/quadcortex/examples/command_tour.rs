//! A tour of the typed command layer: validation, message cost and pacing.
//!
//! Every [`DeviceCommand`] and [`DeviceOperation`] has a `try_encode` that
//! validates its arguments before it produces any bytes. This example encodes a
//! handful of them, prints the message type, payload size and number of HID
//! reports each needs, and then shows the typed error that a bad value produces
//! instead of a frame. It runs entirely offline: nothing is sent to a device.
//!
//! Run with `cargo run -p quadcortex --example command_tour`.

use quadcortex::commands::{DeviceCommand, DeviceOperation, OutboundMessage};
use quadcortex::device::request::operation_inter_message_interval_ms;
use quadcortex::protocol::framing::encode;
use quadcortex::protocol::message_registry::message_name;

/// One line per encoded message: its type and name, payload size, report count.
fn describe(label: &str, messages: &[OutboundMessage], pacing_ms: u64) {
    println!("{label}: {} message(s)", messages.len());
    for message in messages {
        println!(
            "    type {:>2} {:<12} {:>3} payload byte(s), {} report(s)",
            message.message_type,
            message_name(message.message_type).unwrap_or("undeclared"),
            message.payload.len(),
            encode(message.message_type, &message.payload).len(),
        );
    }
    if pacing_ms > 0 {
        println!("    host must wait {pacing_ms} ms between messages");
    }
}

fn main() {
    println!("== accepted ==");
    let commands = [
        ("select scene 3", DeviceCommand::SelectScene(3)),
        (
            "bypass block (row 0, column 2)",
            DeviceCommand::SetBypass {
                row: 0,
                column: 2,
                bypassed: true,
            },
        ),
        ("set preset tempo 96 BPM", DeviceCommand::SetTempo(96)),
        ("master volume 0.5", DeviceCommand::SetMasterVolume(0.5)),
    ];
    for (label, command) in commands {
        match command.try_encode() {
            Ok(message) => describe(label, &[message], 0),
            Err(error) => println!("{label}: rejected, {error}"),
        }
    }

    // An operation may expand to several ordered messages. A screen drag is
    // three, and the device protocol needs a gap between them that the host
    // must honour; the library reports it but never sleeps.
    let operations = [
        (
            "bypass global cab on row 0",
            DeviceOperation::SetGlobalBypass {
                cab: Some([true, false, false, false]),
                ir: None,
            },
        ),
        (
            "swipe across the screen",
            DeviceOperation::ScreenDrag {
                x: 100.0,
                y: 100.0,
                to_x: 300.0,
                to_y: 120.0,
            },
        ),
    ];
    for (label, operation) in operations {
        let pacing = operation_inter_message_interval_ms(&operation);
        match operation.try_encode() {
            Ok(messages) => describe(label, &messages, pacing),
            Err(error) => println!("{label}: rejected, {error}"),
        }
    }

    println!("\n== rejected: a typed error, never a frame ==");
    let bad_commands = [
        ("scene 8 (scenes are 0..=7)", DeviceCommand::SelectScene(8)),
        ("tempo 300 BPM", DeviceCommand::SetTempo(300)),
        ("volume 1.5", DeviceCommand::SetMasterVolume(1.5)),
        (
            "bypass at column 9 (columns are 0..=7)",
            DeviceCommand::SetBypass {
                row: 0,
                column: 9,
                bypassed: false,
            },
        ),
    ];
    for (label, command) in bad_commands {
        match command.try_encode() {
            Ok(_) => println!("{label}: unexpectedly accepted"),
            Err(error) => println!("{label}: {error}"),
        }
    }
    let bad_operation = DeviceOperation::AddBlock {
        row: 0,
        column: 0,
        model_id: 0,
    };
    match bad_operation.try_encode() {
        Ok(_) => println!("add block with model 0: unexpectedly accepted"),
        Err(error) => println!("add block with model 0: {error}"),
    }
}
