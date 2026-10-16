//! Malformed numeric wire data never becomes JSON null or replaces good I/O evidence.
use super::*;
use crate::device::state_runtime::DeviceStateRuntime;

fn io_message(slot: usize, value: f32, plugged: bool) -> Vec<u8> {
    let mut settings = pa::PortSettings {
        in_port: vec![pa::InputPortSettings {
            input_port_id: 1,
            plugged: Some(pa::input_port_settings::Plugged::Plugged(plugged)),
            ..Default::default()
        }],
        out_port: vec![pa::OutputPortSettings {
            output_port_id: 1,
            ..Default::default()
        }],
        exp_port: vec![pa::ExpPortSettings {
            exp_port_id: 1,
            ..Default::default()
        }],
        hp_port: Some(pa::port_settings::HpPort::HpPort(pa::HeadphonesSettings {
            hp_feed: vec![pa::HeadphonesFeedLevel {
                output_port_id: 1,
                level: 0.5,
            }],
            ..Default::default()
        })),
        usb_port: Some(pa::port_settings::UsbPort::UsbPort(
            pa::UsbPortSettings::default(),
        )),
        midi_port: Some(pa::port_settings::MidiPort::MidiPort(
            pa::MidiPortSettings::default(),
        )),
    };
    match slot {
        0 => settings.in_port[0].level = Some(pa::input_port_settings::Level::Level(value)),
        1 => {
            settings.in_port[0].input_zmode =
                Some(pa::input_port_settings::InputZmode::InputZmode(value))
        }
        2 => {
            settings.in_port[0].input_type =
                Some(pa::input_port_settings::InputType::InputType(value))
        }
        3 => {
            settings.in_port[0].ground_lift =
                Some(pa::input_port_settings::GroundLift::GroundLift(value))
        }
        4 => settings.out_port[0].level = Some(pa::output_port_settings::Level::Level(value)),
        5 => {
            settings.out_port[0].ground_lift =
                Some(pa::output_port_settings::GroundLift::GroundLift(value))
        }
        6 => settings.exp_port[0].level = Some(pa::exp_port_settings::Level::Level(value)),
        7 | 8 => {
            let Some(pa::port_settings::HpPort::HpPort(hp)) = settings.hp_port.as_mut() else {
                unreachable!()
            };
            if slot == 7 {
                hp.level = Some(pa::headphones_settings::Level::Level(value));
            } else {
                hp.hp_feed[0].level = value;
            }
        }
        9..=11 => {
            let Some(pa::port_settings::UsbPort::UsbPort(usb)) = settings.usb_port.as_mut() else {
                unreachable!()
            };
            match slot {
                9 => usb.level = Some(pa::usb_port_settings::Level::Level(value)),
                10 => usb.hp_select = Some(pa::usb_port_settings::HpSelect::HpSelect(value)),
                11 => usb.dry_wet = Some(pa::usb_port_settings::DryWet::DryWet(value)),
                _ => unreachable!(),
            }
        }
        12 => {
            let Some(pa::port_settings::MidiPort::MidiPort(midi)) = settings.midi_port.as_mut()
            else {
                unreachable!()
            };
            midi.midi_thru = Some(pa::midi_port_settings::MidiThru::MidiThru(value));
        }
        _ => unreachable!(),
    }
    pa::IoSettingsMessage {
        action: 1,
        settings: Some(pa::io_settings_message::Settings::Settings(settings)),
        ..Default::default()
    }
    .encode_to_vec()
}

#[test]
fn nonfinite_io_fields_are_refused_without_poisoning_retained_settings_or_snapshot() {
    for slot in 0..13 {
        let good = io_message(slot, 0.5, true);
        assert!(decode_io_settings(&good).is_ok());
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let bad = io_message(slot, invalid, false);
            // Actual protobuf decode accepts the IEEE value; response policy rejects it.
            assert!(pa::IoSettingsMessage::decode(bad.as_slice()).is_ok());
            assert!(decode_io_settings(&bad).is_err(), "slot {slot}");
            assert!(decode_io_settings_echo(&bad).is_err(), "slot {slot}");
            assert!(
                merge_io_settings_publication(&good, &bad).is_err(),
                "slot {slot}"
            );
            let mut runtime = DeviceStateRuntime::new();
            runtime
                .ingest(profile::MESSAGE_TYPE_IO_SETTINGS, &good)
                .unwrap();
            let old_snapshot = serde_json::to_value(runtime.snapshot()).unwrap();
            assert!(runtime
                .ingest(profile::MESSAGE_TYPE_IO_SETTINGS, &bad)
                .is_err());
            assert_eq!(
                runtime.published_settings(profile::MESSAGE_TYPE_IO_SETTINGS),
                Some(good.as_slice())
            );
            assert_eq!(
                serde_json::to_value(runtime.snapshot()).unwrap(),
                old_snapshot
            );
            assert!(
                runtime.assert_io_settings_healthy().is_ok(),
                "numeric corruption is keep-last-good, not sticky resource overflow"
            );
            let mut fresh = DeviceStateRuntime::new();
            assert!(fresh
                .ingest(profile::MESSAGE_TYPE_IO_SETTINGS, &bad)
                .is_err());
            assert!(fresh
                .published_settings(profile::MESSAGE_TYPE_IO_SETTINGS)
                .is_none());
        }
    }
}

#[test]
fn general_settings_storage_nonfinite_values_preserve_last_good_and_absent_fields() {
    use pa::general_settings_message as gs;
    let wire = |slot, value| {
        let mut message = pa::GeneralSettingsMessage {
            action: 1,
            ..Default::default()
        };
        match slot {
            0 => {
                message.available_disk_space =
                    Some(gs::AvailableDiskSpace::AvailableDiskSpace(value))
            }
            1 => message.total_disk_space = Some(gs::TotalDiskSpace::TotalDiskSpace(value)),
            _ => unreachable!(),
        }
        message.encode_to_vec()
    };
    let absent = pa::GeneralSettingsMessage {
        action: 1,
        ..Default::default()
    }
    .encode_to_vec();
    let decoded = decode_general_settings(&absent).unwrap();
    assert_eq!(decoded.available_disk_space, None);
    assert_eq!(decoded.total_disk_space, None);
    for slot in 0..2 {
        for healthy in [0.0, -2.0, f32::MAX] {
            let good = wire(slot, healthy);
            let decoded = decode_general_settings(&good).unwrap();
            let observed = if slot == 0 {
                decoded.available_disk_space
            } else {
                decoded.total_disk_space
            };
            assert_eq!(
                observed,
                Some(healthy),
                "no invented storage firmware range"
            );
        }
        let good = wire(slot, 512.0);
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let bad = wire(slot, invalid);
            assert!(pa::GeneralSettingsMessage::decode(bad.as_slice()).is_ok());
            assert!(decode_general_settings(&bad).is_err());
            assert!(merge_general_settings_publication(&good, &bad).is_err());
            let mut runtime = DeviceStateRuntime::new();
            runtime
                .ingest(profile::MESSAGE_TYPE_GENERAL_SETTINGS, &good)
                .unwrap();
            assert!(runtime
                .ingest(profile::MESSAGE_TYPE_GENERAL_SETTINGS, &bad)
                .is_err());
            assert_eq!(
                runtime.published_settings(profile::MESSAGE_TYPE_GENERAL_SETTINGS),
                Some(good.as_slice())
            );
            let mut fresh = DeviceStateRuntime::new();
            assert!(fresh
                .ingest(profile::MESSAGE_TYPE_GENERAL_SETTINGS, &bad)
                .is_err());
            assert!(fresh
                .published_settings(profile::MESSAGE_TYPE_GENERAL_SETTINGS)
                .is_none());
        }
        let merged = merge_general_settings_publication(&good, &absent).unwrap();
        let decoded = decode_general_settings(&merged).unwrap();
        assert_eq!(
            if slot == 0 {
                decoded.available_disk_space
            } else {
                decoded.total_disk_space
            },
            Some(512.0)
        );
    }
}

#[test]
fn finite_io_levels_that_overflow_public_db_are_refused_without_replacing_evidence() {
    for slot in [0, 4] {
        let good = io_message(slot, 0.5, true);
        for invalid in [f32::MAX, f32::MIN] {
            let bad = io_message(slot, invalid, false);
            assert!(invalid.is_finite());
            assert!(pa::IoSettingsMessage::decode(bad.as_slice()).is_ok());
            assert!(decode_io_settings(&bad).is_err());
            assert!(decode_io_settings_echo(&bad).is_err());
            assert!(merge_io_settings_publication(&good, &bad).is_err());
            let mut runtime = DeviceStateRuntime::new();
            runtime
                .ingest(profile::MESSAGE_TYPE_IO_SETTINGS, &good)
                .unwrap();
            assert!(runtime
                .ingest(profile::MESSAGE_TYPE_IO_SETTINGS, &bad)
                .is_err());
            assert_eq!(
                runtime.published_settings(profile::MESSAGE_TYPE_IO_SETTINGS),
                Some(good.as_slice())
            );
            assert!(runtime.assert_io_settings_healthy().is_ok());
            let mut fresh = DeviceStateRuntime::new();
            assert!(fresh
                .ingest(profile::MESSAGE_TYPE_IO_SETTINGS, &bad)
                .is_err());
            assert!(fresh
                .published_settings(profile::MESSAGE_TYPE_IO_SETTINGS)
                .is_none());
        }
        // Finite extrapolation outside normalized0..1 remains compatible.
        let decoded = decode_io_settings(&io_message(slot, -2.0, true)).unwrap();
        let observed = if slot == 0 {
            decoded.inputs[0].level_db
        } else {
            decoded.outputs[0].level_db
        };
        assert_eq!(observed, Some(if slot == 0 { -156.0 } else { -144.0 }));
    }
}

#[test]
fn global_eq_and_looper_nonfinite_values_never_publish_null_numeric_fields() {
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let eq = pa::GlobalEqMessage {
            action: 1,
            parameters: vec![pa::GlobalEqParameter {
                parameter_index: 6,
                value: invalid,
            }],
            ..Default::default()
        }
        .encode_to_vec();
        assert!(decode_global_eq(&eq).is_err());
        for slot in 0..4 {
            let mut status = pa::LooperStatus::default();
            match slot {
                0 => status.progress = invalid,
                1 => status.undo_progress = invalid,
                2 => status.max_write_displacement = invalid,
                3 => status.min_write_displacement = invalid,
                _ => unreachable!(),
            }
            let payload = pa::LooperMessage {
                action: 1,
                status: Some(pa::looper_message::Status::Status(status)),
                ..Default::default()
            }
            .encode_to_vec();
            assert!(decode_looper_status(&payload).is_err(), "slot {slot}");
        }
    }
    // Do not invent firmware ranges: finite values outside normalized0..1 stay lossless.
    let eq = pa::GlobalEqMessage {
        action: 1,
        parameters: vec![
            pa::GlobalEqParameter {
                parameter_index: 6,
                value: -2.0,
            },
            pa::GlobalEqParameter {
                parameter_index: -1,
                value: f32::NAN,
            },
        ],
        ..Default::default()
    }
    .encode_to_vec();
    assert_eq!(decode_global_eq(&eq).unwrap().parameters[0].value, -2.0);
    let looper = pa::LooperMessage {
        action: 1,
        status: Some(pa::looper_message::Status::Status(pa::LooperStatus {
            progress: 2.0,
            min_write_displacement: -2.0,
            ..Default::default()
        })),
        ..Default::default()
    }
    .encode_to_vec();
    let observed = decode_looper_status(&looper).unwrap();
    assert_eq!(observed.progress, Some(2.0));
    assert_eq!(observed.min_write_displacement, Some(-2.0));
}
