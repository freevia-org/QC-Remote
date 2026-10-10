//! JNI action projection for the retained shared preset owner.
use super::*;
use quadcortex::commands::DeviceOperation;
use quadcortex::device::catalog::CatalogVerificationAction;
use quadcortex::device::owner::OwnedPresetPreparationAction;
use quadcortex::device::preset_workflow::PresetWorkflowAction;
use serde_json::json;

fn wait(delay_ms: u64) -> Value {
    json!({"kind": "wait", "delayMs": delay_ms.min(10)})
}

fn write_action(
    operation: OperationTicket,
    write: PlannedWrite,
    preparation: bool,
) -> Result<Value, String> {
    let interval = write.inter_message_interval_ms();
    let messages = match write {
        PlannedWrite::HidCommand(command) => {
            vec![command.try_encode().map_err(|e| e.to_string())?]
        }
        PlannedWrite::HidOperation(operation) => {
            operation.try_encode().map_err(|e| e.to_string())?
        }
        PlannedWrite::MidiControlChange { .. } => return Err("Preset workflow requires HID".into()),
    };
    Ok(
        json!({"kind":"write", "operation":operation, "messages":messages_json(messages),
        "interMessageIntervalMs": interval, "preparation": preparation, "delayMs": 10}),
    )
}

fn read_action(
    owner: &mut SessionRuntime,
    method: &str,
    params: &Value,
    context: &DispatchContext,
    remaining_ms: u64,
    now_ms: u64,
    preparation: bool,
) -> Result<Value, String> {
    let scope = owner.begin_gateway_read_scope_for(context, now_ms, remaining_ms)?;
    let remaining = owner.gateway_read_scope_remaining(&scope, now_ms)?;
    let operation = if method == "device.listPresetFolders" {
        DeviceOperation::ListPresetFolders
    } else {
        let lifecycle = owner
            .lifecycle
            .as_mut()
            .filter(|session| session.is_connected())
            .ok_or("No connected QC session for workflow read")?;
        plan_gateway_read(method, params, lifecycle.reserve_request_id())?.operation
    };
    let ticket = owner.plan_operation_in_context(scope.context(), now_ms, remaining)?;
    write_action(ticket, PlannedWrite::HidOperation(operation), preparation)
}

fn advance(owner: &mut SessionRuntime, preparation: u64, now_ms: u64) -> Result<Value, String> {
    let workflow = match owner.advance_preset_preparation(preparation, now_ms)? {
        OwnedPresetPreparationAction::Read {
            method,
            params,
            context,
            remaining_ms,
        } => {
            return read_action(
                owner,
                &method,
                &params,
                &context,
                remaining_ms,
                now_ms,
                true,
            );
        }
        OwnedPresetPreparationAction::Wait { until_ms } => {
            return Ok(wait(until_ms.saturating_sub(now_ms)))
        }
        OwnedPresetPreparationAction::Ready { workflow_id } => workflow_id,
    };
    let step = owner.advance_preset_workflow(workflow, now_ms)?;
    if let Some(catalog) = step.catalog {
        if catalog.action == CatalogVerificationAction::Request {
            return Ok(read_action(
                owner,
                "device.listPresetFolders",
                &Value::Null,
                &catalog.context,
                catalog.remaining_ms,
                now_ms,
                false,
            )
            .unwrap_or_else(|_| wait(10)));
        }
    }
    match step.action {
        PresetWorkflowAction::Dispatch { write, .. } => write_action(
            step.operation
                .ok_or("Preset action has no owner operation")?,
            write,
            false,
        ),
        PresetWorkflowAction::Finished(_) => {
            let result = owner
                .preset_workflow_result(workflow)?
                .ok_or("Finished preset workflow has no retained result")?
                .gateway_result();
            Ok(match result {
                Ok(value) => json!({"kind":"finished", "result":value}),
                Err(error) => json!({"kind":"finished", "error":error}),
            })
        }
        PresetWorkflowAction::Wait { until_ms } => Ok(wait(until_ms.saturating_sub(now_ms))),
        PresetWorkflowAction::AwaitReceipt { .. } => Ok(wait(10)),
        PresetWorkflowAction::Verify { .. } | PresetWorkflowAction::VerifyCatalog { .. } => {
            Ok(wait(0))
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeBeginPresetWorkflow(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    method: JString,
    args_json: JString,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let method: String = env.get_string(&method).map_err(|e| e.to_string())?.into();
                let args: Value = serde_json::from_str(
                    &env.get_string(&args_json)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|e| e.to_string())?;
                let native = handle(value)?;
                let mut owner = native.session.lock_recover();
                let id = owner.begin_preset_preparation(&method, args, now_ms.max(0) as u64)?;
                Ok(json!({"preparationId":id}).to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeAdvancePresetWorkflow(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    preparation: jlong,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let mut owner = native.session.lock_recover();
                advance(&mut owner, preparation.max(0) as u64, now_ms.max(0) as u64)
                    .map(|value| value.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeCancelPresetPreparation(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    preparation: jlong,
) {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                native
                    .session
                    .lock_recover()
                    .cancel_preset_preparation(preparation.max(0) as u64);
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                let _ = env.throw_new("java/lang/IllegalStateException", error);
            }
        }),
        &mut env,
        (),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use quadcortex::device::transport::IngressTime;
    use quadcortex::proto::cortex_protobuf_v2 as pa;

    fn owner() -> SessionRuntime {
        let mut owner = SessionRuntime::new(0);
        owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "original".into(), 0));
        owner
    }
    fn folder(owner: &mut SessionRuntime, name: &str, now: u64) {
        folder_action(owner, name, now, pa::message_action::Enum::Create);
    }
    fn folder_action(
        owner: &mut SessionRuntime,
        name: &str,
        now: u64,
        action: pa::message_action::Enum,
    ) {
        let payload = pa::FileMessage {
            action: action as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(format!(
                    "/media/p4/Presets/{name}"
                ))),
                name: Some(pa::folder_info::Name::Name(name.into())),
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let observation = owner
            .capture_observation_with_ingress(
                1,
                Some(IngressTime {
                    first_report_at_ms: now,
                    last_report_at_ms: now,
                }),
            )
            .unwrap();
        owner
            .observe_state(
                &observation,
                quadcortex::profile::MESSAGE_TYPE_FILE,
                &payload,
            )
            .unwrap();
    }
    fn message_action(value: &Value) -> i32 {
        let payload: Vec<u8> =
            serde_json::from_value(value["messages"][0]["payload"].clone()).unwrap();
        assert_eq!(value["messages"].as_array().unwrap().len(), 1);
        assert_eq!(
            value["messages"][0]["messageType"],
            quadcortex::profile::MESSAGE_TYPE_FILE
        );
        pa::FileMessage::decode(payload.as_slice()).unwrap().action
    }
    fn begin_delivery(owner: &mut SessionRuntime, value: &Value, now: u64) -> DeliveryTicket {
        let operation: OperationTicket =
            serde_json::from_value(value["operation"].clone()).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, now).unwrap();
        let delivery = owner
            .prepare_delivery(&dispatch, quadcortex::profile::MESSAGE_TYPE_FILE, 1, now)
            .unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        owner.begin_native_report(&delivery, 0, now).unwrap();
        delivery
    }
    fn fixture(name: &str) -> Value {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../quadcortex/device-test-fixtures/directory-workflows.json"
        ))
        .unwrap();
        fixtures[name].clone()
    }
    fn prepared(fixture: &Value) -> (SessionRuntime, u64, Value) {
        let mut owner = owner();
        owner.state.snapshot_mut().has_preset = true;
        owner.state.snapshot_mut().setlist_key = format!(
            "/media/p4/Presets/{}",
            fixture["activeFolder"].as_str().unwrap()
        );
        for name in fixture["before"].as_array().unwrap() {
            folder(&mut owner, name.as_str().unwrap(), 0);
        }
        let preparation = owner
            .begin_preset_preparation(
                fixture["method"].as_str().unwrap(),
                json!({"name":fixture["name"]}),
                0,
            )
            .unwrap();
        // No second directory response exists in this fixture.
        assert_eq!(advance(&mut owner, preparation, 1).unwrap()["kind"], "wait");
        let write = advance(&mut owner, preparation, 252).unwrap();
        assert_eq!(
            message_action(&write),
            if fixture["method"] == "device.deleteSetlist" {
                pa::message_action::Enum::Delete as i32
            } else {
                pa::message_action::Enum::Create as i32
            }
        );
        assert_eq!(write["preparation"], false);
        (owner, preparation, write)
    }

    fn directory_trace(name: &str) {
        let fixture = fixture(name);
        let (mut owner, preparation, write) = prepared(&fixture);
        let delivery = begin_delivery(&mut owner, &write, 253);
        assert_eq!(
            advance(&mut owner, preparation, 254).unwrap()["kind"],
            "wait"
        );
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 255)
            .unwrap();
        assert_eq!(
            advance(&mut owner, preparation, 255).unwrap()["kind"],
            "wait"
        );
        // Verification is passive first: no directory READ is sent inside the passive
        // window; the unit's own File publication (below) confirms the mutation.
        assert_eq!(
            advance(&mut owner, preparation, 256).unwrap()["kind"],
            "wait"
        );
        for name in fixture["after"].as_array().unwrap() {
            folder(&mut owner, name.as_str().unwrap(), 257);
        }
        for name in fixture["deleted"].as_array().unwrap() {
            folder_action(
                &mut owner,
                name.as_str().unwrap(),
                258,
                pa::message_action::Enum::Delete,
            );
        }
        let finished = advance(&mut owner, preparation, 509).unwrap();
        assert_eq!(finished["kind"], "finished");
        let result = &finished["result"];
        assert_eq!(result["verified"], true);
        assert_eq!(result["setlistKey"], fixture["targetKey"]);
        assert_eq!(result["restoration"]["status"], "not_required");
        assert!(result.get("changed").is_none());
        assert!(result.get("snapshot").is_none());
        quadcortex::device::generated_gateway::validate_result(
            fixture["method"].as_str().unwrap(),
            result,
        )
        .unwrap();
        assert_eq!(advance(&mut owner, preparation, 510).unwrap(), finished);
        let operation: OperationTicket =
            serde_json::from_value(write["operation"].clone()).unwrap();
        assert!(owner.prepare_dispatch(Some(&operation), None, 510).is_err());
    }

    #[test]
    fn android_create_uses_one_owner_write_and_native_catalog_result() {
        directory_trace("create");
    }

    #[test]
    fn android_delete_setlist_uses_the_shared_directory_trace() {
        directory_trace("delete");
    }

    #[test]
    fn android_waits_for_late_native_return_then_reports_unverified_delivery() {
        let (mut owner, preparation, write) = prepared(&fixture("create"));
        let delivery = begin_delivery(&mut owner, &write, 253);
        assert_eq!(
            advance(&mut owner, preparation, 4000).unwrap()["kind"],
            "wait"
        );
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 4001)
            .unwrap();
        let finished = advance(&mut owner, preparation, 4001).unwrap();
        assert_eq!(finished["kind"], "finished");
        let error = finished["error"].as_str().unwrap();
        assert!(error.contains("Persistent USB reports were delivered"));
        assert!(error.contains("after its deadline"));
    }

    #[test]
    fn android_read_projection_retains_original_session_and_deadline() {
        let mut owner = owner();
        let context = owner.session_context().unwrap();
        assert!(read_action(
            &mut owner,
            "device.listPresetFolders",
            &Value::Null,
            &context,
            0,
            10,
            true
        )
        .is_err());
        let read = read_action(
            &mut owner,
            "device.listPresetFolders",
            &Value::Null,
            &context,
            90,
            10,
            true,
        )
        .unwrap();
        let operation: OperationTicket = serde_json::from_value(read["operation"].clone()).unwrap();
        assert_eq!(operation.remaining_ms(10), 90);
        owner.opened(11).unwrap();
        assert!(owner.prepare_dispatch(Some(&operation), None, 11).is_err());
    }
    mod persistent_trace {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../quadcortex/device-test-fixtures/persistent_workflows.rs"
        ));
    }
    fn persistent_trace(copy: bool, cancel_after_admission: bool) {
        let mut owner = owner();
        let mut device = persistent_trace::Device::new(copy, &mut owner);
        let expected_commands = device.command_count();
        let preparation = owner
            .begin_preset_preparation(device.method(), device.params(), 0)
            .unwrap();
        let mut now = 1;
        for _ in 0..2000 {
            // The unit's own File publication, delivered as the clock advances.
            device.tick(&mut owner, now);
            let action = advance(&mut owner, preparation, now).unwrap();
            match action["kind"].as_str().unwrap() {
                "write" => {
                    let operation: OperationTicket =
                        serde_json::from_value(action["operation"].clone()).unwrap();
                    let messages = action["messages"].as_array().unwrap();
                    assert_eq!(messages.len(), 1);
                    let kind = messages[0]["messageType"].as_u64().unwrap() as u16;
                    let payload: Vec<u8> =
                        serde_json::from_value(messages[0]["payload"].clone()).unwrap();
                    let dispatch = owner.prepare_dispatch(Some(&operation), None, now).unwrap();
                    let delivery = owner.prepare_delivery(&dispatch, kind, 1, now).unwrap();
                    owner.delivery_admitted(&delivery).unwrap();
                    owner.begin_native_report(&delivery, 0, now).unwrap();
                    if cancel_after_admission && action["preparation"] == false {
                        owner.cancel_preset_preparation(preparation);
                    }
                    now += 2;
                    device.respond(&mut owner, kind, &payload, now);
                    owner
                        .finish_native_report(&delivery, 0, ReportOutcome::Delivered, now)
                        .unwrap();
                }
                "finished" => {
                    assert!(
                        action.get("error").is_none(),
                        "{}; actions: {:?}",
                        action,
                        device.actions
                    );
                    assert_eq!(device.actions.len(), expected_commands);
                    device.assert_result(&action["result"], &owner);
                    assert_eq!(advance(&mut owner, preparation, now + 1).unwrap(), action);
                    return;
                }
                "wait" => {}
                other => panic!("unexpected action {other}"),
            }
            now += 10;
        }
        panic!("workflow did not terminate: {:?}", device.actions);
    }
    #[test]
    fn android_save_executes_the_shared_persistent_trace() {
        persistent_trace(false, false);
    }
    #[test]
    fn android_copy_sends_one_file_copy_frame_and_verifies_in_the_shared_trace() {
        persistent_trace(true, false);
    }

    #[test]
    fn android_admitted_copy_survives_preparation_cancellation() {
        persistent_trace(true, true);
    }
}
