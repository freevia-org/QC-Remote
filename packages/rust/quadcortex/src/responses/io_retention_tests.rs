use super::*;

#[test]
fn cumulative_headphone_feeds_share_the_software_record_budget() {
    let feeds = |ids: &[u32]| {
        pa::IoSettingsMessage {
            action: 1,
            settings: Some(pa::io_settings_message::Settings::Settings(
                pa::PortSettings {
                    hp_port: Some(pa::port_settings::HpPort::HpPort(pa::HeadphonesSettings {
                        hp_feed: ids
                            .iter()
                            .map(|&output_port_id| pa::HeadphonesFeedLevel {
                                output_port_id,
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec()
    };
    let ids: Vec<_> = (0..MAX_RETAINED_IO_RECORDS as u32 - 1).collect();
    let previous = feeds(&ids);
    validate_io_settings_retention(&previous).unwrap();
    let same = merge_io_settings_publication(&previous, &feeds(&[0])).unwrap();
    validate_io_settings_retention(&same).unwrap();
    assert!(matches!(
        merge_io_settings_publication(&previous, &feeds(&[999])),
        Err(ResponseDecodeError::RetainedIoBudget)
    ));
    assert_eq!(
        previous,
        feeds(&ids),
        "caller-owned old bytes remain unchanged on refusal"
    );
}

#[test]
fn cumulative_io_retention_has_an_encoded_byte_budget_not_only_a_record_budget() {
    let message = pa::IoSettingsMessage {
        action: 1,
        preset_to_load: Some(pa::io_settings_message::PresetToLoad::PresetToLoad(
            pa::ModelPresetId {
                value: Some(pa::model_preset_id::Value::Value(
                    "x".repeat(MAX_RETAINED_IO_BYTES),
                )),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    assert!(matches!(
        check_io_retention(&message),
        Err(ResponseDecodeError::RetainedIoBudget)
    ));
}
