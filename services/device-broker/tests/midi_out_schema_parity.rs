//! The generated public action schema (from `contracts/public/qc-actions.v1.json`)
//! and the shared MIDI Out bounds in `quadcortex::commands` must agree, or MCP
//! validation would admit a value the encoder refuses, or the other way round.

use quadcortex::commands::{
    MIDI_OUT_CHANNELS, MIDI_OUT_PARAMETER_MAX, MIDI_OUT_SLOTS_PER_SOURCE, MIDI_OUT_SOURCES,
    MIDI_OUT_TYPES, MIDI_OUT_TYPE_NONE,
};
use serde_json::Value;

const GENERATED: &str = include_str!("../../rust-mcp/src/generated_actions.rs");

fn input_schema(action: &str) -> Value {
    let start = GENERATED
        .find(&format!("name: \"{action}\","))
        .unwrap_or_else(|| panic!("{action} is generated"));
    let rest = &GENERATED[start..];
    let open = rest.find("input_schema: \"").expect("input schema") + "input_schema: \"".len();
    let close = rest[open..].find("\",\n").expect("schema literal ends") + open;
    let literal = rest[open..close]
        .replace("\\\"", "\"")
        .replace("\\\\", "\\");
    serde_json::from_str(&literal).expect("the generated input schema is JSON")
}

fn bounds(property: &Value) -> (u32, u32) {
    (
        u32::try_from(property["minimum"].as_u64().expect("minimum")).expect("u32 minimum"),
        u32::try_from(property["maximum"].as_u64().expect("maximum")).expect("u32 maximum"),
    )
}

#[test]
fn public_midi_out_schema_bounds_equal_the_shared_definition() {
    let mut checked = 0;
    for (action, lists) in [
        ("set_midi_out", &["messages"][..]),
        (
            "set_preset_load_midi_out",
            &["messages", "expected_messages"][..],
        ),
    ] {
        let schema = input_schema(action);
        for list in lists {
            let array = &schema["properties"][list];
            assert_eq!(
                array["maxItems"].as_u64(),
                Some(MIDI_OUT_SLOTS_PER_SOURCE as u64),
                "{action}.{list}"
            );
            let message = &array["items"]["properties"];
            // The upper bound is the hazard: type 4 and above must never pass
            // schema validation. The lower bound is either the first defined
            // type or the empty-slot type, which keeps a row's position.
            let (type_min, type_max) = bounds(&message["type"]);
            assert_eq!(type_max, *MIDI_OUT_TYPES.end(), "{action}.{list}.type");
            assert!(
                type_min == MIDI_OUT_TYPE_NONE || type_min == *MIDI_OUT_TYPES.start(),
                "{action}.{list}.type minimum {type_min}"
            );
            let (channel_min, channel_max) = bounds(&message["channel"]);
            assert_eq!(channel_max, *MIDI_OUT_CHANNELS.end(), "{action}.{list}");
            assert!(
                channel_min <= *MIDI_OUT_CHANNELS.start(),
                "{action}.{list}.channel minimum {channel_min}"
            );
            for param in ["param1", "param2", "param3"] {
                assert_eq!(
                    bounds(&message[param]),
                    (0, MIDI_OUT_PARAMETER_MAX),
                    "{action}.{list}.{param}"
                );
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 3);
    assert_eq!(
        bounds(&input_schema("set_midi_out")["properties"]["source"]),
        (*MIDI_OUT_SOURCES.start(), *MIDI_OUT_SOURCES.end())
    );
}
