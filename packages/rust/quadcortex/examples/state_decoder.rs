//! Turn device messages into normalized state updates.
//!
//! [`StateDecoder`] is the stateful half of the library: it keeps what it has
//! learned so far (active scene, preset, model catalog) and maps each message
//! the device pushes onto a small, serializable [`StateUpdate`]. This example
//! feeds it payloads built with the command builders, which use the same
//! protobuf messages the device sends back, so it needs no hardware.
//!
//! Run with `cargo run -p quadcortex --example state_decoder`.

use quadcortex::commands::{select_scene, set_tempo, OutboundMessage};
use quadcortex::state::StateDecoder;

fn main() {
    let mut decoder = StateDecoder::new();

    let messages: [(&str, OutboundMessage); 2] = [
        ("scene 3 selected", select_scene(3)),
        ("tempo 120 BPM", set_tempo(120)),
    ];

    for (label, message) in &messages {
        let updates = decoder
            .decode(message.message_type, &message.payload)
            .expect("well-formed payloads decode");
        println!("{label}: {} update(s)", updates.len());
        for update in updates {
            // Updates serialize to the camelCase JSON contract hosts consume.
            println!(
                "  {}",
                serde_json::to_string(&update).expect("updates serialize")
            );
        }
    }

    // Everything the decoder received but could not act on is counted rather
    // than dropped silently.
    println!("inbound telemetry: {:?}", decoder.inbound_telemetry());
}
