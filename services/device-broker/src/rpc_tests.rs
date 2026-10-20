#![cfg(test)]

use super::*;

#[cfg(target_os = "linux")]
#[test]
fn startup_failure_releases_only_the_claim_it_published() {
    use std::os::unix::fs::DirBuilderExt;
    let directory =
        std::env::temp_dir().join(format!("qc-publish-failure-{}", uuid::Uuid::new_v4()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap();
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _scratch = Scratch(directory.clone());
    let path = directory.join("device-broker.json");
    let fail_after_publication = || -> Result<(), String> {
        let claim = qc_owner_only_file::ClaimedFile::try_claim(&path).unwrap();
        let slot = Arc::new(Mutex::new(Some(claim)));
        let _cleanup = ServingLifetime::new(Arc::new(AtomicBool::new(false)), Arc::clone(&slot));
        slot.lock_recover()
            .as_mut()
            .unwrap()
            .write(b"complete document")
            .unwrap();
        assert!(qc_owner_only_file::release_if_unclaimed(&path).is_err());
        Err("injected startup failure after publication".into())
    };
    let outcome = fail_after_publication();
    assert!(outcome.is_err());
    assert!(!path.exists());
    assert!(!qc_owner_only_file::claim_path(&path).exists());
    let mut successor = qc_owner_only_file::ClaimedFile::try_claim(&path).unwrap();
    successor.write(b"successor").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"successor");
    successor.release().unwrap();
}

#[test]
fn reconnect_waits_only_when_the_requested_qc_is_present() {
    let devices = vec![quadcortex_hid::usb::QcUsbDevice {
        id: "qc-a".into(),
        name: "Quad Cortex".into(),
    }];
    assert!(!requested_device_is_present(None, &[]));
    assert!(requested_device_is_present(None, &devices));
    assert!(requested_device_is_present(Some("qc-a"), &devices));
    assert!(!requested_device_is_present(Some("qc-b"), &devices));
}

#[test]
fn a_reconnect_for_an_absent_qc_returns_searching_without_waiting_for_a_handshake() {
    // Always exercise absence, including on hosts where HID cannot initialize.
    // The caller witness distinguishes this RPC's discovery from its worker's.
    let caller = std::thread::current().id();
    let rpc_discoveries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = Arc::clone(&rpc_discoveries);
    let root = crate::device_lease::testing::fresh_root("absent-ready");
    let dir = root.0.clone();
    // An id that matches nothing HID can enumerate, so this test can never
    // open a real Quad Cortex, and a lease of its own so it never contends
    // with a running broker.
    let controller = DeviceController::start_for_device_with_test_discovery(
        Some("no-such-device".into()),
        true,
        dir.join("device-owner.json"),
        move || {
            if std::thread::current().id() == caller {
                observed.fetch_add(1, Ordering::SeqCst);
            }
            Ok(Vec::new())
        },
    )
    .expect("controller starts");
    struct RootOwner(
        crate::worker::WorkerShutdown,
        Arc<crate::device_lease::testing::FixtureRoot>,
    );
    impl Drop for RootOwner {
        fn drop(&mut self) {
            if !matches!(
                self.0.stop_and_join(crate::worker::WORKER_EXIT_BUDGET),
                crate::worker::WorkerExit::Joined | crate::worker::WorkerExit::NoWorker
            ) {
                static RETAINED: Mutex<Vec<Arc<crate::device_lease::testing::FixtureRoot>>> =
                    Mutex::new(Vec::new());
                RETAINED.lock_recover().push(Arc::clone(&self.1));
                eprintln!("absent-ready worker exit unacknowledged; exact root retained");
            }
        }
    }
    let root_owner = RootOwner(controller.shutdown_handle(), Arc::clone(&root));
    assert_eq!(controller.status().phase, "searching");
    let started = Instant::now();
    let status = await_ready_status(&controller);
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_millis(profile::READY_WAIT_TIMEOUT_MS / 2),
        "an absent QC must not cost the ready wait; took {waited:?}"
    );
    assert_eq!(status.phase, "searching");
    assert!(!status.connected);
    assert!(!status.synchronized);
    assert!(rpc_discoveries.load(Ordering::SeqCst) > 0);
    drop(controller);
    drop(root_owner);
}

#[test]
fn cached_snapshot_preserves_incomplete_grid_without_requesting_a_refresh() {
    let snapshot = quadcortex::device::GatewaySnapshot {
        setlist_key: "test-setlist".into(),
        preset_name: "test-preset".into(),
        grid_layout_complete: false,
        ..Default::default()
    };
    let value = serialize_gateway_snapshot(snapshot).expect("serialize cached snapshot");
    assert_eq!(value["gridLayoutComplete"], false);
    assert!(matches!(
        generated_gateway::broker_dispatch("device.snapshot"),
        Some(generated_gateway::BrokerDispatch::Snapshot)
    ));
    assert!(matches!(
        generated_gateway::broker_dispatch("device.currentPreset"),
        Some(generated_gateway::BrokerDispatch::CurrentPreset)
    ));
}

#[test]
fn incomplete_current_preset_refresh_requires_new_full_same_session_publication() {
    let mut snapshot = quadcortex::device::GatewaySnapshot {
        grid_layout_complete: true,
        full_preset_revision: 7,
        state_version: Some(quadcortex::generated_payloads::NativeStateVersion {
            session: "session-a".into(),
            revision: 90,
        }),
        ..Default::default()
    };
    assert!(!current_preset_refresh_matches(&snapshot, 7, "session-a"));
    snapshot.full_preset_revision = 8;
    assert!(current_preset_refresh_matches(&snapshot, 7, "session-a"));
    assert!(!current_preset_refresh_matches(&snapshot, 7, "session-b"));
    snapshot.grid_layout_complete = false;
    assert!(!current_preset_refresh_matches(&snapshot, 7, "session-a"));
}

#[test]
fn backup_name_is_checked_before_the_export_is_dispatched() {
    let payload = r#"{"type":"backup","creator":"quad","payload":"AQID","payload_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}"#;
    let accepted_name = "x".repeat(runtime_request::BACKUP_NAME_MAX_CHARACTERS);
    let mut accepted_dispatches = 0;
    let accepted = gateway_create_backup_with(&json!({"name": accepted_name}), || {
        accepted_dispatches += 1;
        Ok(payload.to_string())
    })
    .expect("64 visible characters are accepted");
    assert_eq!(accepted["name"], accepted_name);
    assert_eq!(accepted_dispatches, 1);

    for invalid_name in ["x".repeat(65), "bad\nname".to_string()] {
        let mut invalid_dispatches = 0;
        let result = gateway_create_backup_with(&json!({"name": invalid_name}), || {
            invalid_dispatches += 1;
            Ok(payload.to_string())
        });
        assert!(result.is_err());
        assert_eq!(invalid_dispatches, 0, "invalid names must not start export");
    }
}

#[test]
fn phase_events_publish_each_visible_status_change() {
    let controller = DeviceController::start_disconnected().expect("controller starts");
    let output = Mutex::new(Vec::<u8>::new());
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        struct StopOnDrop<'a>(&'a AtomicBool);
        impl Drop for StopOnDrop<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let _stop_on_unwind = StopOnDrop(&stop);
        let publisher = scope.spawn(|| publish_phase_changes(&controller, &output, &stop));
        // Several polls against an unchanged phase still yield one frame.
        thread::sleep(PHASE_EVENT_POLL * 3);
        // Startup detail changes within one phase are visible progress too.
        controller.hold_status_lock_for_test().detail = "Handshake attempt 2".into();
        thread::sleep(PHASE_EVENT_POLL * 3);
        stop.store(true, Ordering::Release);
        publisher
            .join()
            .expect("phase publisher exits when stopped");
    });
    let bytes = output.into_inner().expect("output lock");
    let mut frames = Vec::new();
    let mut rest = bytes.as_slice();
    while rest.len() >= 4 {
        let length = u32::from_be_bytes(rest[..4].try_into().expect("header")) as usize;
        frames.push(serde_json::from_slice::<Value>(&rest[4..4 + length]).expect("frame"));
        rest = &rest[4 + length..];
    }
    assert!(rest.is_empty(), "every published byte belongs to a frame");
    assert_eq!(
        frames.len(),
        2,
        "an unchanged status is deduplicated but changed progress is published"
    );
    assert_eq!(frames[0]["method"], json!("device.phase"));
    assert_eq!(frames[0]["params"]["phase"], json!("disconnected"));
    assert_eq!(frames[0]["params"]["connected"], json!(false));
    assert_eq!(frames[1]["params"]["phase"], json!("disconnected"));
    assert_eq!(frames[1]["params"]["detail"], json!("Handshake attempt 2"));
    assert!(
        frames[0].get("id").is_none(),
        "a notification never claims a request id"
    );
}

#[test]
fn history_rpc_preserves_caller_identity_through_native_admission() {
    let controller = DeviceController::start_disconnected().expect("controller starts");
    let midi = PerformanceMidiWorkers::default();
    for method in ["device.undo", "device.redo"] {
        let response = handle(
            &controller,
            &midi,
            Request {
                jsonrpc: "2.0".into(),
                id: json!(1),
                method: method.into(),
                params: json!({"expectedPresetIdentity": "previous-session"}),
                origin: None,
                physical_only: false,
                expected_broker_target: None,
            },
        );
        assert_eq!(
            response["error"]["message"],
            "The preset or QC session changed. Refresh the device state before retrying",
            "{method} must check the caller identity before dispatching history"
        );
    }
}

#[test]
fn block_projection_refuses_incomplete_grid_and_checks_caller_identity() {
    let controller = DeviceController::start_disconnected().expect("controller starts");
    for column in [8, 9] {
        let error = state_block_details(&controller, &json!({"row": 0, "column": column}))
            .expect_err("an unsynchronized Grid cannot be projected as an empty cell");
        assert!(error.contains("preset"), "{error}");
    }
    assert!(state_block_details(&controller, &json!({"row": 0, "column": 10})).is_err());
    assert!(
        state_block_details(
            &controller,
            &json!({
                "row": 0, "column": 8, "expectedPresetIdentity": "previous-session"
            })
        )
        .is_err(),
        "a missing current preset cannot satisfy another session's identity"
    );
}

#[test]
fn a_reservation_timeout_gets_its_own_code_and_is_marked_retryable() {
    // Preserve the legacy broker protocol without classifying current
    // scope expiry as an invitation to replay the caller's operation.
    assert_eq!(
        classify_handler_error(RESERVE_REQUEST_ID_TIMEOUT_MESSAGE),
        (RESERVE_REQUEST_ID_TIMEOUT_ERROR_CODE, true)
    );
    // An unrelated handler failure keeps the existing generic code and is
    // not claimed to be retryable.
    assert_eq!(
        classify_handler_error("Quad Cortex is not connected"),
        (HANDLER_ERROR_CODE, false)
    );
    for message in [
        "QC request budget expired before request ID reservation",
        "QC request budget expired while reserving a request ID",
        "QC logical read budget expired",
        "QC command budget expired before native admission",
    ] {
        assert_eq!(classify_handler_error(message), (HANDLER_ERROR_CODE, false));
    }
}

/// `reconnect_device` is contracted `risky-write` and needs
/// `confirm_risky_operation`. A caller who paid that price and is told
/// "Quad Cortex handshake complete" has every reason to believe the
/// transport was reconnected; while the device is already connected the
/// broker deliberately short-circuits and nothing is touched. The result
/// has to carry that difference.
#[test]
fn a_short_circuited_reconnect_does_not_report_a_handshake_it_never_performed() {
    assert_eq!(
        reconnect_report(
            ReconnectOutcome::Restarted,
            "Quad Cortex handshake complete"
        ),
        ("Quad Cortex handshake complete", true)
    );

    let (detail, restarted) = reconnect_report(
        ReconnectOutcome::AlreadyConnected,
        "Quad Cortex handshake complete",
    );
    assert!(!restarted);
    assert_ne!(detail, "Quad Cortex handshake complete");
    assert!(
        detail.contains("reset_device_session"),
        "the honest result must name the forcing path: {detail}"
    );

    // The same distinction has to survive into the result the caller
    // actually receives, on a session that is ready either way.
    let ready = BrokerStatus {
        phase: "ready".into(),
        detail: "Quad Cortex ready".into(),
        connected: true,
        connected_at_unix_ms: Some(17),
        ..BrokerStatus::default()
    };
    let performed = reconnect_state_json(
        &ready,
        ReconnectOutcome::Restarted,
        "Quad Cortex handshake complete",
    );
    assert_eq!(performed["restarted"], json!(true));
    assert_eq!(performed["detail"], json!("Quad Cortex handshake complete"));

    let untouched = reconnect_state_json(
        &ready,
        ReconnectOutcome::AlreadyConnected,
        "Quad Cortex handshake complete",
    );
    assert_eq!(untouched["restarted"], json!(false));
    assert_ne!(untouched["detail"], json!("Quad Cortex handshake complete"));
    // The live session is still reported as ready: what changed is the
    // claim that this call is why.
    assert_eq!(untouched["phase"], json!("ready"));
    assert_eq!(untouched["lastSync"], json!(17));
}

#[test]
fn an_explicit_catalog_refresh_fails_closed_and_names_the_session_rebuild() {
    // The 45-second listPresets hang and the 15-second listPresetFolders
    // hang were both this flag waiting for a readFolders reply the QC never
    // sends. Refusing in under a millisecond is the whole fix: the caller
    // learns the one mechanism that does re-pull the directory instead of
    // watching a timer run out and then being told the device is at fault.
    assert!(preset_catalog_refresh_requested(&json!({"refresh": true})));
    assert!(!preset_catalog_refresh_requested(
        &json!({"refresh": false})
    ));
    assert!(!preset_catalog_refresh_requested(&json!({})));
    assert!(!preset_catalog_refresh_requested(
        &json!({"refresh": "yes"})
    ));

    for message in [
        PRESET_CATALOG_REFRESH_UNAVAILABLE,
        PRESET_CATALOG_UNPUBLISHED,
    ] {
        assert!(
            message.contains("reconnect the device session")
                || message.contains("Reconnect the device session"),
            "{message:?} must name the one mechanism that re-pulls the directory"
        );
    }
    assert!(
        PRESET_CATALOG_REFRESH_UNAVAILABLE.contains("retained directory"),
        "the refusal has to say what is served instead of a refresh"
    );
    assert!(
            !PRESET_CATALOG_REFRESH_UNAVAILABLE.contains("does not answer")
                && !PRESET_CATALOG_REFRESH_UNAVAILABLE.contains("did not answer")
                && !PRESET_CATALOG_UNPUBLISHED.contains("in time"),
            "neither message may claim the unit cannot answer a directory read or blame a timeout it no longer waits for"
        );
}

#[test]
fn an_unobserved_dirty_flag_refusal_is_retryable_and_quarantines_nothing() {
    let (controller, _commands) = DeviceController::live_session_for_test();
    let message = format!(
        "{}The Quad Cortex has not yet reported whether the current preset has unsaved changes.",
        runtime_request::UNSAVED_STATE_NOT_YET_OBSERVED_PREFIX
    );
    let response = failure_response(&controller, json!(6), "device.recallPreset", true, &message);
    let error = &response["error"];
    assert_eq!(error["code"], HANDLER_ERROR_CODE);
    assert_eq!(
        error["message"],
        "The Quad Cortex has not yet reported whether the current preset has unsaved changes."
    );
    assert_eq!(error["data"]["code"], "UNSAVED_STATE_NOT_YET_OBSERVED");
    assert_eq!(error["data"]["retryable"], true);
    assert_eq!(error["data"]["userActionRequired"], false);
    assert!(controller.write_quarantine().is_none());
}

#[test]
fn an_unpublished_catalog_keeps_its_code_and_message_and_carries_why_in_data() {
    let (controller, _commands) = DeviceController::live_session_for_test();
    let why = controller
        .wait_for_preset_catalog(
            "device.listPresets",
            Some("/media/p4/Presets/My Presets"),
            Duration::from_millis(30),
        )
        .unwrap()
        .unwrap_err();
    let message = catalog_unpublished_error(PRESET_CATALOG_UNPUBLISHED, &why);
    let response = failure_response(&controller, json!(7), "device.listPresets", false, &message);
    let error = &response["error"];
    // Existing callers and pins match on these two; they must not move.
    assert_eq!(error["code"], HANDLER_ERROR_CODE);
    assert_eq!(error["message"], PRESET_CATALOG_UNPUBLISHED);
    assert_eq!(error["data"]["code"], "PRESET_CATALOG_UNPUBLISHED");
    assert_eq!(error["data"]["retryable"], false);
    // Every absence echoes the key it looked up.
    assert_eq!(error["data"]["setlistKey"], "/media/p4/Presets/My Presets");
    assert!(error["data"]["knownSetlists"].is_null());
    let catalog = &error["data"]["catalog"];
    assert_eq!(catalog["reason"], "noCatalogFrame");
    assert_eq!(catalog["setlistKey"], "/media/p4/Presets/My Presets");
    assert_eq!(catalog["setlistKeyUnknown"], false);
    assert_eq!(catalog["method"], "device.listPresets");
    assert_eq!(catalog["timeoutMs"], 30);
    assert!(catalog["waitedMs"].as_u64().unwrap() >= 30);
    for field in [
        "fileFramesReceived",
        "fileFramesObserved",
        "fileFramesDiscardedByRebuild",
        "sessionEpoch",
        "sessionRevision",
        "connectionFramesReceived",
    ] {
        assert!(catalog[field].is_u64(), "{field} missing from {catalog}");
    }
    assert!(!response.to_string().contains(CATALOG_DIAGNOSTICS_MARKER));
    // Any other handler failure is untouched.
    let other = failure_response(&controller, json!(8), "device.listPresets", false, "boom");
    assert_eq!(other["error"]["message"], "boom");
    assert!(other["error"].get("data").is_none());
}

#[test]
fn list_presets_refuses_an_unknown_setlist_key_at_once_and_echoes_it() {
    // Git Bash rewrote `/media/p4/Presets/My Presets` into
    // this key, and listPresets waited its whole 15 s budget before
    // blaming an unpublished catalog that was in fact available.
    let mangled = "C:/Program Files/Git/media/p4/Presets/My Presets";
    let (controller, _commands) = DeviceController::connected_catalog_session_for_test(&[
        "/media/p4/Presets/Other",
        "/media/p4/Presets/Live",
        "/media/p4/Presets/Studio",
    ]);
    let started = Instant::now();
    let message = gateway_list_presets(&controller, &json!({"setlistKey": mangled}))
        .expect_err("an unknown setlist key is refused");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(profile::PRESET_SYNC_TIMEOUT_MS) / 10,
        "refused after {elapsed:?}, budget {} ms",
        profile::PRESET_SYNC_TIMEOUT_MS
    );
    let response = failure_response(&controller, json!(9), "device.listPresets", false, &message);
    let error = &response["error"];
    // The same handler code as every other refusal of a target the device
    // does not hold; only the message and `data.code` name the case.
    assert_eq!(error["code"], HANDLER_ERROR_CODE);
    assert_eq!(
        error["message"],
        format!("The setlist \"{mangled}\" does not exist on this device.")
    );
    assert_eq!(error["data"]["code"], "PRESET_SETLIST_UNKNOWN");
    assert_eq!(error["data"]["retryable"], false);
    assert_eq!(error["data"]["setlistKey"], mangled);
    assert_eq!(error["data"]["knownSetlists"], 3);
    assert_eq!(error["data"]["catalog"]["reason"], "setlistKeyMissing");
    assert_eq!(error["data"]["catalog"]["setlistKeyListed"], false);
    assert_eq!(error["data"]["catalog"]["startupComplete"], true);
    let text = response.to_string();
    assert!(!text.contains(CATALOG_DIAGNOSTICS_MARKER));
    for name in ["Other", "Live", "Studio"] {
        assert!(!text.contains(name), "folder name {name} leaked: {text}");
    }
    // A known key on the same session is served.
    let listed = gateway_list_presets(
        &controller,
        &json!({"setlistKey": "/media/p4/Presets/Live"}),
    )
    .unwrap();
    assert_eq!(listed["setlistKey"], "/media/p4/Presets/Live");
    assert_eq!(listed["loading"], false);
}

#[test]
fn the_capture_catalog_refusal_names_the_session_rebuild_and_blames_no_timeout() {
    // A `device.captures` read that happens to return successfully is
    // catching a directory republication the device produces on its own
    // in-session rebuilds, not a reply correlated to the read itself. On a
    // quiet session there is nothing to catch, so the honest failure names
    // the one mechanism that does publish a directory rather than
    // reporting a slow device.
    assert!(
        CAPTURE_CATALOG_UNPUBLISHED.contains("Reconnect the device session"),
        "the refusal must name the one mechanism that re-pulls the directory"
    );
    assert!(
        CAPTURE_CATALOG_UNPUBLISHED.contains("capture"),
        "a caller must be able to tell which catalog is missing"
    );
    assert!(
        !CAPTURE_CATALOG_UNPUBLISHED.contains("in time")
            && !CAPTURE_CATALOG_UNPUBLISHED.contains("second"),
        "the read no longer waits on a device reply, so it cannot blame a timeout"
    );
    // The preset and capture catalogs fail separately; one message standing
    // in for the other would send the caller to the wrong listing.
    assert_ne!(CAPTURE_CATALOG_UNPUBLISHED, PRESET_CATALOG_UNPUBLISHED);
    assert_eq!(
        runtime_request::plan_retained_library_read("device.captures", &Value::Null)
            .unwrap()
            .key,
        "local_nc_root"
    );
}

#[test]
fn the_io_settings_refusal_names_the_session_rebuild_and_blames_no_timeout() {
    assert_eq!(
        generated_gateway::broker_dispatch("device.ioSettings"),
        Some(generated_gateway::BrokerDispatch::IoSettings),
        "I/O settings must not regress to an on-demand GatewayRead"
    );
    assert!(
        IO_SETTINGS_UNPUBLISHED.contains("Reconnect the device session"),
        "the refusal must name the mechanism that republishes type 3"
    );
    assert!(IO_SETTINGS_UNPUBLISHED.contains("I/O settings"));
    assert!(
        !IO_SETTINGS_UNPUBLISHED.contains("in time") && !IO_SETTINGS_UNPUBLISHED.contains("second"),
        "the cached read does not wait on a device reply"
    );
}

#[test]
fn cached_setting_publications_share_one_exact_method_projection() {
    assert_eq!(
        published_settings("device.generalSettings"),
        Some((
            profile::MESSAGE_TYPE_GENERAL_SETTINGS,
            GatewayResponseProjection::GeneralSettings
        ))
    );
    assert_eq!(
        published_settings("device.ioSettings"),
        Some((
            profile::MESSAGE_TYPE_IO_SETTINGS,
            GatewayResponseProjection::IoSettings
        ))
    );
    assert_eq!(published_settings("device.captures"), None);
}

#[test]
fn the_general_settings_refusal_names_the_session_rebuild_and_blames_no_timeout() {
    assert_eq!(
        generated_gateway::broker_dispatch("device.generalSettings"),
        Some(generated_gateway::BrokerDispatch::GeneralSettings),
        "General Settings must not regress to an on-demand GatewayRead"
    );
    assert!(GENERAL_SETTINGS_UNPUBLISHED.contains("Reconnect the device session"));
    assert!(GENERAL_SETTINGS_UNPUBLISHED.contains("General Settings"));
    assert!(
        !GENERAL_SETTINGS_UNPUBLISHED.contains("in time")
            && !GENERAL_SETTINGS_UNPUBLISHED.contains("second"),
        "the cached read does not wait on a device reply"
    );
}

#[test]
fn recall_navigate_and_reload_send_the_exact_desktop_frame_without_a_request_id() {
    for (position, expected) in [
        (8_u32, &b"\x08\x01\x1a\x0a/user/live\x20\x08\x28\x00"[..]),
        (16_u32, &b"\x08\x01\x1a\x0a/user/live\x20\x10\x28\x00"[..]),
    ] {
        let plan = runtime_request::PresetRecallPlan {
            command: quadcortex::commands::DeviceCommand::SetlistPosition {
                setlist_key: "/user/live".into(),
                position,
                is_factory: false,
            },
            setlist_key: "/user/live".into(),
            position,
            after_position_revision: 14,
            after_full_preset_revision: 14,
            require_clean: position == 8,
            detail: "test".into(),
        };
        let message = preset_recall_message(&plan).unwrap();
        assert_eq!(message.message_type, 2);
        assert_eq!(message.payload, expected);
    }
}

#[test]
fn a_read_never_tears_down_the_session_on_its_callers_behalf() {
    // `device.resetSession` is contracted `risky-write` requiring
    // `confirm_risky_operation` because a teardown fails every in-flight
    // request, voids every `expected_*` value the caller holds, and resets
    // the unit's screen in front of the player. A read routed a recoverable
    // timeout straight into `controller.reset_session()` and then returned
    // `Ok(value)` with nothing saying so — and `get_current_preset` and
    // `get_block_details`, the two reads this project's own guide opens
    // every session with, were both in scope.
    //
    // Disclosure after the fact cannot repair that: the in-flight requests
    // have already failed and the expected values are already void by the
    // time any field could say so. The consent had to come first, so the
    // read refuses and names the confirmed operation instead.
    let timeout = "The Quad Cortex did not return a valid device.irs reply within 30 seconds";
    assert!(gateway_read_timeout_permits_same_session_retry(
        "device.irs",
        timeout
    ));

    let refusal = unresponsive_read_session_refusal(timeout);
    assert!(
        refusal.starts_with(timeout),
        "the refusal must keep the original transport failure: {refusal}"
    );
    assert!(
        refusal.contains("reset_device_session"),
        "the refusal must name the one confirmed operation that restarts the session: {refusal}"
    );
    assert!(
        refusal.contains("screen"),
        "the caller has to be told a teardown resets the unit's screen: {refusal}"
    );
    assert!(
        !refusal.contains("was restarted") && !refusal.contains("Read recovery"),
        "a read that restarted nothing must not imply it did: {refusal}"
    );
}

#[test]
fn only_safe_timed_out_reads_trigger_session_recovery() {
    let timeout = "The Quad Cortex did not return a valid device.irs reply within 30 seconds";
    assert!(gateway_read_timeout_permits_same_session_retry(
        "device.irs",
        timeout
    ));
    assert!(gateway_read_timeout_permits_same_session_retry(
        "device.presetScreenshot",
        "No correlated QC message type 25 response for request 42 before timeout"
    ));
    assert!(!gateway_read_timeout_permits_same_session_retry(
        "device.diagnostics",
        "The Quad Cortex did not return a valid device.diagnostics reply within 8 seconds"
    ));
    assert!(!gateway_read_timeout_permits_same_session_retry(
        "device.diagnostics",
        "No correlated QC message type 27 response for request 42 before timeout"
    ));
    assert!(!gateway_read_timeout_permits_same_session_retry(
        "device.looperStatus",
        "The Quad Cortex did not return a valid device.looperStatus reply within 5 seconds"
    ));
    assert!(!gateway_read_timeout_permits_same_session_retry(
        "device.irs",
        "folder must not contain path traversal"
    ));
}

#[test]
fn preset_recall_results_satisfy_their_own_generated_contract() {
    // Regression: recallPreset, navigateBank and reloadPreset are contracted
    // as DeviceActionResult but returned only {detail, snapshot}. The host
    // validates every result and reported the mismatch as a transport
    // fault, so it killed a healthy broker on every preset change. Nothing
    // exercised validate_result against a real broker result, so the whole
    // parity gate stayed green while the device session died on each click.
    // The shape the broker returns since the address-only result: accepted_unverified with
    // an address observation and the mirror attached.
    let recall = json!({
        "accepted": true,
        "verified": false,
        "verification": "accepted_unverified",
        "detail": "Preset change sent; the host observed a position update for /user/live position 9, which does not prove this request caused it, and no preset body can be bound to that address, so the loaded preset's contents are not verified",
        "observation": {"kind": "preset_address_observed", "setlistKey": "/user/live", "position": 9,
            "address": "observed", "body": "unbound", "causallyCorrelated": false},
        "snapshot": {"presetName": "Test", "blocks": []}
    });
    for method in [
        "device.recallPreset",
        "device.navigateBank",
        "device.reloadPreset",
    ] {
        generated_gateway::validate_result(method, &recall)
            .unwrap_or_else(|error| panic!("{method} result must satisfy its contract: {error}"));
    }

    // The shape that caused the outage must still be rejected, so this can
    // never silently regress to "detail plus snapshot".
    let without_verification = json!({
        "detail": "Preset recall requested",
        "snapshot": {"presetName": "Test", "blocks": []}
    });
    for method in [
        "device.recallPreset",
        "device.navigateBank",
        "device.reloadPreset",
    ] {
        assert!(
            generated_gateway::validate_result(method, &without_verification).is_err(),
            "{method} must not accept a result without verification semantics"
        );
    }
}

#[test]
fn a_recall_with_only_an_observed_address_is_not_reported_as_an_authoritative_readback() {
    // Preset A ("Preset A", model 41) was at slot 8 and the recall targeted slot 9. The
    // mirror below is what the predicate matched when the destination's SetlistPosition was
    // followed by A's old body. execute_preset_recall used to return it as
    // verified:true, authoritative_readback; this pins the honest result built from the same
    // inputs, for all three methods that share it.
    let mut after = quadcortex::device::GatewaySnapshot {
        setlist_key: "/user/live".into(),
        preset_name: "Preset A".into(),
        grid_layout_complete: true,
        position_revision: 15,
        full_preset_revision: 15,
        ..Default::default()
    };
    for (method, position, require_clean) in [
        ("device.recallPreset", 9_u32, false),
        ("device.navigateBank", 16, false),
        ("device.reloadPreset", 8, true),
    ] {
        after.preset_position = position;
        let plan = runtime_request::PresetRecallPlan {
            command: quadcortex::commands::DeviceCommand::SetlistPosition {
                setlist_key: "/user/live".into(),
                position,
                is_factory: false,
            },
            setlist_key: "/user/live".into(),
            position,
            after_position_revision: 14,
            after_full_preset_revision: 14,
            require_clean,
            detail: "test".into(),
        };
        assert!(
            plan.matches(&after),
            "{method}: the observed-address predicate holds"
        );
        let result = preset_recall_address_result(&plan, &after).unwrap();
        assert!(!result_is_verified(&result), "{method}: {result}");
        assert_eq!(result["verification"], "accepted_unverified", "{method}");
        assert!(result.get("changed").is_none(), "{method}");
        assert_eq!(
            result["observation"]["kind"],
            runtime_request::PRESET_ADDRESS_OBSERVED,
            "{method}"
        );
        assert_eq!(result["observation"]["position"], position, "{method}");
        assert_eq!(result["observation"]["body"], "unbound", "{method}");
        // The caller still gets the mirror it navigates by, at the observed address.
        assert_eq!(result["snapshot"]["presetPosition"], position, "{method}");
        assert_eq!(result["snapshot"]["setlistKey"], "/user/live", "{method}");
        generated_gateway::validate_result(method, &result)
            .unwrap_or_else(|error| panic!("{method} result must satisfy its contract: {error}"));
    }
}

#[test]
fn a_recall_timeout_says_what_was_not_observed_and_claims_nothing() {
    // Before this, a recall that timed out read "Preset recalled and verified; the QC pushed
    // no confirming position and preset within N ms, so the effect is not verified".
    let mut after = quadcortex::device::GatewaySnapshot {
        setlist_key: "/user/live".into(),
        position_revision: 15,
        full_preset_revision: 15,
        ..Default::default()
    };
    for (method, position, require_clean, detail) in [
        (
            "device.recallPreset",
            9_u32,
            false,
            "Preset recall requested",
        ),
        (
            "device.navigateBank",
            16,
            false,
            "Preset bank change requested",
        ),
        (
            "device.reloadPreset",
            8,
            true,
            "Active preset address resent",
        ),
    ] {
        let plan = runtime_request::PresetRecallPlan {
            command: quadcortex::commands::DeviceCommand::SetlistPosition {
                setlist_key: "/user/live".into(),
                position,
                is_factory: false,
            },
            setlist_key: "/user/live".into(),
            position,
            after_position_revision: 14,
            after_full_preset_revision: 14,
            require_clean,
            detail: detail.into(),
        };
        let timeout = preset_recall_timeout_result(&plan, 2500);
        assert!(!result_is_verified(&timeout), "{method}");
        assert_eq!(timeout["verification"], "accepted_unverified", "{method}");
        let text = timeout["detail"].as_str().unwrap();
        assert!(text.starts_with(detail), "{method}: {text}");
        assert!(text.contains("the frame was sent"), "{method}: {text}");
        assert!(
            text.contains(&format!("/user/live position {position}")),
            "{method}: {text}"
        );
        assert!(text.contains("2500 ms"), "{method}: {text}");
        assert!(
            !runtime_request::detail_claims_verification(text),
            "{method}: {text}"
        );
        generated_gateway::validate_result(method, &timeout).unwrap();

        after.preset_position = position;
        let observed = preset_recall_address_result(&plan, &after).unwrap();
        let observed_text = observed["detail"].as_str().unwrap();
        assert!(
            !runtime_request::detail_claims_verification(observed_text),
            "{method}: {observed_text}"
        );
    }
}

#[test]
fn a_recall_timeout_result_is_true_for_every_partial_observation() {
    let plan =
        |position: u32, require_clean: bool, detail: &str| runtime_request::PresetRecallPlan {
            command: quadcortex::commands::DeviceCommand::SetlistPosition {
                setlist_key: "/user/live".into(),
                position,
                is_factory: false,
            },
            setlist_key: "/user/live".into(),
            position,
            after_position_revision: 14,
            after_full_preset_revision: 14,
            require_clean,
            detail: detail.into(),
        };
    let recall = plan(9, false, "Preset recall requested");
    let reload = plan(8, true, "Active preset address resent");
    let held = |position: u32, position_revision: u64, full_preset_revision: u64, dirty: bool| {
        quadcortex::device::GatewaySnapshot {
            setlist_key: "/user/live".into(),
            preset_position: position,
            position_revision,
            full_preset_revision,
            dirty,
            ..Default::default()
        }
    };
    for (label, method, plan, evidence) in [
        (
            "(a) newer position, old body",
            "device.recallPreset",
            &recall,
            held(9, 15, 14, false),
        ),
        (
            "(b) newer body, no newer position",
            "device.recallPreset",
            &recall,
            held(8, 14, 15, false),
        ),
        (
            "(c) reload: position and body, clean flag missing",
            "device.reloadPreset",
            &reload,
            held(8, 15, 15, true),
        ),
        (
            "(d) nothing arrived",
            "device.recallPreset",
            &recall,
            held(8, 14, 14, false),
        ),
    ] {
        let policy = runtime_request::gateway_write_verification_policy(method);
        let mut runtime = runtime_request::GatewayVerificationRuntime::new(
            plan.verification(),
            5,
            1_000,
            &policy,
        );
        assert!(
            matches!(
                runtime.advance(Some(&evidence), None, 6, 1_001),
                runtime_request::GatewayVerificationAction::Wait { .. }
            ),
            "{label}"
        );
        assert_eq!(
            runtime.advance(Some(&evidence), None, 6, 1_000 + policy.timeout_ms),
            runtime_request::GatewayVerificationAction::TimedOut,
            "{label}"
        );
        let result = preset_recall_timeout_result(plan, policy.timeout_ms);
        assert!(!result_is_verified(&result), "{label}");
        let detail = result["detail"].as_str().unwrap();
        assert!(
            !runtime_request::detail_claims_verification(detail),
            "{label}: {detail}"
        );
        assert!(
            detail.contains("did not observe all of the required conditions together"),
            "{label}: {detail}"
        );
        assert!(
            detail.contains("a newer full preset publication"),
            "{label}: {detail}"
        );
        assert_eq!(
            detail.contains("a clean flag"),
            plan.require_clean,
            "{label}: {detail}"
        );
        for asserted_absence in [
            "no position update",
            "no newer",
            "no full preset",
            "no clean flag",
        ] {
            assert!(!detail.contains(asserted_absence), "{label}: {detail}");
        }
        generated_gateway::validate_result(method, &result).unwrap();
    }
}

#[test]
fn both_observation_kinds_carry_exactly_their_contract_variant_fields() {
    // The payload schema closes each DeviceActionResult.observation
    // variant (additionalProperties false, discriminated by kind). validate_result does not
    // read observation, so this pins what the broker emits to the declared field sets and to
    // the accepted_unverified outcome both variants require.
    fn keys(observation: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = observation
            .as_object()
            .expect("observation is an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }
    let settings = matching_settings_observation_result("setGeneralToggle sent");
    assert_eq!(
        keys(&settings["observation"]),
        ["causallyCorrelated", "kind"]
    );
    assert_eq!(
        settings["observation"]["kind"],
        "matching_publication_after_native_admission"
    );
    generated_gateway::validate_result("device.setGeneralToggle", &settings).unwrap();

    let mut after = quadcortex::device::GatewaySnapshot {
        setlist_key: "/user/live".into(),
        position_revision: 15,
        full_preset_revision: 15,
        ..Default::default()
    };
    for (method, position, require_clean, expected) in [
        (
            "device.recallPreset",
            9_u32,
            false,
            &[
                "address",
                "body",
                "causallyCorrelated",
                "kind",
                "position",
                "setlistKey",
            ][..],
        ),
        (
            "device.reloadPreset",
            8,
            true,
            &[
                "address",
                "body",
                "causallyCorrelated",
                "dirty",
                "kind",
                "position",
                "setlistKey",
            ][..],
        ),
    ] {
        after.preset_position = position;
        let plan = runtime_request::PresetRecallPlan {
            command: quadcortex::commands::DeviceCommand::SetlistPosition {
                setlist_key: "/user/live".into(),
                position,
                is_factory: false,
            },
            setlist_key: "/user/live".into(),
            position,
            after_position_revision: 14,
            after_full_preset_revision: 14,
            require_clean,
            detail: "test".into(),
        };
        let result = preset_recall_address_result(&plan, &after).unwrap();
        let observation = &result["observation"];
        assert_eq!(keys(observation), expected, "{method}");
        assert_eq!(observation["kind"], "preset_address_observed", "{method}");
        assert_eq!(observation["address"], "observed", "{method}");
        assert_eq!(observation["body"], "unbound", "{method}");
        assert_eq!(observation["causallyCorrelated"], false, "{method}");
        assert!(observation["setlistKey"].is_string(), "{method}");
        assert!(
            observation["position"]
                .as_u64()
                .is_some_and(|value| value <= 255),
            "{method}"
        );
        assert!(
            observation.get("dirty").is_none_or(Value::is_boolean),
            "{method}"
        );
        assert_eq!(result["verification"], "accepted_unverified", "{method}");
        assert!(result.get("changed").is_none() && result.get("readback").is_none());
        generated_gateway::validate_result(method, &result).unwrap();
    }
}

#[test]
fn a_plugin_add_that_reads_back_bypassed_carries_a_neutral_note_and_stays_contract_valid() {
    // The host no longer refuses plugin models and does not establish
    // ownership. gateway_operation reports the device's readback as fact
    // on the verified result it returns.
    let write = quadcortex::device::request::PlannedWrite::HidOperation(
        quadcortex::commands::DeviceOperation::AddBlock {
            row: 0,
            column: 1,
            model_id: 5024,
        },
    );
    let snapshot = |bypassed: bool| {
        json!({"presetName": "Clean", "blocks": [
            {"id": "added", "modelId": 5024, "name": "Plugin", "kind": "Model",
             "plugin": true, "pluginId": "13", "row": 0, "column": 1, "bypassed": bypassed}
        ]})
    };
    let mut locked = verified_readback_result(
        "addBlock sent to the Quad Cortex",
        Some(true),
        "snapshot",
        snapshot(true),
    );
    runtime_request::annotate_plugin_block_readback(&write, &mut locked);
    assert_eq!(
        locked["pluginBlockReadback"],
        json!({
            "plugin": true,
            "bypassed": true,
            "bypassRequested": false,
            "note": runtime_request::PLUGIN_BLOCK_BYPASSED_NOTE
        })
    );
    assert!(locked["detail"]
        .as_str()
        .unwrap()
        .ends_with(runtime_request::PLUGIN_BLOCK_BYPASSED_NOTE));
    assert_eq!(locked["verified"], true, "the add is reported, not refused");
    generated_gateway::validate_result("device.addBlock", &locked)
        .expect("an annotated add_block result satisfies its contract");

    let mut active = verified_readback_result(
        "addBlock sent to the Quad Cortex",
        Some(true),
        "snapshot",
        snapshot(false),
    );
    runtime_request::annotate_plugin_block_readback(&write, &mut active);
    assert_eq!(
        active["pluginBlockReadback"],
        json!({"plugin": true, "bypassed": false, "bypassRequested": false})
    );
    assert_eq!(active["detail"], "addBlock sent to the Quad Cortex");
    generated_gateway::validate_result("device.addBlock", &active).unwrap();
}

/// One realistic post-write readback: Dual Delay SYNC NOTE L, where 0.6 is
/// "1/8D" and the previous value 0.5 was "1/4".
fn sync_note_readback(normalized: f64, display: &str) -> BlockDetails {
    serde_json::from_value(json!({
        "row": 1,
        "column": 3,
        "modelId": 4001,
        "name": "Dual Delay",
        "category": "Delay",
        "scene": 0,
        "parameters": [{
            "index": 7,
            "displayPosition": 3,
            "name": "SYNC NOTE L",
            "normalizedValue": normalized,
            "displayValue": display,
            "units": "",
            "type": "rotarySwitch",
            "minimum": 0.0,
            "maximum": 1.0,
            "valueScale": "linear",
            "showAsInteger": false,
            "hasOffPosition": false,
            "floorDisplay": null,
            "floorNormalized": null,
            "scalePoints": [],
            "scaleKnown": true,
            "sceneMode": false,
            "options": ["1/4", "1/8D", "1/8"],
            "writable": true,
            "enabled": true,
            "expressionAssignable": true,
            "wireValueKind": "text"
        }]
    }))
    .expect("block details fixture")
}

#[test]
fn a_parameter_write_reports_one_coherent_block_readback() {
    // Regression: the block returned by set_parameter was the *pre-write*
    // view with only normalizedValue patched onto it, so Dual Delay
    // SYNC NOTE L came back as normalizedValue 0.6 beside displayValue
    // "1/4" — the previous value, which catches up only on the next call.
    // One response describing two device states makes a landed write look
    // like it failed, and anyone reading the human-facing field is told
    // something untrue.
    let after = sync_note_readback(0.6, "1/8D");
    let mut result = json!({
        "accepted": true,
        "verified": true,
        "verification": "authoritative_readback",
        "detail": "Set SYNC NOTE L",
    });
    attach_parameter_block(&mut result, after.clone()).expect("attach block");

    let parameter = &result["block"]["parameters"][0];
    assert_eq!(parameter["normalizedValue"], json!(0.6));
    assert_eq!(
        parameter["displayValue"],
        json!("1/8D"),
        "displayValue must describe the same state as normalizedValue"
    );

    // Both fields must come from one readback. Asserting the whole block is
    // the readback verbatim is what stops the old patch — or any new one —
    // from reintroducing a half-updated parameter.
    assert_eq!(
        result["block"],
        serde_json::to_value(&after).expect("readback json"),
        "the reported block must be the readback, unmodified"
    );

    // Attaching the block must not disturb the write's own outcome fields.
    assert_eq!(result["verification"], json!("authoritative_readback"));
    assert_eq!(result["verified"], json!(true));
}

#[test]
fn an_unpublished_guard_fails_closed_without_asking_the_device() {
    // Cortex Control reads nothing before a write. A guard whose state the
    // session has not been given fails closed with a message that names the
    // read the CALLER may make; the host sends none.
    let controller = DeviceController::start_disconnected().expect("controller starts");
    for (method, hint) in [
        ("device.setFavorite", "list_favorites"),
        ("device.setModelPinned", "list_pinned_models"),
        ("device.setTempoMode", "type 33"),
        ("device.setGlobalTempo", "type 33"),
        ("device.selectScene", "published a preset"),
    ] {
        let error = preflight_observation(&controller, method).unwrap_err();
        assert!(error.contains(hint), "{method}: {error}");
        assert!(error.contains("nothing was sent"), "{method}: {error}");
        assert!(
            !error_has_ambiguous_write_outcome(&error),
            "{method}: a refusal before any frame must never quarantine: {error}"
        );
    }
}

#[test]
fn only_a_verified_result_carries_a_post_write_block() {
    assert!(result_is_verified(&json!({"verified": true})));
    assert!(!result_is_verified(&json!({"verified": false})));
    assert!(!result_is_verified(&accepted_unverified("sent")));
}

#[test]
fn a_preflight_reads_a_source_that_can_actually_answer_its_decision() {
    // Regression: device.selectScene preflighted with device.currentPreset,
    // whose reply projects PresetTempoSettings and carries neither
    // presetName nor activeScene. Every select_scene therefore failed closed
    // with "authoritative scene preflight did not contain a presetName", and
    // scene selection was unavailable over MCP.
    //
    // This slipped through because the two halves were asserted apart: one
    // test pinned the preflight *method name*, another fed the decision a
    // hand-built response that the real method never produces. Nothing tied
    // the source to what the decision reads. That is the gap this closes.
    for method in [
        "device.selectScene",
        "device.setTempoMode",
        "device.setGlobalTempo",
        "device.setFavorite",
        "device.setModelPinned",
    ] {
        runtime_request::gateway_write_preflight_method(method)
            .unwrap_or_else(|| panic!("{method} has no preflight source"));
        // Every preflight consumes decoded state; none sends a READ, so there is
        // no read plan whose projection could fail to answer its decision.
        assert_eq!(
            runtime_request::gateway_write_preflight_observation_source(method),
            Some(runtime_request::GatewayPreflightObservationSource::DecodedSnapshot),
            "{method} must preflight against decoded state"
        );
    }

    // The tempo guards read a decoded source too. Their projection
    // is NOT the GatewaySnapshot: `mode` there is the footswitch mode,
    // whose domain contains "PRESET" too, so reusing it would report a
    // PRESET tempo-mode write as already satisfied and would make
    // setGlobalTempo - which demands "GLOBAL" - permanently unsatisfiable.
    let footswitch_preset = json!({"mode": "PRESET", "globalBpm": 120});
    assert_eq!(
        runtime_request::gateway_write_preflight_decision(
            "device.setTempoMode",
            &json!({"mode": "GLOBAL"}),
            &footswitch_preset,
        ),
        Ok(runtime_request::GatewayWritePreflightDecision::Proceed),
        "a PRESET tempo mode must not satisfy a GLOBAL request"
    );
    let tempo_observation = json!({"mode": "GLOBAL", "globalBpm": 120});
    for field in ["mode", "globalBpm"] {
        assert!(
            tempo_observation.get(field).is_some(),
            "the tempo preflight observation must carry {field}"
        );
    }
    assert_eq!(
        runtime_request::gateway_write_preflight_decision(
            "device.setTempoMode",
            &json!({"mode": "GLOBAL"}),
            &tempo_observation,
        ),
        Ok(runtime_request::GatewayWritePreflightDecision::AlreadyMatched)
    );

    // The scene preflight reads the decoded snapshot, so that snapshot must
    // carry the fields the decision inspects. Proving it against a real
    // GatewaySnapshot is the link the original tests never made.
    let snapshot = serde_json::to_value(quadcortex::device::GatewaySnapshot::default())
        .expect("snapshot json");
    for field in ["presetName", "activeScene"] {
        assert!(
            snapshot.get(field).is_some(),
            "the decoded snapshot must carry {field} for the scene preflight"
        );
    }

    // And end to end: a snapshot sitting on the expected preset and scene
    // suppresses the write instead of failing it.
    let mut settled = snapshot;
    settled["presetName"] = json!("Numb");
    settled["activeScene"] = json!(0);
    assert_eq!(
        runtime_request::gateway_write_preflight_decision(
            "device.selectScene",
            &json!({"scene": 0, "expectedPresetName": "Numb"}),
            &settled,
        ),
        Ok(runtime_request::GatewayWritePreflightDecision::AlreadyMatched)
    );
}

#[test]
fn coros_preset_name_deduplication_is_accepted_without_accepting_unrelated_names() {
    assert!(runtime_request::stored_preset_name_matches(
        "Crying Wah",
        "Crying Wah"
    ));
    assert!(runtime_request::stored_preset_name_matches(
        "Crying Wah",
        "Crying Wah_1"
    ));
    assert!(runtime_request::stored_preset_name_matches(
        "Cali Basswalk [Ret1]",
        "Cali Basswalk [Ret_1"
    ));
    assert!(!runtime_request::stored_preset_name_matches(
        "Crying Wah",
        "Crying Wah copy"
    ));
    assert!(!runtime_request::stored_preset_name_matches(
        "Crying Wah",
        "Other_1"
    ));
    assert!(!runtime_request::stored_preset_name_matches(
        "Crying Wah",
        "Crying Wah_x"
    ));
    assert!(!runtime_request::stored_preset_name_matches(
        "Crying Wah Plus",
        "Crying Wah_1"
    ));
}

#[test]
fn optional_event_and_raw_request_numbers_are_strictly_typed_and_bounded() {
    assert_eq!(optional_u64(&json!({}), "afterSequence"), Ok(None));
    assert_eq!(
        optional_u64(&json!({"afterSequence": 7}), "afterSequence"),
        Ok(Some(7))
    );
    for invalid in [json!(null), json!(-1), json!(1.5), json!("7"), json!(true)] {
        assert!(optional_u64(&json!({"afterSequence": invalid}), "afterSequence").is_err());
    }
    assert!(optional_bounded_u64(&json!({"limit": 0}), "limit", 1, 4096).is_err());
    assert!(optional_bounded_u64(&json!({"limit": 4097}), "limit", 1, 4096).is_err());
    assert_eq!(
        optional_bounded_u64(&json!({"timeoutMs": 60_000}), "timeoutMs", 1, 60_000),
        Ok(Some(60_000))
    );
}

#[test]
fn master_volume_read_fails_closed_without_a_ready_device_session() {
    let controller = DeviceController::start_disconnected().expect("controller starts");
    assert_eq!(
        gateway_master_volume(&controller),
        Err("Quad Cortex USB is not connected".to_string())
    );
}

#[test]
fn list_models_fails_closed_without_an_installed_startup_catalog() {
    let controller = DeviceController::start_disconnected().expect("controller starts");
    assert_eq!(
        gateway_list_models(&controller),
        Err("The Quad Cortex model catalog has not been synchronized yet".to_string())
    );
}

// A device backup as the unit reports it: the screen lock engages, then
// the suspension starts.
fn suspended_for_backup() -> (DeviceController, crate::worker::TestCommands) {
    let (controller, commands) = DeviceController::live_session_for_test();
    controller.observe_device_frame_for_test(
        profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        &[0x08, 0x01, 0x30, 0x01],
    );
    controller.observe_device_frame_for_test(
        profile::MESSAGE_TYPE_SUSPEND_CONNECTION,
        &[0x10, 0x01, 0x18, 0x00],
    );
    (controller, commands)
}

fn call(controller: &DeviceController, method: &str, params: Value) -> Value {
    let performance_midi = PerformanceMidiWorkers::default();
    handle(
        controller,
        &performance_midi,
        Request {
            jsonrpc: "2.0".into(),
            id: json!(7),
            method: method.into(),
            params,
            origin: None,
            physical_only: false,
            expected_broker_target: None,
        },
    )
}

fn assert_device_suspended(response: &Value) {
    assert_eq!(
        response["error"]["code"], DEVICE_SUSPENDED_ERROR_CODE,
        "{response}"
    );
    let data = &response["error"]["data"];
    assert_eq!(data["code"], "DEVICE_SUSPENDED");
    assert_eq!(data["retryable"], false);
    assert_eq!(data["requestSent"], false);
    assert_eq!(data["suspension"]["reason"], "BACKUP");
    let message = response["error"]["message"].as_str().unwrap();
    assert!(message.starts_with(DEVICE_SUSPENDED_MARKER), "{message}");
    assert!(message.contains("BACKUP in progress"), "{message}");
}

#[test]
fn a_suspended_device_refuses_writes_with_a_typed_error_and_queues_nothing() {
    let (controller, commands) = suspended_for_backup();
    for (method, params) in [
        (
            "device.setTempo",
            json!({
                "bpm": 120, "expectedTempo": 100, "expectedPresetName": "Clean",
                "expectedPresetIdentity": "fixture-native-A"
            }),
        ),
        (
            "device.selectScene",
            json!({"scene": 1, "expectedPresetName": "Clean",
                    "expectedPresetIdentity": "fixture-native-A"}),
        ),
        (
            "device.tapTempo",
            json!({"expectedMode": "STOMP", "expectedPresetIdentity": "x"}),
        ),
    ] {
        assert_device_suspended(&call(&controller, method, params));
    }
    assert_eq!(controller.status().suspension.refused_requests, 3);
    // Flight records only: no write, read or backup reached the worker.
    assert_eq!(
        commands.device_work(),
        0,
        "a refused request queued device work"
    );
}

#[test]
fn status_reports_the_suspension_to_every_client() {
    let (controller, _commands) = suspended_for_backup();
    let response = call(&controller, "system.status", json!({}));
    let diagnostics = &response["result"]["usbDiagnostics"];
    assert_eq!(diagnostics["deviceSuspended"], true, "{response}");
    assert_eq!(diagnostics["suspension"]["active"]["reason"], "BACKUP");
    assert!(diagnostics["suspension"]["active"]["sinceUnixMs"]
        .as_u64()
        .is_some());
    let response = call(&controller, "device.status", json!({}));
    assert_eq!(response["result"]["deviceSuspended"], true);

    controller.observe_device_frame_for_test(
        profile::MESSAGE_TYPE_SUSPEND_CONNECTION,
        &[0x10, 0x00, 0x18, 0x00],
    );
    let response = call(&controller, "system.status", json!({}));
    let diagnostics = &response["result"]["usbDiagnostics"];
    assert_eq!(diagnostics["deviceSuspended"], false);
    assert!(diagnostics["suspension"]["active"].is_null());
    assert_eq!(
        diagnostics["suspension"]["lastEnded"]["endedBy"],
        "deviceResumed"
    );
    assert_eq!(
        diagnostics["suspension"]["lastEnded"]["resumeReason"],
        "BACKUP"
    );
}

#[test]
fn status_reports_a_backup_that_ends_in_device_frame_order() {
    // The unit releases the screen lock just before it sends the resume.
    let (controller, _commands) = suspended_for_backup();
    controller.observe_device_frame_for_test(
        profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        &[0x08, 0x01, 0x30, 0x00],
    );
    let response = call(&controller, "system.status", json!({}));
    let suspension = &response["result"]["usbDiagnostics"]["suspension"];
    assert_eq!(
        response["result"]["usbDiagnostics"]["deviceSuspended"], false,
        "{response}"
    );
    assert_eq!(suspension["lastEnded"]["endedBy"], "screenUnlocked");
    assert!(suspension["lastEnded"]["resumeReason"].is_null());

    controller.observe_device_frame_for_test(
        profile::MESSAGE_TYPE_SUSPEND_CONNECTION,
        &[0x10, 0x00, 0x18, 0x00],
    );
    let response = call(&controller, "system.status", json!({}));
    let suspension = &response["result"]["usbDiagnostics"]["suspension"];
    assert_eq!(suspension["unmatchedResumes"], 0, "{response}");
    assert_eq!(suspension["suspensionsObserved"], 1);
    assert_eq!(suspension["lastEnded"]["endedBy"], "screenUnlocked");
    assert_eq!(suspension["lastEnded"]["resumeReason"], "BACKUP");
    assert_eq!(suspension["lastEnded"]["resyncRequested"], false);
}

#[test]
fn session_control_stays_available_while_suspended() {
    let (controller, commands) = suspended_for_backup();
    // No worker answers here; what matters is that the gate let them by.
    drop(commands);
    for method in [
        "device.resetSession",
        "device.disconnect",
        "device.reconnect",
    ] {
        let response = call(&controller, method, json!({}));
        assert_ne!(
            response["error"]["code"], DEVICE_SUSPENDED_ERROR_CODE,
            "{method} was refused: {response}"
        );
    }
}

#[test]
fn an_owner_refusal_reads_as_device_suspended_unless_part_of_it_was_sent() {
    let (controller, _commands) = suspended_for_backup();
    let refusal = controller.suspension_refusal().unwrap();
    // A wire read the session owner refused before sending.
    assert_device_suspended(&failure_response(
        &controller,
        json!(7),
        "device.identity",
        false,
        &format!("USB command was not admitted: {refusal}"),
    ));
    assert!(controller.write_quarantine().is_none());
    // A refusal inside a workflow whose earlier steps ran: typed, but it
    // must not claim the request never left, and keeps its full text.
    let composed = format!("No persistent USB write was attempted. {refusal}");
    let response = failure_response(&controller, json!(9), "device.copyPreset", true, &composed);
    assert_eq!(response["error"]["code"], DEVICE_SUSPENDED_ERROR_CODE);
    assert!(
        response["error"]["data"]["requestSent"].is_null(),
        "{response}"
    );
    assert_eq!(response["error"]["message"], composed);
    assert!(controller.write_quarantine().is_none());
    // A write whose first frame had already left is a different matter.
    let response = failure_response(
        &controller,
        json!(8),
        "device.setTempo",
        true,
        &format!(
            "{refusal}; an earlier frame of this request was already sent, so the device \
                 effect may be uncertain"
        ),
    );
    assert_eq!(response["error"]["data"]["code"], "WRITE_OUTCOME_AMBIGUOUS");
    assert!(controller.write_quarantine().is_some());
}

#[test]
fn a_suspended_current_preset_refresh_reads_as_device_suspended() {
    // device.currentPreset with an incomplete Grid sends one type-15 READ.
    // The session owner refuses it during a backup like any other read, and
    // the reply must say so: not STATE_UNAVAILABLE without requestSent.
    let (controller, _commands) = suspended_for_backup();
    let refusal = controller.suspension_refusal().unwrap();
    assert_device_suspended(&failure_response(
        &controller,
        json!(7),
        "device.currentPreset",
        false,
        &current_preset_refresh_unavailable(refusal),
    ));
    // Every other refresh failure keeps its STATE_UNAVAILABLE code.
    let response = failure_response(
        &controller,
        json!(8),
        "device.currentPreset",
        false,
        &current_preset_refresh_unavailable("QC session changed before logical read scope".into()),
    );
    assert_eq!(response["error"]["data"]["code"], "STATE_UNAVAILABLE");
}

#[test]
fn framing_reads_one_request() {
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"system.status"}"#;
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(body);
    let request = read_request(&mut frame.as_slice()).unwrap().unwrap();
    assert_eq!(request.method, "system.status");
    assert_eq!(request.id, 1);
}

#[test]
fn request_envelope_rejects_unknown_fields_and_nonpositive_ids() {
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"system.status","extra":true}"#;
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(body);
    assert!(read_request(&mut frame.as_slice()).is_err());

    let controller = DeviceController::start_disconnected().expect("controller starts");
    let performance_midi = PerformanceMidiWorkers::default();
    let response = handle(
        &controller,
        &performance_midi,
        Request {
            jsonrpc: "2.0".into(),
            id: json!(0),
            method: "system.status".into(),
            params: json!({}),
            origin: None,
            physical_only: false,
            expected_broker_target: None,
        },
    );
    assert_eq!(response["error"]["code"], -32600);
}

#[test]
fn canonical_gateway_boundary_rejects_missing_extra_and_wrong_typed_arguments() {
    assert!(generated_gateway::validate_params("device.snapshot", &json!({})).is_ok());
    assert!(
        generated_gateway::validate_params("device.snapshot", &json!({"ignored": true})).is_err()
    );
    assert!(generated_gateway::validate_params(
            "device.setTempo",
            &json!({
                "bpm": 120, "expectedTempo": 120, "expectedPresetName": "Clean", "expectedPresetIdentity": "fixture-native-A"
            })
        )
        .is_ok());
    assert!(generated_gateway::validate_params(
            "device.setTempo",
            &json!({
                "bpm": true, "expectedTempo": 120, "expectedPresetName": "Clean", "expectedPresetIdentity": "fixture-native-A"
            })
        )
        .is_err());
    assert!(generated_gateway::validate_params(
            "device.setTempo",
            &json!({
                "expectedTempo": 120, "expectedPresetName": "Clean", "expectedPresetIdentity": "fixture-native-A"
            })
        )
        .is_err());
    assert!(generated_gateway::validate_params(
        "device.setInputPort",
        &json!({
            "inputPortId": 1, "levelDb": null, "impedance": null,
            "inputType": null, "groundLift": null
        })
    )
    .is_ok());
    assert!(generated_gateway::validate_params(
        "device.setInputPort",
        &json!({
            "inputPortId": 1, "levelDb": "12", "impedance": null,
            "inputType": null, "groundLift": null
        })
    )
    .is_err());
}

#[test]
fn native_broker_schema_describes_gateway_envelope_and_host_internal_methods() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../contracts/public/native-broker.v1.schema.json"
    ))
    .expect("native broker schema parses");
    // The public request definition points at the fully modeled
    // compatibility request. Host-specific response extensions remain
    // outside this core broker contract.
    assert_eq!(
        schema["$defs"]["request"]["$ref"],
        "#/$defs/internalCompatibilityRequest"
    );
    let request_description = schema["$defs"]["request"]["description"]
        .as_str()
        .expect("request def carries a plain-text description");
    assert!(request_description.contains("host-specific response extensions"));
    assert!(!request_description.contains("../private"));
    assert!(!request_description.contains("gateway.v1.schema.json"));

    // The compatibility methods are exactly the host-internal names the
    // broker still answers; the deleted raw/command surface is
    // neither enumerated nor given parameter definitions.
    let compatibility_methods = schema["$defs"]["internalCompatibilityRequest"]["properties"]
        ["method"]["enum"]
        .as_array()
        .expect("compatibility methods are enumerated")
        .iter()
        .map(|method| method.as_str().expect("method is a string"))
        .collect::<Vec<_>>();
    assert_eq!(
        compatibility_methods,
        [
            "device.status",
            "device.state.events",
            "device.state.blockDetails"
        ]
    );
    assert!(!compatibility_methods.is_empty(), "must enumerate methods");
    for method in &compatibility_methods {
        assert!(!generated_gateway::METHODS.contains(method));
        assert_eq!(flight_method_name(method), *method);
    }
    let text = serde_json::to_string(&schema["$defs"]).unwrap();
    assert!(!text.contains("device.raw."), "{text}");
    assert!(!text.contains("device.command."), "{text}");
    assert!(schema["$defs"].get("messageType").is_none());
}

#[test]
fn writes_without_a_readback_predicate_are_explicitly_unverified() {
    let result = accepted_unverified("Undo sent");
    assert_eq!(result["accepted"], true);
    assert_eq!(result["verified"], false);
    assert_eq!(result["verification"], "accepted_unverified");
    assert_eq!(result["detail"], "Undo sent");
}

#[test]
fn matching_uncorrelated_settings_publication_never_claims_authoritative_effect() {
    let result = matching_settings_observation_result("Set input gain");
    assert_eq!(result["accepted"], true);
    assert_eq!(result["verified"], false);
    assert_eq!(result["verification"], "accepted_unverified");
    assert_eq!(result["observation"]["causallyCorrelated"], false);
    assert!(result.get("changed").is_none());
    assert!(result.get("readback").is_none());
}

#[test]
fn a_write_that_moved_nothing_reports_that_instead_of_implying_it_landed() {
    // The write verifier's predicate is absolute (state == desired) against
    // the cached snapshot, and freshness was any newer frame sequence -
    // which `worker.rs` hands out for a tempo-clock-only frame. So every
    // non-realtime write whose requested value already equalled the current
    // one returned `verified: true, authoritative_readback` with nothing
    // distinguishing "the device applied it" from "it was already there".
    //
    // Worst case on record: writing Pattern Tremolo's RATE the value it
    // already shows. A 2026-09-11 measurement recorded in the device-knowledge skill
    // (no artifact retained in the repository, BELIEVED) says CorOS redirects the write to SYNC NOTE, and
    // the five models that declare SYNC without gating RATE never publish
    // `enabled: false`, so nothing else catches it either.
    let unmoved = verified_readback_result(
        "Set RATE",
        Some(false),
        "snapshot",
        json!({"presetName": "Numb", "blocks": []}),
    );
    assert_eq!(unmoved["verification"], "authoritative_readback");
    assert_eq!(
        unmoved["changed"],
        json!(false),
        "the requested state was already present before anything was sent"
    );

    let moved = verified_readback_result(
        "Set RATE",
        Some(true),
        "snapshot",
        json!({"presetName": "Numb", "blocks": []}),
    );
    assert_eq!(moved["changed"], json!(true));

    // No prior observation means no claim either way: the schema's
    // `x-verification-outcomes` marks `changed` optional on a readback for
    // exactly this case.
    let blind = verified_readback_result(
        "Set RATE",
        None,
        "snapshot",
        json!({"presetName": "Numb", "blocks": []}),
    );
    assert!(blind.get("changed").is_none());

    // A preflight that suppressed the mutation sent nothing at all.
    let suppressed = authoritative_preflight_result(json!({"activeScene": 0}));
    assert_eq!(suppressed["verification"], "authoritative_preflight");
    assert_eq!(suppressed["changed"], json!(false));
    assert_eq!(suppressed["requestSent"], json!(false));
    assert!(suppressed["detail"]
        .as_str()
        .unwrap()
        .contains("no mutation was sent"));

    // And an acknowledgement that observed nothing must never carry it.
    assert!(accepted_unverified("Undo sent").get("changed").is_none());

    // Every shape has to satisfy the generated contract, whose
    // x-verification-outcomes rules reject a no-op that claims it changed
    // state and an unverified acknowledgement that claims to know.
    for result in [&unmoved, &moved, &blind, &suppressed] {
        generated_gateway::validate_result("device.setParameter", result)
            .unwrap_or_else(|error| panic!("{result} must satisfy its contract: {error}"));
    }
    let lying_preflight = json!({
        "accepted": true,
        "verified": true,
        "verification": "authoritative_preflight",
        "detail": "Set RATE",
        "changed": true,
    });
    assert!(
        generated_gateway::validate_result("device.setParameter", &lying_preflight).is_err(),
        "a suppressed mutation may never claim it changed device state"
    );
    let mut lying_acknowledgement = accepted_unverified("Undo sent");
    lying_acknowledgement["changed"] = json!(false);
    assert!(
        generated_gateway::validate_result("device.undo", &lying_acknowledgement).is_err(),
        "an unverified acknowledgement may never claim to know either way"
    );
}

#[test]
fn system_status_identifies_the_rust_gateway_contract() {
    // Non-vacuous pin: the response must identify at least one gateway capability.
    assert!(!generated_gateway::CAPABILITIES.is_empty());
    let controller = DeviceController::start_disconnected().expect("controller starts");
    assert_eq!(controller.status().phase, "disconnected");
    let performance_midi = PerformanceMidiWorkers::default();
    let response = handle(
        &controller,
        &performance_midi,
        Request {
            jsonrpc: "2.0".into(),
            id: json!(1),
            method: "system.status".into(),
            params: json!({}),
            origin: None,
            physical_only: false,
            expected_broker_target: None,
        },
    );
    assert_eq!(response["result"]["platform"], "Rust device gateway");
    assert_eq!(
        response["result"]["gatewayApiVersion"],
        generated_gateway::API_VERSION
    );
    assert_eq!(response["result"]["gatewayAvailable"], true);
    assert_eq!(
        response["result"]["usbDiagnostics"]["phase"],
        "disconnected"
    );
    assert_eq!(response["result"]["usbDiagnostics"]["connected"], false);
    assert_eq!(response["result"]["usbDiagnostics"]["messagesSent"], 0);
    assert_eq!(
        response["result"]["usbDiagnostics"]["messagesSentByType"],
        json!({})
    );
    assert_eq!(
        response["result"]["usbDiagnostics"]["messagesReceivedByType"],
        json!({})
    );
    assert_eq!(
        response["result"]["usbDiagnostics"]["maxHidWriteDurationMs"],
        0
    );
    // What the host received and could not act on is readable from the
    // same payload as what it did act on, without a debugger.
    let inbound = &response["result"]["usbDiagnostics"]["inboundTelemetry"];
    assert_eq!(inbound["frames"], 0);
    assert_eq!(inbound["unactedFrames"], 0);
    assert_eq!(inbound["noStateHandlerFrames"], 0);
    assert_eq!(inbound["undeclaredTypes"], json!({}));
    assert_eq!(inbound["decodeFailures"], json!({}));
    assert_eq!(inbound["noStateHandlerTypes"], json!({}));
    assert_eq!(inbound["encryptedRejections"], json!({}));
    assert_eq!(inbound["compressed"], json!({}));
    assert_eq!(inbound["inflateFailures"], json!({}));
    assert_eq!(inbound["deviceTrailerBytes"], json!([]));
    assert_eq!(inbound["lastUnacted"], json!(null));
    for capability in generated_gateway::CAPABILITIES {
        assert!(response["result"]["capabilities"]
            .as_array()
            .expect("capability array")
            .iter()
            .any(|value| value == capability));
    }
}

#[test]
fn quarantine_blocks_mutations_but_keeps_reads_available() {
    let controller = DeviceController::start_disconnected().expect("controller starts");
    let performance_midi = PerformanceMidiWorkers::default();
    controller.quarantine_writes("device.removeBlock failed after transmitting to the device");

    let write = handle(
        &controller,
        &performance_midi,
        Request {
            jsonrpc: "2.0".into(),
            id: json!(7),
            method: "device.setTempo".into(),
            params: json!({
                "bpm": 120,
                "expectedTempo": 120,
                "expectedPresetName": "Clean", "expectedPresetIdentity": "fixture-native-A"
            }),
            origin: None,
            physical_only: false,
            expected_broker_target: None,
        },
    );
    assert_eq!(write["error"]["code"], WRITE_QUARANTINED_ERROR_CODE);
    assert_eq!(write["error"]["data"]["writesQuarantined"], true);

    let read = handle(
        &controller,
        &performance_midi,
        Request {
            jsonrpc: "2.0".into(),
            id: json!(8),
            method: "system.status".into(),
            params: json!({}),
            origin: None,
            physical_only: false,
            expected_broker_target: None,
        },
    );
    assert!(read.get("result").is_some(), "{read}");
    assert_eq!(read["result"]["usbDiagnostics"]["writesQuarantined"], true);
}

/// The legacy `device.raw.*` and `device.command.*`
/// compatibility surface had no caller, and `device.raw.request` slipped
/// past the suspension and quarantine gates. It is deleted: every one of
/// those names is an unknown method, refused before anything reaches the
/// worker, and none of them is classified as a device mutation any more.
#[test]
fn the_legacy_raw_and_command_surface_is_gone() {
    let legacy = [
        ("device.raw.latest", json!({"messageType": 33})),
        ("device.raw.events", json!({"afterSequence": 0})),
        (
            "device.raw.send",
            json!({"messageType": 33, "payloadBase64": ""}),
        ),
        (
            "device.raw.request",
            json!({"messageType": 33, "payloadBase64": "", "timeoutMs": 50}),
        ),
        ("device.command.scene", json!({"scene": 1})),
        (
            "device.command.bypass",
            json!({"row": 0, "column": 0, "bypassed": true}),
        ),
        (
            "device.command.parameter",
            json!({"row": 0, "column": 0, "parameterIndex": 0, "value": 0.5}),
        ),
        ("device.command.tempo", json!({"bpm": 120})),
        ("device.command.operation", json!({"operation": "undo"})),
    ];
    let controller = DeviceController::start_disconnected().expect("controller starts");
    for (method, params) in &legacy {
        assert!(!is_device_mutation(method), "{method}");
        assert_eq!(flight_method_name(method), "unknown-method", "{method}");
        let response = call(&controller, method, params.clone());
        assert_eq!(response["error"]["code"], -32601, "{method}: {response}");
        assert!(
            response["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("Method not found")),
            "{method}: {response}"
        );
    }
    let status = controller.status();
    assert_eq!(status.messages_sent, 0);
    assert!(status.messages_sent_by_type.is_empty());

    // A suspended device answers the same way and queues nothing: the
    // names are unknown, so they are not even counted as refused writes.
    let (controller, commands) = suspended_for_backup();
    for (method, params) in &legacy {
        let response = call(&controller, method, params.clone());
        assert_eq!(response["error"]["code"], -32601, "{method}: {response}");
    }
    assert_eq!(controller.status().suspension.refused_requests, 0);
    assert_eq!(commands.device_work(), 0, "a legacy name queued work");
}

/// One failed descriptor write used to end the heartbeat
/// for good, silently. It must retry, recover, and stop only when the
/// claim is released.
#[test]
fn a_failed_descriptor_refresh_is_retried_until_the_claim_is_released() {
    let claim = Arc::new(Mutex::new(Some(0_u32)));
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let refresher = {
        let (claim, attempts) = (Arc::clone(&claim), Arc::clone(&attempts));
        test_support::FixtureJob::spawn(move |stop| {
            refresh_descriptor_while_claimed_until(
                &claim,
                Duration::from_millis(20),
                Duration::from_millis(1),
                &stop,
                |written| {
                    // Three failures in a row, then every write lands.
                    if attempts.fetch_add(1, Ordering::SeqCst) < 3 {
                        return Err("held open without delete sharing".into());
                    }
                    *written += 1;
                    Ok(())
                },
            )
        })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while claim.lock_recover().is_some_and(|written| written < 2) {
        assert!(
            Instant::now() < deadline && !refresher.is_finished(),
            "the heartbeat stopped after {} attempt(s)",
            attempts.load(Ordering::SeqCst)
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !refresher.is_finished(),
        "the heartbeat ended while claimed"
    );
    claim.lock_recover().take();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !refresher.is_finished() {
        assert!(
            Instant::now() < deadline,
            "a released claim must end the heartbeat"
        );
        thread::sleep(Duration::from_millis(5));
    }
    refresher.join().unwrap();
}

#[test]
fn heartbeat_fixture_unwind_stops_its_original_job_even_while_the_claim_remains() {
    let claim = Arc::new(Mutex::new(Some(0_u32)));
    let finished = Arc::new(AtomicBool::new(false));
    let (ready, entered) = mpsc::channel();
    let owned_claim = Arc::clone(&claim);
    let done = Arc::clone(&finished);
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _refresher = test_support::FixtureJob::spawn(move |stop| {
            refresh_descriptor_while_claimed_until(
                &owned_claim,
                Duration::from_millis(1),
                Duration::from_millis(1),
                &stop,
                |written| {
                    *written += 1;
                    let _ = ready.send(());
                    Ok(())
                },
            );
            done.store(true, Ordering::Release);
        });
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        panic!("injected heartbeat assertion failure");
    }));
    assert!(failure.is_err());
    assert!(finished.load(Ordering::Acquire));
    assert!(
        claim.lock_recover().is_some(),
        "stop ownership, not claim removal, ended the worker"
    );
}

/// A failed `accept` backs off instead of spinning, and only
/// the first of a run (then every hundredth) is logged.
#[test]
fn a_failing_accept_backs_off_and_logs_once_per_run() {
    let error = std::io::Error::other("no buffer space");
    let mut backoff = AcceptBackoff::default();
    assert!(backoff.failed(&error).is_some());
    assert_eq!(backoff.delay(), ACCEPT_RETRY_FLOOR);
    assert!(backoff.failed(&error).is_none());
    assert_eq!(backoff.delay(), ACCEPT_RETRY_FLOOR * 2);
    for _ in 2..99 {
        assert!(backoff.failed(&error).is_none());
    }
    assert!(backoff.failed(&error).is_some(), "the hundredth is logged");
    assert_eq!(backoff.delay(), ACCEPT_RETRY_CEILING);
    backoff.accepted();
    assert!(backoff.failed(&error).is_some(), "a new run is logged");
    assert_eq!(backoff.delay(), ACCEPT_RETRY_FLOOR);
}

/// The refusal is shown to people; no indentation may leak
/// into it from the source layout.
#[test]
fn the_descriptor_claim_refusal_reads_as_one_sentence_run() {
    let message = descriptor_claim_refusal(Path::new("Z:\\no-such\\device-broker.json"));
    assert!(!message.contains("  "), "{message}");
    assert!(message.contains("already published there"), "{message}");
    assert!(
        message.contains("attaches to that one instead."),
        "{message}"
    );
}

#[test]
fn unused_grace_reservation_admits_no_new_participant_until_retirement() {
    let shared = test_support::handle().0;
    // The actual admission helper is taken under the same production lock,
    // after finding no holder, rather than repeating an independent predicate.
    let reserve = || {
        let _request = shared.request_lock.lock_recover();
        BrokerShared::reserve_unused_device(&shared.clients, &shared.controller, &shared.controller)
    };
    let admission = reserve().unwrap();
    let newcomer = accept_client(&shared.clients).unwrap();
    let refused = shared.mark_client_authenticated(newcomer).unwrap_err();
    assert!(refused.contains("closing"));
    assert!(!shared.clients.lock_recover().connections[&newcomer].authenticated);
    drop(admission);
    shared.mark_client_authenticated(newcomer).unwrap();
    assert!(
        reserve().is_none(),
        "a real current holder keeps the device open"
    );
    shared.detach_client(newcomer);
    let pending = accept_client(&shared.clients).unwrap();
    assert!(
        reserve().is_none(),
        "the existing pending-socket keep-open policy is preserved"
    );
    shared.detach_client(pending);
}

#[test]
fn total_connection_admission_counts_authenticated_and_detached_live_owners() {
    let shared = test_support::handle().0;
    let mut ids = Vec::new();
    for _ in 0..BROKER_MAX_CONNECTION_OWNERS {
        let id = accept_client(&shared.clients).expect("within total software budget");
        shared.mark_client_authenticated(id).unwrap();
        ids.push(id);
    }
    assert!(accept_client(&shared.clients).is_none());
    assert_eq!(
        shared.clients.lock_recover().count,
        BROKER_MAX_CONNECTION_OWNERS
    );
    let id = ids.pop().unwrap();
    let owner = connection_owner(&shared.clients, id).unwrap();
    let (release, released) = mpsc::channel();
    let (entered, ready) = mpsc::channel();
    let mut serving =
        ServingLifetime::new(Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(None)));
    let thread_owner = Arc::clone(&owner);
    let worker = thread::spawn(move || {
        let _owner = thread_owner;
        entered.send(()).unwrap();
        let _ = released.recv_timeout(Duration::from_secs(5));
    });
    serving.jobs.push(ServingJob {
        thread: worker,
        socket: None,
        _connection_owner: Some(owner),
    });
    struct ReleaseOnDrop(mpsc::Sender<()>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let _release_on_unwind = ReleaseOnDrop(release.clone());
    ready.recv_timeout(Duration::from_secs(1)).unwrap();
    shared.detach_client(id);
    assert!(
        accept_client(&shared.clients).is_none(),
        "detached live original worker consumes its slot"
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !crate::platform::process::thread_signalled(&serving.jobs[0].thread) {
        assert!(
            Instant::now() < deadline,
            "controlled actual worker must finish"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        accept_client(&shared.clients).is_none(),
        "unreaped original handle still retains its lease"
    );
    serving.reap_finished();
    assert!(serving.jobs.is_empty());
    let replacement = accept_client(&shared.clients).expect("actual retirement frees capacity");
    assert!(accept_client(&shared.clients).is_none());
    shared.detach_client(replacement);
    for id in ids {
        shared.detach_client(id);
    }
    assert_eq!(shared.clients.lock_recover().count, 0);
}

#[test]
fn hello_provider_budget_preserves_previous_identity_and_counts_utf8_bytes() {
    let shared = test_support::handle().0;
    let id = accept_client(&shared.clients).unwrap();
    shared.mark_client_authenticated(id).unwrap();
    let hello = |provider: String| {
        dispatch_broker_client_request(
            &shared,
            Some(id),
            Request {
                jsonrpc: "2.0".into(),
                id: json!(1),
                method: "broker.hello".into(),
                params: json!({"provider":provider}),
                origin: None,
                physical_only: false,
                expected_broker_target: None,
            },
        )
    };
    let accepted = "é".repeat(BROKER_PROVIDER_MAX_UTF8_BYTES / 2);
    assert!(hello(accepted.clone()).get("result").is_some());
    let refused = hello(format!("{accepted}x"));
    assert_eq!(refused["error"]["code"], -32602);
    assert!(refused["error"]["message"]
        .as_str()
        .unwrap()
        .contains("4096 UTF-8 bytes"));
    assert_eq!(
        shared.clients.lock_recover().connections[&id].provider,
        accepted
    );
    assert_eq!(shared.clients.lock_recover().count, 1);
    assert_eq!(
        shared.clients.lock_recover().connection_owners.len(),
        1,
        "hello preserves the same admission lease"
    );
    shared.detach_client(id);
}

#[test]
fn an_oversized_owner_private_descriptor_is_not_read_for_a_claim_diagnostic() {
    let root = crate::device_lease::testing::fresh_root("claim-diagnostic-budget");
    let path = root.0.join("device-broker.json");
    let bytes = br#"{"processId":12345,"credential":"must-not-be-disclosed"}"#;
    qc_owner_only_file::publish_atomically(&path, bytes).unwrap();
    let healthy = descriptor_claim_refusal(&path);
    assert!(healthy.contains("device broker process 12345"), "{healthy}");
    assert!(!healthy.contains("must-not-be-disclosed"));
    // Sparse growth changes no ACL and requires no large allocation in the
    // fixture. The original document remains untouched by the refusal.
    let oversized = (BROKER_DESCRIPTOR_DIAGNOSTIC_MAX_BYTES + 1) as u64;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(oversized)
        .unwrap();
    let refused = descriptor_claim_refusal(&path);
    assert!(
        !refused.contains("device broker process 12345"),
        "{refused}"
    );
    assert!(refused.contains("already published there"), "{refused}");
    assert!(!refused.contains("credential"));
    assert_eq!(std::fs::metadata(&path).unwrap().len(), oversized);
    let mut file = std::fs::File::open(&path).unwrap();
    let mut prefix = vec![0; bytes.len()];
    std::io::Read::read_exact(&mut file, &mut prefix).unwrap();
    assert_eq!(prefix, bytes);
}

/// A selection waits for `request_lock`, possibly behind
/// another client's backup, so it must not hold the connection's output
/// while it does; only hello, which never waits for it, may.
#[test]
fn only_hello_holds_the_connection_output_while_dispatching() {
    assert!(holds_output_while_dispatching("broker.hello"));
    for method in [
        "broker.selectDevice",
        "broker.shutdown",
        "device.createBackup",
        generated_gateway::rpc::DISCONNECT,
    ] {
        assert!(!holds_output_while_dispatching(method), "{method}");
    }
}

#[test]
fn generated_action_results_are_all_classified_as_mutations() {
    assert!(!generated_gateway::METHODS.is_empty());
    for method in generated_gateway::METHODS {
        if generated_gateway::result_kind(method)
            == Some(generated_gateway::GatewayResultKind::DeviceActionResult)
        {
            assert!(is_device_mutation(method), "{method}");
        }
    }
    assert!(is_device_mutation("device.createBackup"));
    assert!(is_device_mutation("device.previewParameter"));
    assert!(!is_device_mutation("device.snapshot"));
    assert!(!is_device_mutation("device.currentPreset"));
}

#[test]
fn single_frame_device_writes_have_no_readback_to_quarantine_through() {
    // A tuner write and set_device_name used to be followed by a host
    // READ ladder. When the unit answered late or not at all, the ladder
    // failed as "<detail> was sent, but ...", which
    // `error_has_ambiguous_write_outcome` reads as an ambiguous write and
    // answers with a Device-scope quarantine that refuses every later
    // mutation until a reconnect. They now send one frame and return
    // accepted_unverified, so no readback mismatch exists to raise it.
    let cases = [
        (
            "device.setTunerInput",
            json!({"inputPortId": 1, "confirmTunerActivation": true}),
        ),
        (
            "device.setTunerMute",
            json!({"muted": false, "confirmTunerActivation": true}),
        ),
        (
            "device.setTunerReference",
            json!({"referenceOffsetHz": 1.0, "confirmTunerActivation": true}),
        ),
        (
            "device.setTunerMeter",
            json!({"enabled": true, "confirmTunerActivation": true}),
        ),
        ("device.setDeviceName", json!({"name": "Stage QC"})),
        ("device.undo", json!({})),
        ("device.redo", json!({})),
    ];
    for (method, params) in cases {
        let policy = runtime_request::gateway_write_verification_policy(method);
        assert!(policy.readback_method.is_none(), "{method}");
        assert!(policy.preflight_method.is_none(), "{method}");
        let plan = runtime_request::plan_gateway_write(method, &params, None).unwrap();
        assert!(
            !plan.verification.requires_authoritative_readback(),
            "{method}"
        );
        assert!(
            !runtime_request::gateway_write_is_realtime(method),
            "{method}"
        );
    }
    // The shape the removed ladder produced would have quarantined.
    assert!(error_has_ambiguous_write_outcome(
            "Tuner mute preference updated; the tuner is now invisibly engaged was sent, but correlated device.tunerSettings readback did not match after 5 attempts"
        ));
    // The single-frame paths report only a delivery wait's own outcome.
    assert!(!error_has_ambiguous_write_outcome(
        "The Quad Cortex is not connected"
    ));
}

#[test]
fn only_explicitly_ambiguous_failures_trigger_global_quarantine() {
    for message in [
            "Persistent USB reports were delivered, but the device outcome was not verified. Catalog timeout",
            "A persistent USB write was attempted; its device outcome remains unconfirmed. Native timeout",
        ] {
            assert!(error_has_ambiguous_write_outcome(message), "{message}");
        }
    assert!(!error_has_ambiguous_write_outcome(
        "No persistent USB write was attempted. Preparation expired."
    ));
    assert!(error_has_ambiguous_write_outcome(
        "Remove block was sent, but authoritative preset readback did not confirm it"
    ));
    assert!(error_has_ambiguous_write_outcome(
        "The persistent File mutation was sent exactly once, but verification failed"
    ));
    assert!(!error_has_ambiguous_write_outcome(
        "The requested setting already matched authoritative device state; no mutation was sent."
    ));
    assert!(!error_has_ambiguous_write_outcome(
        "Expected preset Clean but the active preset is Lead"
    ));
    // The three shapes `DeviceController::wait_for_native_write` actually
    // returns (worker.rs). Every raw/legacy command handler
    // (`command_scene`, `raw_send`, ...) propagates one of these
    // unwrapped, so this classifier is the only place that can still
    // catch them.
    for message in [
            "QC write admission was not confirmed within its original budget: timed out; device effect may be uncertain",
            "QC native delivery was not confirmed within its original budget; device effect may be uncertain",
            "QC native delivery was not confirmed: channel closed; device effect may be uncertain",
            // DeviceController::create_backup's own worker-channel timeout
            // (worker.rs): the backup command was already admitted and may
            // already be executing by the time this fires.
            "Native QC backup worker timed out; device effect may be uncertain",
        ] {
            assert!(error_has_ambiguous_write_outcome(message), "{message}");
        }
    assert_eq!(
        flight_method_name("device.removeBlock"),
        "device.removeBlock"
    );
    assert_eq!(
        flight_method_name("customer-name-or-secret"),
        "unknown-method",
        "untrusted method text must not enter the persistent flight record"
    );
}

/// `DeviceController::plan_gateway_write` (`worker.rs`) plans against `owner.state` under
/// the `Arc<Mutex<SessionRuntime>>` the USB reader pushes into (`quadcortex-windows`
/// `read_message_poll`). This test takes the same two steps on that shared owner; it does not
/// construct a `DeviceController`.
#[test]
fn a_lost_delete_refuses_the_next_broker_load_with_no_later_frame() {
    use quadcortex::device::owner::SessionRuntime;
    use quadcortex::device::session::SessionDriver;
    let owner = std::sync::Arc::new(std::sync::Mutex::new(SessionRuntime::new(0)));
    let (connection, epoch) = {
        let mut owner = owner.lock().unwrap();
        let connection = owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
        (connection, owner.capture_report_epoch(connection).unwrap())
    };
    let push = |message_type: u16, payload: &[u8], at: u64| {
        let mut completed = None;
        for mut report in quadcortex::framing::encode(message_type, payload) {
            report[0] = quadcortex::framing::IN_REPORT_ID;
            completed = owner
                .lock()
                .unwrap()
                .push_native_report(connection, epoch, &report, at)
                .unwrap();
        }
        completed
    };
    let plan = || {
        let owner = owner.lock().unwrap();
        runtime_request::plan_gateway_write_with_state(
            "device.loadGlobalEqModelPreset",
            &serde_json::json!({"value": "one", "isFactory": false, "hash": 4004}),
            &owner.state,
        )
        .map(|_| ())
    };
    push(13, &[8, 1, 16, 42], 1).unwrap();
    let create = [
        0x08, 0x00, 0x1a, 0x0c, 0x0a, 0x0a, 0x0a, 0x03, b'o', b'n', b'e', 0x10, 0x00, 0x18, 0xa4,
        0x1f,
    ];
    let (frame, ticket) = push(71, &create, 2).unwrap();
    owner
        .lock()
        .unwrap()
        .observe_state(&ticket, frame.message_type, &frame.payload)
        .unwrap();
    assert!(plan().is_ok());

    let reports = quadcortex::framing::encode(71, &vec![0_u8; 400]);
    assert!(reports.len() >= 3);
    for mut report in reports.into_iter().skip(1) {
        report[0] = quadcortex::framing::IN_REPORT_ID;
        let _ = owner
            .lock()
            .unwrap()
            .push_native_report(connection, epoch, &report, 3);
    }
    let refused = plan().unwrap_err();
    assert!(refused.starts_with("STATE_UNAVAILABLE: "), "{refused}");
    assert!(refused.contains("inbound-loss"), "{refused}");
}
