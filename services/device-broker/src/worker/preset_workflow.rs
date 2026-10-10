//! Windows executes owner actions; only SessionRuntime decides workflow progress.
use super::*;

/// `queue_preset_write`'s error text when the worker's command channel is
/// disconnected (its receiver dropped, meaning the worker thread has died) -
/// as opposed to merely full, which is the ordinary backpressure case this
/// module's dispatch loop deliberately ignores (see its own comment: late
/// native callbacks stay authoritative regardless). Read by
/// `execute_preset_workflow`'s `Dispatch` arm to fail fast instead of
/// polling for the workflow's entire remaining budget - tens of seconds, for
/// `copyPreset` - toward a native receipt that can now never arrive.
const COMMAND_QUEUE_DISCONNECTED: &str = "QC command worker is no longer running";
use quadcortex::device::catalog::CatalogVerificationAction;
use quadcortex::device::owner::OwnedPresetPreparationAction;
use quadcortex::device::preset_workflow::PresetWorkflowAction;
use quadcortex::device::request::PlannedWrite;
use serde_json::Value;

impl DeviceController {
    pub fn execute_preset_workflow(&self, method: &str, params: &Value) -> Result<Value, String> {
        let now = || self.session_clock.elapsed().as_millis() as u64;
        let preparation = self.session_owner.lock_recover().begin_preset_preparation(
            method,
            params.clone(),
            now(),
        )?;
        let workflow = loop {
            let action = self
                .session_owner
                .lock_recover()
                .advance_preset_preparation(preparation, now());
            match action {
                Ok(OwnedPresetPreparationAction::Ready { workflow_id }) => break workflow_id,
                Ok(OwnedPresetPreparationAction::Read {
                    method,
                    params,
                    context,
                    remaining_ms,
                }) => {
                    // Queue only. Native ingress and the owner collect the source and
                    // directory concurrently, within the original preparation budget.
                    if let Err(error) =
                        self.queue_workflow_read(&method, &params, &context, remaining_ms)
                    {
                        self.session_owner
                            .lock_recover()
                            .cancel_preset_preparation(preparation);
                        return Err(error);
                    }
                }
                Ok(OwnedPresetPreparationAction::Wait { until_ms }) => {
                    self.wait_preset_action(until_ms)
                }
                Err(error) => {
                    self.session_owner
                        .lock_recover()
                        .cancel_preset_preparation(preparation);
                    return Err(error);
                }
            }
        };
        loop {
            let step = self
                .session_owner
                .lock_recover()
                .advance_preset_workflow(workflow, now())?;
            if let Some(catalog) = step.catalog {
                if catalog.action == CatalogVerificationAction::Request {
                    let _ = self.queue_workflow_read(
                        "device.listPresetFolders",
                        &Value::Null,
                        &catalog.context,
                        catalog.remaining_ms,
                    );
                }
            }
            match step.action {
                PresetWorkflowAction::Dispatch { write, .. } => {
                    let operation = step
                        .operation
                        .ok_or("Preset dispatch has no owner operation ticket")?;
                    // Do not allocate another ticket or report a host-side success.
                    // A queue-full failure is retired by the owner's original
                    // deadline; late native callbacks remain authoritative
                    // until they return. A disconnected queue is a different
                    // failure mode with no such recovery: the worker thread
                    // that would ever send a native receipt is gone, so
                    // failing immediately here is what actually fails fast -
                    // without this, the caller silently polls
                    // `wait_preset_action` toward the workflow's full
                    // remaining budget (tens of seconds for `copyPreset`)
                    // for a receipt that can now never arrive.
                    if let Err(error) = self.queue_preset_write(operation, write) {
                        if error == COMMAND_QUEUE_DISCONNECTED {
                            return Err(error);
                        }
                    }
                }
                PresetWorkflowAction::Finished(_) => {
                    return self
                        .session_owner
                        .lock_recover()
                        .preset_workflow_result(workflow)?
                        .ok_or("Finished preset workflow has no retained result")?
                        .gateway_result();
                }
                PresetWorkflowAction::Wait { until_ms } => self.wait_preset_action(until_ms),
                PresetWorkflowAction::AwaitReceipt { .. } => {
                    self.wait_preset_action(now().saturating_add(10))
                }
                PresetWorkflowAction::Verify { .. }
                | PresetWorkflowAction::VerifyCatalog { .. } => {}
            }
        }
    }

    fn wait_preset_action(&self, until_ms: u64) {
        let now_ms = self.session_clock.elapsed().as_millis() as u64;
        // Short bounded waits keep session replacement and new ingress observable.
        thread::sleep(Duration::from_millis(
            until_ms.saturating_sub(now_ms).min(10),
        ));
    }

    fn queue_workflow_read(
        &self,
        method: &str,
        params: &Value,
        context: &DispatchContext,
        remaining_ms: u64,
    ) -> Result<(), String> {
        let scope = self.begin_gateway_read_scope_for(context, remaining_ms)?;
        let operation = if method == "device.listPresetFolders" {
            DeviceOperation::ListPresetFolders
        } else {
            let request_id = self.reserve_request_id_in_scope(&scope)?;
            runtime_request::plan_gateway_read(method, params, request_id)?.operation
        };
        let ticket = {
            let mut owner = self.session_owner.lock_recover();
            let now_ms = self.session_clock.elapsed().as_millis() as u64;
            let remaining = owner.gateway_read_scope_remaining(&scope, now_ms)?;
            owner.plan_operation_in_context(scope.context(), now_ms, remaining)?
        };
        self.queue_preset_write(ticket, PlannedWrite::HidOperation(operation))
    }

    fn queue_preset_write(
        &self,
        admission: OperationTicket,
        write: PlannedWrite,
    ) -> Result<(), String> {
        let (messages, interval_ms) = match write {
            PlannedWrite::HidCommand(command) => {
                (vec![command.try_encode().map_err(|e| e.to_string())?], 0)
            }
            PlannedWrite::HidOperation(operation) => {
                let interval = runtime_request::operation_inter_message_interval_ms(&operation);
                (operation.try_encode().map_err(|e| e.to_string())?, interval)
            }
            PlannedWrite::MidiControlChange { .. } => {
                return Err("Preset workflow requires HID".into())
            }
        };
        let (reply, _admission_only) = mpsc::channel();
        self.commands
            .try_send(Command::SendSequence {
                admission,
                messages,
                delay: Duration::ZERO,
                interval: Duration::from_millis(interval_ms),
                release_on_failure: None,
                hold: None,
                reply,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => error.to_string(),
                mpsc::TrySendError::Disconnected(_) => COMMAND_QUEUE_DISCONNECTED.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quadcortex::device::delivery::ReportOutcome;
    use quadcortex::device::transport::IngressTime;

    fn publish_folder(owner: &mut SessionRuntime, clock: Instant, name: &str) {
        publish_folder_action(owner, clock, name, pa::message_action::Enum::Create);
    }
    fn publish_folder_action(
        owner: &mut SessionRuntime,
        clock: Instant,
        name: &str,
        action: pa::message_action::Enum,
    ) {
        let now = clock.elapsed().as_millis() as u64;
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
        let ticket = owner
            .capture_observation_with_ingress(
                1, // refresh_controller opens its single test connection once.
                Some(IngressTime {
                    first_report_at_ms: now,
                    last_report_at_ms: now,
                }),
            )
            .unwrap();
        owner
            .observe_state(&ticket, profile::MESSAGE_TYPE_FILE, &payload)
            .unwrap();
    }

    fn directory_trace(late_native_return: bool, deletion: bool) {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../../../packages/rust/quadcortex/device-test-fixtures/directory-workflows.json"
        ))
        .unwrap();
        let fixture = fixtures[if deletion { "delete" } else { "create" }].clone();
        let (controller, receiver) = super::super::tests::refresh_controller();
        let owner = Arc::clone(&controller.session_owner);
        {
            let mut runtime = owner.lock_recover();
            runtime.state.snapshot_mut().has_preset = true;
            runtime.state.snapshot_mut().setlist_key = format!(
                "/media/p4/Presets/{}",
                fixture["activeFolder"].as_str().unwrap()
            );
            for folder in fixture["before"].as_array().unwrap() {
                publish_folder(
                    &mut runtime,
                    controller.session_clock,
                    folder.as_str().unwrap(),
                );
            }
        }
        let worker_fixture = fixture.clone();
        let clock = controller.session_clock;
        let worker = thread::spawn(move || {
            let mut persistent_ticket = None;
            // Startup has already supplied the catalog; only the mutation is
            // sent. Verification is passive first, so the unit's own publication
            // confirms it and no directory READ follows.
            for index in 0..1 {
                let Command::SendSequence {
                    admission,
                    messages,
                    delay,
                    interval,
                    ..
                } = receiver.recv_timeout(Duration::from_secs(2)).unwrap()
                else {
                    panic!("workflow must queue owner frames, not a host-local stage")
                };
                assert_eq!(messages.len(), 1);
                assert_eq!(delay, Duration::ZERO);
                assert_eq!(interval, Duration::ZERO);
                let message = &messages[0];
                let file = pa::FileMessage::decode(message.payload.as_slice()).unwrap();
                assert_eq!(
                    file.action,
                    if deletion {
                        pa::message_action::Enum::Delete as i32
                    } else {
                        pa::message_action::Enum::Create as i32
                    }
                );
                {
                    let mut runtime = owner.lock_recover();
                    let now = clock.elapsed().as_millis() as u64;
                    assert!(admission.remaining_ms(now) > 0);
                    let dispatch = runtime
                        .prepare_dispatch(Some(&admission), None, now)
                        .unwrap();
                    let delivery = runtime
                        .prepare_delivery(&dispatch, message.message_type, 1, now)
                        .unwrap();
                    runtime.delivery_admitted(&delivery).unwrap();
                    runtime.begin_native_report(&delivery, 0, now).unwrap();
                    drop(runtime);
                    if index == 0 && late_native_return {
                        let remaining = admission.remaining_ms(clock.elapsed().as_millis() as u64);
                        assert!(
                            receiver
                                .recv_timeout(Duration::from_millis(remaining + 30))
                                .is_err(),
                            "an expired native call must return before any following action"
                        );
                        assert_eq!(
                            admission.remaining_ms(clock.elapsed().as_millis() as u64),
                            0
                        );
                    }
                    owner
                        .lock_recover()
                        .finish_native_report(
                            &delivery,
                            0,
                            ReportOutcome::Delivered,
                            clock.elapsed().as_millis() as u64,
                        )
                        .unwrap();
                    if index == 0 {
                        persistent_ticket = Some(admission);
                    }
                }
                if index == 0 && !late_native_return {
                    // The unit's File publication after the frame.
                    thread::sleep(Duration::from_millis(40));
                    let mut runtime = owner.lock_recover();
                    for folder in worker_fixture["after"].as_array().unwrap() {
                        publish_folder(&mut runtime, clock, folder.as_str().unwrap());
                    }
                    if deletion {
                        for folder in worker_fixture["deleted"].as_array().unwrap() {
                            publish_folder_action(
                                &mut runtime,
                                clock,
                                folder.as_str().unwrap(),
                                pa::message_action::Enum::Delete,
                            );
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(300));
            assert!(
                receiver.try_recv().is_err(),
                "persistent CREATE must not replay"
            );
            assert!(
                owner
                    .lock_recover()
                    .prepare_dispatch(
                        persistent_ticket.as_ref(),
                        None,
                        clock.elapsed().as_millis() as u64
                    )
                    .is_err(),
                "retired workflow ticket cannot send again"
            );
        });
        let result = controller.execute_preset_workflow(
            fixture["method"].as_str().unwrap(),
            &serde_json::json!({"name": fixture["name"]}),
        );
        if late_native_return {
            let error = result.unwrap_err();
            assert!(error.contains("Persistent USB reports were delivered"));
            assert!(error.contains("after its deadline"));
            worker.join().unwrap();
            return;
        }
        let result = result.unwrap();
        assert_eq!(result["accepted"], true);
        assert_eq!(result["verified"], true);
        assert_eq!(result["setlistKey"], fixture["targetKey"]);
        assert_eq!(result["restoration"]["status"], "not_required");
        assert!(result.get("changed").is_none());
        assert!(result.get("snapshot").is_none());
        quadcortex::device::generated_gateway::validate_result(
            fixture["method"].as_str().unwrap(),
            &result,
        )
        .unwrap();
        assert_eq!(
            controller
                .preset_folders()
                .iter()
                .any(|f| f.name == fixture["name"].as_str().unwrap()),
            !deletion
        );
        worker.join().unwrap();
    }

    #[test]
    fn windows_create_uses_one_owner_write_and_native_catalog_result() {
        directory_trace(false, false);
    }

    #[test]
    fn windows_waits_for_late_native_return_then_reports_unverified_delivery() {
        directory_trace(true, false);
    }

    #[test]
    fn windows_delete_setlist_uses_the_shared_directory_trace() {
        directory_trace(false, true);
    }

    #[test]
    fn a_dead_command_worker_fails_the_workflow_immediately_instead_of_polling_to_the_deadline() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../../../packages/rust/quadcortex/device-test-fixtures/directory-workflows.json"
        ))
        .unwrap();
        let fixture = fixtures["create"].clone();
        let (controller, receiver) = super::super::tests::refresh_controller();
        {
            let mut runtime = controller.session_owner.lock_recover();
            runtime.state.snapshot_mut().has_preset = true;
            runtime.state.snapshot_mut().setlist_key = format!(
                "/media/p4/Presets/{}",
                fixture["activeFolder"].as_str().unwrap()
            );
            for folder in fixture["before"].as_array().unwrap() {
                publish_folder(
                    &mut runtime,
                    controller.session_clock,
                    folder.as_str().unwrap(),
                );
            }
        }
        // No worker thread is spawned to drain `receiver`; dropping it here
        // reproduces a dead command worker - the receiver end is gone, so
        // every subsequent `try_send` on `controller.commands` observes
        // `Disconnected` rather than `Full`.
        drop(receiver);
        let started = std::time::Instant::now();
        let result = controller.execute_preset_workflow(
            fixture["method"].as_str().unwrap(),
            &serde_json::json!({"name": fixture["name"]}),
        );
        let error = result.unwrap_err();
        assert_eq!(error, COMMAND_QUEUE_DISCONNECTED);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a disconnected command worker must fail fast, not poll toward the workflow deadline"
        );
    }

    #[test]
    fn workflow_reads_keep_the_original_deadline_and_session() {
        let (controller, receiver) = super::super::tests::refresh_controller();
        let context = controller
            .session_owner
            .lock_recover()
            .session_context()
            .unwrap();
        assert!(controller
            .queue_workflow_read("device.listPresetFolders", &Value::Null, &context, 0)
            .is_err());
        assert!(receiver.try_recv().is_err());
        controller
            .queue_workflow_read("device.listPresetFolders", &Value::Null, &context, 100)
            .unwrap();
        let Command::SendSequence { admission, .. } = receiver.try_recv().unwrap() else {
            panic!("expected read");
        };
        let now = controller.session_clock.elapsed().as_millis() as u64;
        assert!(admission.remaining_ms(now) <= 100);
        let mut owner = controller.session_owner.lock_recover();
        owner.opened(now).unwrap();
        assert!(owner.prepare_dispatch(Some(&admission), None, now).is_err());
    }
    mod persistent_trace {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/rust/quadcortex/device-test-fixtures/persistent_workflows.rs"
        ));
    }
    fn persistent_trace(copy: bool) {
        let (controller, receiver) = super::super::tests::refresh_controller();
        let owner = Arc::clone(&controller.session_owner);
        let mut device = persistent_trace::Device::new(copy, &mut owner.lock_recover());
        let method = device.method();
        let params = device.params();
        let clock = controller.session_clock;
        let worker = thread::spawn(move || {
            for _ in 0..device.command_count() {
                let Command::SendSequence {
                    admission,
                    messages,
                    ..
                } = receiver.recv_timeout(Duration::from_secs(5)).unwrap()
                else {
                    panic!("expected one owner-issued native command");
                };
                assert_eq!(messages.len(), 1);
                let message = &messages[0];
                let mut runtime = owner.lock_recover();
                let now = clock.elapsed().as_millis() as u64;
                let dispatch = runtime
                    .prepare_dispatch(Some(&admission), None, now)
                    .unwrap();
                let delivery = runtime
                    .prepare_delivery(&dispatch, message.message_type, 1, now)
                    .unwrap();
                runtime.delivery_admitted(&delivery).unwrap();
                runtime.begin_native_report(&delivery, 0, now).unwrap();
                drop(runtime);
                thread::sleep(Duration::from_millis(2));
                let mut runtime = owner.lock_recover();
                let now = clock.elapsed().as_millis() as u64;
                device.respond(&mut runtime, message.message_type, &message.payload, now);
                runtime
                    .finish_native_report(&delivery, 0, ReportOutcome::Delivered, now)
                    .unwrap();
            }
            // The unit publishes its catalog on its own; the host sends no READ.
            thread::sleep(Duration::from_millis(60));
            device.tick(
                &mut owner.lock_recover(),
                clock.elapsed().as_millis() as u64,
            );
            thread::sleep(Duration::from_millis(100));
            assert!(
                receiver.try_recv().is_err(),
                "completed workflow issued another command"
            );
            device
        });
        let result = controller.execute_preset_workflow(method, &params).unwrap();
        let device = worker.join().unwrap();
        device.assert_result(&result, &controller.session_owner.lock_recover());
    }
    #[test]
    fn windows_save_executes_the_shared_persistent_trace() {
        persistent_trace(false);
    }
    #[test]
    fn windows_copy_sends_one_file_copy_frame_and_verifies_in_the_shared_trace() {
        persistent_trace(true);
    }
}
