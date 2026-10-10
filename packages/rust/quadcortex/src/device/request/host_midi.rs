use super::*;

/// A MIDI Control Change that a host sends for a physical-control gateway method, as planned by
/// [`plan_host_midi`].
#[derive(Debug, Clone, PartialEq)]
pub struct HostMidiPlan {
    /// Control Change controller number to send.
    pub controller: u8,
    /// Control Change data value to send.
    pub value: u8,
    /// Short human-readable description of what is being sent, for the write result.
    pub detail: String,
}

/// Whether `method` is one of the performance methods that are sent as MIDI rather than HID.
///
/// Such methods are planned with [`plan_host_midi`].
pub fn is_host_midi_method(method: &str) -> bool {
    crate::device::generated_gateway::PERFORMANCE_MIDI_METHODS.contains(&method)
}

/// Shared mapping for physical QC controls. Both native hosts execute this
/// plan with their persistent platform MIDI handle.
pub fn plan_host_midi(method: &str, params: &Value) -> Result<HostMidiPlan, String> {
    match method {
        "device.pressFootswitch" => {
            let index = bounded_u32(params, "index", domain::SCENE_COUNT - 1)? as u8;
            let expected_mode = required_text(params, "expectedMode")?;
            if !matches!(
                expected_mode.as_str(),
                "PRESET" | "SCENE" | "STOMP" | "HYBRID"
            ) {
                return Err("expectedMode must be PRESET, SCENE, STOMP, or HYBRID".into());
            }
            Ok(HostMidiPlan {
                controller: profile::FOOTSWITCH_BASE_CONTROLLER + index,
                value: profile::MIDI_PRESSED_VALUE,
                detail: format!("Footswitch index {index} sent"),
            })
        }
        "device.tapTempo" => {
            let expected_mode = required_text(params, "expectedMode")?;
            if !matches!(
                expected_mode.as_str(),
                "PRESET" | "SCENE" | "STOMP" | "HYBRID"
            ) {
                return Err("expectedMode must be PRESET, SCENE, STOMP, or HYBRID".into());
            }
            Ok(HostMidiPlan {
                controller: profile::TAP_TEMPO_CONTROLLER,
                value: profile::MIDI_PRESSED_VALUE,
                detail: "Tap Tempo sent".into(),
            })
        }
        "device.controlLooper" => {
            let command = required_text(params, "command")?;
            let supplied = params.get("value").filter(|value| !value.is_null());
            // CC#48 is deliberately inverted relative to every other MENU ACCESS &
            // FEATURE CONTROL toggle: per [MANUAL 7.3], values 0-63 open the
            // Looper X parameter editor and values 64-127 close it, the
            // opposite polarity from Tuner (CC#45) and Gig View (CC#46), where
            // low closes and high opens. This is not a transcription slip: the
            // CorOS 2.1.0 release notes briefly listed the reverse mapping
            // (64-127 opening the Looper UI), and the CorOS 3.1.0 notes record a
            // fix restoring the polarity the current manual states. Do not
            // "correct" open/close here to match Tuner/Gig View's pattern.
            // Observed on a CorOS 4.1.0 unit, 2026-10-07 (channel 1): values 0
            // and 63 opened the Looper X view and 127 closed it, confirming the
            // manual's polarity.
            let (controller, value, needs_value, maximum) = match command.as_str() {
                "open" => (profile::LOOPER_MENU_CONTROLLER, 0, false, 0),
                "close" => (
                    profile::LOOPER_MENU_CONTROLLER,
                    profile::MIDI_PRESSED_VALUE,
                    false,
                    0,
                ),
                "duplicate" => (
                    profile::LOOPER_DUPLICATE_CONTROLLER,
                    profile::MIDI_PRESSED_VALUE,
                    false,
                    0,
                ),
                "oneShot" => (
                    profile::LOOPER_ONE_SHOT_CONTROLLER,
                    profile::MIDI_PRESSED_VALUE,
                    false,
                    0,
                ),
                "halfSpeed" => (
                    profile::LOOPER_HALF_SPEED_CONTROLLER,
                    profile::MIDI_PRESSED_VALUE,
                    false,
                    0,
                ),
                "punch" => (
                    profile::LOOPER_PUNCH_CONTROLLER,
                    profile::MIDI_PRESSED_VALUE,
                    false,
                    0,
                ),
                "record" => (
                    profile::LOOPER_RECORD_CONTROLLER,
                    profile::MIDI_PRESSED_VALUE,
                    false,
                    0,
                ),
                "play" => (
                    profile::LOOPER_PLAY_CONTROLLER,
                    profile::MIDI_PRESSED_VALUE,
                    false,
                    0,
                ),
                "reverse" => (
                    profile::LOOPER_REVERSE_CONTROLLER,
                    profile::MIDI_PRESSED_VALUE,
                    false,
                    0,
                ),
                "undoRedo" => (
                    profile::LOOPER_UNDO_REDO_CONTROLLER,
                    profile::MIDI_PRESSED_VALUE,
                    false,
                    0,
                ),
                "duplicateMode" => (profile::LOOPER_DUPLICATE_MODE_CONTROLLER, 0, true, 1),
                "quantize" => (profile::LOOPER_QUANTIZE_CONTROLLER, 0, true, 9),
                "midiClockStart" => (profile::LOOPER_MIDI_CLOCK_START_CONTROLLER, 0, true, 1),
                "performMode" => (profile::LOOPER_PERFORM_MODE_CONTROLLER, 0, true, 1),
                "routingMode" => (profile::LOOPER_ROUTING_MODE_CONTROLLER, 0, true, 13),
                _ => return Err("unsupported Looper X command".into()),
            };
            if !needs_value && supplied.is_some() {
                return Err(format!("{command} does not accept value"));
            }
            let value = if needs_value {
                supplied
                    .and_then(Value::as_u64)
                    .and_then(|value| u8::try_from(value).ok())
                    .filter(|value| *value <= maximum)
                    .ok_or_else(|| format!("{command} requires value 0 through {maximum}"))?
            } else {
                value
            };
            Ok(HostMidiPlan {
                controller,
                value,
                detail: format!("Looper X {command} sent"),
            })
        }
        _ => Err(format!("Gateway method does not use host MIDI: {method}")),
    }
}
