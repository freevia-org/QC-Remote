//! Actual native-frame reduction and delivery-admission regressions; no transport.
use super::*;
use crate::{
    commands::DeviceOperation, device::request::PlannedWrite, proto::cortex_protobuf_v2 as pa,
};
use prost::Message;

fn io(ids: &[u32], plugged: bool) -> Vec<u8> {
    pa::IoSettingsMessage {
        action: 1,
        settings: Some(pa::io_settings_message::Settings::Settings(
            pa::PortSettings {
                in_port: ids
                    .iter()
                    .map(|&input_port_id| pa::InputPortSettings {
                        input_port_id,
                        plugged: Some(pa::input_port_settings::Plugged::Plugged(plugged)),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
    .encode_to_vec()
}

fn owner() -> (SessionRuntime, u64) {
    let mut owner = SessionRuntime::new(0);
    let connection = owner.opened(0).unwrap();
    owner.lifecycle = Some(SessionDriver::start(7, "io-test".into(), 0));
    (owner, connection)
}
fn observe(owner: &mut SessionRuntime, connection: u64, payload: &[u8]) -> Result<(), String> {
    let ticket = owner.capture_observation_with_ingress(
        connection,
        Some(IngressTime {
            first_report_at_ms: 1,
            last_report_at_ms: 1,
        }),
    )?;
    owner
        .observe_state(&ticket, crate::profile::MESSAGE_TYPE_IO_SETTINGS, payload)
        .map(|_| ())
}

#[test]
fn cumulative_native_io_flood_invalidates_once_and_keeps_known_updates_at_capacity() {
    let (mut owner, connection) = owner();
    let cap = crate::responses::MAX_RETAINED_IO_RECORDS;
    for id in 0..cap {
        observe(&mut owner, connection, &io(&[id as u32], true)).unwrap();
        owner.take_state_publications().unwrap();
    }
    assert_eq!(owner.state.snapshot().io_ports.as_ref().unwrap().len(), cap);
    observe(&mut owner, connection, &io(&[1], false)).unwrap();
    assert!(
        !owner
            .state
            .snapshot()
            .io_ports
            .as_ref()
            .unwrap()
            .iter()
            .find(|port| port.id == 1)
            .unwrap()
            .plugged
    );
    owner.take_state_publications().unwrap();
    assert!(observe(&mut owner, connection, &io(&[cap as u32], true)).is_err());
    assert!(owner.state.snapshot().io_ports.is_none());
    assert!(!owner.state.snapshot().io_settings_error.is_empty());
    assert!(owner
        .state
        .published_settings(crate::profile::MESSAGE_TYPE_IO_SETTINGS)
        .is_none());
    let invalidation = owner.take_state_publications().unwrap();
    assert_eq!(invalidation.len(), 1);
    assert_eq!(invalidation[0].states[0].kind, "ioPorts");
    assert!(invalidation[0].states[0].io_settings_error.is_some());
    assert!(observe(&mut owner, connection, &io(&[1], true)).is_err());
    assert!(
        owner.take_state_publications().unwrap().is_empty(),
        "sticky refusal must not flood invalidation events"
    );
    owner.state.reset();
    assert!(owner.state.snapshot().io_settings_error.is_empty());
    observe(&mut owner, connection, &io(&[1], true)).unwrap();
    assert_eq!(owner.state.snapshot().io_ports.as_ref().unwrap().len(), 1);
}

#[test]
fn io_budget_failure_revokes_an_already_planned_write_before_its_first_report() {
    let (mut owner, connection) = owner();
    observe(&mut owner, connection, &io(&[1], true)).unwrap();
    let write = PlannedWrite::HidOperation(DeviceOperation::SetInputPort {
        input_port_id: 1,
        level: Some(0.5),
        impedance: None,
        input_type: None,
        ground_lift: None,
    });
    let context = owner
        .context_for_write("device.setInputPort", &serde_json::json!({}), &write)
        .unwrap();
    let operation = owner.plan_operation_in_context(&context, 1, 100).unwrap();
    let dispatch = owner.prepare_dispatch(Some(&operation), None, 1).unwrap();
    let delivery = owner
        .prepare_delivery(&dispatch, crate::profile::MESSAGE_TYPE_IO_SETTINGS, 1, 1)
        .unwrap();
    let ids: Vec<_> = (0..=crate::responses::MAX_RETAINED_IO_RECORDS as u32).collect();
    assert!(observe(&mut owner, connection, &io(&ids, true)).is_err());
    assert!(owner.begin_native_report(&delivery, 0, 2).is_err());
    assert_eq!(
        owner.delivery_record(&delivery).unwrap().started_at_ms,
        None
    );
    assert!(owner
        .context_for_write("device.setInputPort", &serde_json::json!({}), &write)
        .is_err());
    let eq = PlannedWrite::HidOperation(DeviceOperation::SetGlobalEqBypassed(true));
    assert!(
        owner
            .context_for_write("device.setGlobalEqBypassed", &serde_json::json!({}), &eq)
            .is_ok(),
        "independent EQ state stays usable"
    );
}

#[test]
fn malformed_io_keeps_last_good_but_a_first_over_budget_publication_is_unavailable() {
    let mut runtime = DeviceStateRuntime::new();
    let good = io(&[1], true);
    runtime
        .ingest(crate::profile::MESSAGE_TYPE_IO_SETTINGS, &good)
        .unwrap();
    assert!(runtime
        .ingest(crate::profile::MESSAGE_TYPE_IO_SETTINGS, &[0xff])
        .is_err());
    assert_eq!(
        runtime.published_settings(crate::profile::MESSAGE_TYPE_IO_SETTINGS),
        Some(good.as_slice())
    );
    assert!(runtime.assert_io_settings_healthy().is_ok());
    runtime.reset();
    let ids: Vec<_> = (0..=crate::responses::MAX_RETAINED_IO_RECORDS as u32).collect();
    assert!(runtime
        .ingest(crate::profile::MESSAGE_TYPE_IO_SETTINGS, &io(&ids, true))
        .is_err());
    assert!(runtime
        .published_settings(crate::profile::MESSAGE_TYPE_IO_SETTINGS)
        .is_none());
    assert!(runtime.snapshot().io_ports.is_none());
    assert!(runtime.assert_io_settings_healthy().is_err());
}

#[test]
fn unknown_protobuf_fields_cannot_bypass_the_retained_io_wire_byte_budget() {
    use crate::responses::{
        merge_io_settings_publication, validate_io_settings_retention, ResponseDecodeError,
        MAX_RETAINED_IO_BYTES,
    };
    let good = io(&[1], true);
    let mut oversized = good.clone();
    // Field2047, length-delimited; length1MiB. It is legal unknown wire data,
    // so Prost drops it rather than including it in decoded encoded_len().
    oversized.extend_from_slice(&[0xfa, 0x7f, 0x80, 0x80, 0x40]);
    oversized.resize(oversized.len() + MAX_RETAINED_IO_BYTES, 0);
    let decoded = pa::IoSettingsMessage::decode(oversized.as_slice()).unwrap();
    assert!(decoded.encoded_len() < MAX_RETAINED_IO_BYTES);
    assert!(oversized.len() > MAX_RETAINED_IO_BYTES);
    assert!(matches!(
        validate_io_settings_retention(&oversized),
        Err(ResponseDecodeError::RetainedIoBudget)
    ));
    assert!(matches!(
        merge_io_settings_publication(&good, &oversized),
        Err(ResponseDecodeError::RetainedIoBudget)
    ));
    let (mut owner, connection) = owner();
    assert!(observe(&mut owner, connection, &oversized).is_err());
    assert!(owner
        .state
        .published_settings(crate::profile::MESSAGE_TYPE_IO_SETTINGS)
        .is_none());
    assert!(owner.state.assert_io_settings_healthy().is_err());
    let invalidation = owner.take_state_publications().unwrap();
    assert_eq!(invalidation.len(), 1);
    assert_eq!(invalidation[0].states[0].kind, "ioPorts");
    assert!(invalidation[0].states[0]
        .io_settings_error
        .as_ref()
        .is_some_and(|s| !s.is_empty()));
}
