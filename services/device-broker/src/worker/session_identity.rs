use super::*;

/// One best-effort read per transport connection. Completion is driven by
/// incoming frames; neither status reads nor a failed attempt schedule traffic.
pub(super) struct SessionIdentityRead {
    enabled: Arc<AtomicBool>,
    attempted: bool,
    pending: Option<PendingIdentityRead>,
}

struct PendingIdentityRead {
    scope: GatewayReadScope,
    runtime: GatewayReadRuntime,
    after_sequence: u64,
    epoch: u64,
}

impl SessionIdentityRead {
    pub(super) fn new(enabled: Arc<AtomicBool>) -> Self {
        Self {
            enabled,
            attempted: false,
            pending: None,
        }
    }

    pub(super) fn start_once(
        &mut self,
        session: &Arc<Mutex<SessionRuntime>>,
        epoch: u64,
        now_ms: u64,
        after_sequence: u64,
        mut send: impl FnMut(OutboundMessage, &OperationTicket) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.attempted || !self.enabled.load(Ordering::Acquire) {
            return Ok(());
        }
        // Latch before planning or dispatch: failure must not become a retry loop.
        self.attempted = true;
        let plan =
            runtime_request::plan_gateway_read("device.identity", &serde_json::Value::Null, 0)?;
        let (scope, operation) = {
            let mut owner = session.lock_recover();
            let scope = owner.begin_gateway_read_scope(now_ms, plan.timeout_ms)?;
            let operation = owner.plan_operation_for(
                scope.context().session_fence(),
                now_ms,
                plan.timeout_ms,
            )?;
            (scope, operation)
        };
        for message in plan
            .operation
            .try_encode()
            .map_err(|error| error.to_string())?
        {
            send(message, &operation)?;
        }
        self.pending = Some(PendingIdentityRead {
            scope,
            runtime: GatewayReadRuntime::new(
                plan.response_type,
                plan.projection,
                now_ms,
                plan.timeout_ms,
            ),
            after_sequence,
            epoch,
        });
        Ok(())
    }

    pub(super) fn observe(
        &mut self,
        session: &Arc<Mutex<SessionRuntime>>,
        state: &Arc<Mutex<BrokerStatus>>,
        message: &IncomingMessage,
        now_ms: u64,
    ) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        let owner = session.lock_recover();
        if owner
            .gateway_read_scope_remaining(&pending.scope, now_ms)
            .is_err()
            || state.lock_recover().session_epoch != pending.epoch
        {
            self.pending = None;
            return;
        }
        if message.sequence <= pending.after_sequence
            || !message.observation.as_ref().is_some_and(|ticket| {
                owner.accepts_observation_for(pending.scope.context().session_fence(), ticket)
            })
            || !pending.runtime.accepts_ingress(
                message
                    .observation
                    .as_ref()
                    .and_then(ObservationTicket::ingress),
                now_ms,
            )
        {
            return;
        }
        match pending
            .runtime
            .observe(message.message_type, &message.payload, now_ms)
        {
            ReadObservation::Matched { value } => {
                // Retain only the display name, never the persistent serial.
                state.lock_recover().device_name = value
                    .get("customName")
                    .and_then(serde_json::Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                    .map(str::to_owned);
                self.pending = None;
            }
            ReadObservation::Expired => self.pending = None,
            ReadObservation::Unrelated | ReadObservation::Rejected { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_session(owner: &mut SessionRuntime, at: u64) -> u64 {
        let connection = owner.opened(at).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            connection,
            "identity-test".into(),
            at,
        ));
        connection
    }

    fn identity_message(
        owner: &mut SessionRuntime,
        connection: u64,
        name: &str,
        at: u64,
    ) -> IncomingMessage {
        IncomingMessage {
            observation: Some(
                owner
                    .capture_observation_with_ingress(
                        connection,
                        Some(quadcortex::device::transport::IngressTime {
                            first_report_at_ms: at,
                            last_report_at_ms: at,
                        }),
                    )
                    .unwrap(),
            ),
            sequence: 2,
            message_type: profile::MESSAGE_TYPE_VERSION,
            payload: pa::VersionMessage {
                action: pa::message_action::Enum::Update as i32,
                device_serial_number: Some(
                    pa::version_message::DeviceSerialNumber::DeviceSerialNumber(
                        "synthetic-identity-test".into(),
                    ),
                ),
                custom_name: Some(pa::version_message::CustomName::CustomName(name.into())),
                ..Default::default()
            }
            .encode_to_vec(),
            received_at_unix_ms: 0,
        }
    }

    #[test]
    fn device_identity_is_read_once_per_session_and_cleared_on_drop() {
        let controller = DeviceController::start_disconnected().unwrap();
        controller.set_identity_observation_enabled(true);
        let session = &controller.session_owner;
        let first_connection = open_session(&mut session.lock_recover(), 0);
        let mut read =
            SessionIdentityRead::new(Arc::clone(&controller.identity_observation_enabled));
        let mut sent = Vec::new();
        for _ in 0..3 {
            read.start_once(session, 0, 10, 1, |message, _| {
                sent.push(message);
                Ok(())
            })
            .unwrap();
        }
        let expected =
            runtime_request::plan_gateway_read("device.identity", &serde_json::Value::Null, 0)
                .unwrap()
                .operation
                .try_encode()
                .unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].message_type, expected[0].message_type);
        assert_eq!(sent[0].payload, expected[0].payload);
        let old_ingress =
            identity_message(&mut session.lock_recover(), first_connection, "Stale", 9);
        read.observe(session, &controller.state, &old_ingress, 11);
        assert!(
            controller.status().device_name.is_none(),
            "a queued pre-request reply is stale even with a newer sequence"
        );
        let mut reply =
            identity_message(&mut session.lock_recover(), first_connection, "Studio", 11);
        reply.sequence = 1;
        read.observe(session, &controller.state, &reply, 11);
        assert!(
            controller.status().device_name.is_none(),
            "pre-request frames cannot satisfy the read"
        );
        reply.sequence = 2;
        read.observe(session, &controller.state, &reply, 11);
        assert!(!serde_json::to_string(&controller.status())
            .unwrap()
            .contains("synthetic-identity-test"));
        for _ in 0..3 {
            assert_eq!(controller.status().device_name.as_deref(), Some("Studio"));
            assert_eq!(
                serde_json::to_value(controller.status()).unwrap()["deviceName"],
                "Studio"
            );
            read.start_once(session, 0, 20, 2, |_, _| {
                panic!("status must not refresh identity")
            })
            .unwrap();
        }
        // Exercise the worker's actual cache clearing seam and session fencing.
        clear_session_caches(&controller.state, &controller.latest_messages);
        session.lock_recover().disconnect(30, false);
        assert!(controller.status().device_name.is_none());
        let second_connection = open_session(&mut session.lock_recover(), 40);
        read = SessionIdentityRead::new(Arc::clone(&controller.identity_observation_enabled));
        read.start_once(session, 0, 41, 1, |message, _| {
            sent.push(message);
            Ok(())
        })
        .unwrap();
        read.observe(session, &controller.state, &reply, 42);
        assert!(
            controller.status().device_name.is_none(),
            "old-session response must be ignored"
        );
        let reply = identity_message(&mut session.lock_recover(), second_connection, "Stage", 42);
        read.observe(session, &controller.state, &reply, 42);
        assert_eq!(controller.status().device_name.as_deref(), Some("Stage"));
        assert_eq!(sent.len(), 2, "one wire read per connection");
        controller.disconnect().unwrap();
        assert!(controller.status().device_name.is_none());
    }

    #[test]
    fn device_identity_requires_an_observer_and_reads_only_once_after_opt_in() {
        let controller = DeviceController::start_disconnected().unwrap();
        let session = &controller.session_owner;
        let connection = open_session(&mut session.lock_recover(), 0);
        let mut read =
            SessionIdentityRead::new(Arc::clone(&controller.identity_observation_enabled));
        for now in [0, 100, 10_000] {
            read.start_once(session, 0, now, 1, |_, _| {
                panic!("a broker without observers must not read identity")
            })
            .unwrap();
        }
        assert!(
            !read.attempted,
            "disabled observation cannot consume the session attempt"
        );
        controller.set_identity_observation_enabled(true);
        let mut writes = 0;
        read.start_once(session, 0, 10_001, 1, |_, _| {
            writes += 1;
            Ok(())
        })
        .unwrap();
        let reply = identity_message(&mut session.lock_recover(), connection, "Observed", 10_002);
        read.observe(session, &controller.state, &reply, 10_002);
        controller.set_identity_observation_enabled(false);
        controller.set_identity_observation_enabled(true);
        read.start_once(session, 0, 10_003, 2, |_, _| {
            writes += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(writes, 1, "observer churn must not repeat a session read");
        assert_eq!(controller.status().device_name.as_deref(), Some("Observed"));
    }

    #[test]
    fn device_identity_failure_is_not_retried_in_the_session() {
        let controller = DeviceController::start_disconnected().unwrap();
        controller.set_identity_observation_enabled(true);
        let session = &controller.session_owner;
        let connection = open_session(&mut session.lock_recover(), 0);
        let mut read =
            SessionIdentityRead::new(Arc::clone(&controller.identity_observation_enabled));
        let mut writes = 0;
        assert!(read
            .start_once(session, 0, 0, 1, |_, _| {
                writes += 1;
                Err("transport rejected read".into())
            })
            .is_err());
        for now in [1, 10, 10_000] {
            read.start_once(session, 0, now, 1, |_, _| {
                writes += 1;
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(writes, 1);
        assert!(controller.status().device_name.is_none());
        // A dispatched read with no timely reply likewise stays attempted.
        let mut read =
            SessionIdentityRead::new(Arc::clone(&controller.identity_observation_enabled));
        read.start_once(session, 0, 10, 1, |_, _| Ok(())).unwrap();
        let late = identity_message(&mut session.lock_recover(), connection, "Too late", 5_010);
        read.observe(session, &controller.state, &late, 5_010);
        assert!(controller.status().device_name.is_none());
        read.start_once(session, 0, 5_011, 2, |_, _| {
            panic!("timeout must not retry")
        })
        .unwrap();
    }
}
