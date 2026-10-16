use super::*;

impl SessionRuntime {
    /// Creates an empty, closed owner. `now_ms` seeds the transport's clock.
    pub fn new(now_ms: u64) -> Self {
        Self {
            state: DeviceStateRuntime::new(),
            transport: TransportRuntime::new(now_ms),
            lifecycle: None,
            connection: 0,
            open: false,
            next_observation: 1,
            next_state_revision: 1,
            publications: crate::device::publication::StatePublications::default(),
            next_operation: 1,
            next_verification: 1,
            report_epoch: 0,
            report_fence: None,
            preset_observation: None,
            metadata_pending: None,
            metadata_active: None,
            metadata_latest: None,
            deliveries: DeliveryJournal::default(),
            backup: None,
            last_backup_operation: 0,
            next_workflow: 1,
            preset_preparation: None,
            preset_workflow: None,
            gateway_verifications: HashMap::new(),
            catalog_observations: catalog_observations::CatalogObservations::default(),
            library_catalog: library_catalog::LibraryCatalog::default(),
            suspension: suspension::SuspensionTracker::default(),
            cor_os_version: None,
        }
    }

    /// Registers a newly opened native connection.
    ///
    /// Stops outstanding deliveries, advances to a fresh connection identity, clears all session state and returns that identity. Fails if the connection counter is exhausted.
    pub fn opened(&mut self, now_ms: u64) -> Result<u64, String> {
        self.deliveries
            .stop_all(DeliveryStop::SessionChanged, now_ms);
        self.connection = next_identity(self.connection, u64::MAX, "QC connection")?;
        self.reset_state();
        self.transport.transport_opened(now_ms);
        self.open = true;
        self.next_observation = 1;
        self.next_state_revision = 1;
        Ok(self.connection)
    }

    /// The CorOS version the unit stated in an update request during this session, or `None`
    /// until it has sent one. The unit sends it only when it checks for an update, so a
    /// session can run for a long time without it.
    pub fn observed_cor_os_version(&self) -> Option<&str> {
        self.cor_os_version.as_deref()
    }

    /// Returns `true` if `connection` is the currently open native connection.
    pub fn accepts_connection(&self, connection: u64) -> bool {
        self.open && self.connection == connection
    }

    /// Clears all session-scoped state, including the lifecycle driver, and marks the owner closed.
    pub fn reset_state(&mut self) {
        self.clear_session_state();
        self.open = false;
    }

    pub(super) fn clear_session_state(&mut self) {
        self.clear_lifecycle_scoped_state(DeliveryWipe::Reset);
        // Only a full session reset gives up the lifecycle driver itself and
        // the fence gating repeated session-identity reports; the narrower
        // wipes below keep both so the same lifecycle can keep reporting
        // (and `expect("validated lifecycle")` elsewhere keeps holding).
        self.lifecycle = None;
        self.report_fence = None;
    }

    /// The ~9 lines every "wipe this session's scoped state" path shares:
    /// staged publications, the preset workflow, the backup snapshot, both
    /// catalogs, in-flight gateway verifications, queued deliveries, the
    /// projected state snapshot, the pending preset observation, and deferred
    /// metadata. Callers differ only in how outstanding deliveries are
    /// stopped -- `DeliveryJournal` has no single method covering both, so
    /// that choice is threaded through explicitly rather than duplicated.
    pub(super) fn clear_lifecycle_scoped_state(&mut self, deliveries: DeliveryWipe) {
        self.publications.clear();
        self.invalidate_preset_workflow("QC session was reset or replaced".into());
        self.backup = None;
        self.catalog_observations.clear();
        self.library_catalog.clear();
        self.gateway_verifications.clear();
        match deliveries {
            DeliveryWipe::Reset => self.deliveries.reset(),
            DeliveryWipe::StopAll(now_ms) => {
                self.deliveries
                    .stop_all(DeliveryStop::SessionChanged, now_ms);
            }
        }
        self.state.reset();
        self.seed_model_preset_loss();
        self.preset_observation = None;
        self.clear_metadata();
        // A suspension belongs to the session that observed it.
        self.suspension.session_ended();
        // The version belongs to the unit that stated it in this session.
        self.cor_os_version = None;
    }

    /// Select and install a handshake attempt as one owner transition. A retry
    /// keeps the native connection and transport retry budget, but none of the
    /// old lifecycle's observations, catalogs, work or completion registries.
    pub fn next_handshake_write(
        &mut self,
        now_ms: u64,
        session_id: impl Into<String>,
        layout: Option<ReportLayout>,
    ) -> Option<HandshakeWrite> {
        if !self.open {
            return None;
        }
        let write = match layout {
            Some(layout) => self
                .transport
                .next_handshake_write_with_layout(now_ms, session_id, layout),
            None => self.transport.next_handshake_write(now_ms, session_id),
        }?;
        self.deliveries
            .stop_all(DeliveryStop::SessionChanged, now_ms);
        self.clear_session_state();
        self.lifecycle = Some(SessionDriver::start(
            write.request_id(),
            write.session_id().to_string(),
            now_ms,
        ));
        Some(write)
    }

    /// Records a native disconnect.
    ///
    /// Stops outstanding deliveries, informs the transport (`reconnect` says whether a reconnect should follow) and clears session state.
    pub fn disconnect(&mut self, now_ms: u64, reconnect: bool) {
        self.deliveries
            .stop_all(DeliveryStop::SessionChanged, now_ms);
        self.transport.disconnect(now_ms, reconnect);
        self.reset_state();
    }

    /// Asks the transport to reconnect, stopping outstanding deliveries and clearing session state.
    pub fn request_reconnect(&mut self, now_ms: u64) {
        self.deliveries
            .stop_all(DeliveryStop::SessionChanged, now_ms);
        self.transport.request_reconnect(now_ms);
        self.reset_state();
    }

    /// Captures an [`ObservationTicket`] for the current session without ingress timing.
    ///
    /// Fails if `connection` is not the open native connection.
    pub fn capture_observation(&mut self, connection: u64) -> Result<ObservationTicket, String> {
        self.capture_observation_with_ingress(connection, None)
    }

    /// Captures the accepted-observation boundary without creating an observation.
    /// Hosts must hold the owner mutex when capturing or comparing this boundary.
    pub fn observation_fence(&self) -> Result<ObservationTicket, String> {
        let fence = self.session_fence()?;
        Ok(ObservationTicket {
            connection: fence.connection,
            epoch: Some(fence.epoch),
            sequence: self.next_observation.saturating_sub(1),
            ingress: None,
        })
    }

    /// Qualifies evidence against the actual native start of this operation.
    /// Missing receipts/timing, partial frames begun before dispatch, stale sessions,
    /// and same-millisecond timing are conservatively not confirming evidence.
    pub fn operation_observation_is_fresh(
        &self,
        operation: &OperationTicket,
        observation: &ObservationTicket,
    ) -> bool {
        let Some((boundary, started_at_ms)) =
            self.deliveries.operation_observation_start(operation)
        else {
            return false;
        };
        self.accepts_observation(observation)
            && boundary.same_session_as(observation)
            && observation.sequence > boundary.sequence
            && observation.ingress.is_some_and(|ingress| {
                ingress.first_report_at_ms > started_at_ms
                    && ingress.first_report_at_ms <= ingress.last_report_at_ms
            })
    }

    /// Stamp a native read under the owner before host buffering or decoding.
    /// One compact identity is reused throughout an epoch; no per-report map or
    /// serialization is needed. Zero denotes an inactive or replaced reader.
    pub fn capture_report_epoch(&mut self, connection: u64) -> Result<u64, String> {
        if !self.accepts_connection(connection) {
            return Ok(0);
        }
        let Ok(fence) = self.session_fence() else {
            return Ok(0);
        };
        if self.report_fence.as_ref() != Some(&fence) {
            self.report_epoch =
                next_identity(self.report_epoch, i64::MAX as u64, "QC native ingress")?;
            self.report_fence = Some(fence);
        }
        Ok(self.report_epoch)
    }

    /// Everything this connection received and could not act on, both
    /// halves merged: the transport's envelope outcomes and trailer tally,
    /// and the state decoder's dispatch outcomes. See
    /// [`crate::inbound_telemetry`].
    pub fn inbound_telemetry(&self) -> InboundTelemetrySnapshot {
        let mut merged = self.transport.inbound_telemetry().clone();
        merged.merge(self.state.inbound_telemetry());
        merged.snapshot()
    }

    /// Moves whenever either half recorded something. A host compares this
    /// against the value it last published so that an idle link never pays
    /// for copying the tally.
    pub fn inbound_telemetry_revision(&self) -> u64 {
        self.transport
            .inbound_telemetry()
            .revision()
            .saturating_add(self.state.inbound_telemetry().revision())
    }

    /// Feeds one raw native report into the transport.
    ///
    /// Returns `Ok(None)` when the connection or report epoch is stale, or when the report does not yet complete an inbound frame. Otherwise returns the completed frame together with its [`ObservationTicket`]. Fails if the report is not a valid frame.
    pub fn push_native_report(
        &mut self,
        connection: u64,
        report_epoch: u64,
        report: &[u8],
        received_at_ms: u64,
    ) -> Result<Option<(crate::device::transport::InboundFrame, ObservationTicket)>, String> {
        if !self.accepts_connection(connection)
            || report_epoch == 0
            || self.report_epoch != report_epoch
            || !self
                .report_fence
                .as_ref()
                .is_some_and(|fence| self.accepts_fence(fence))
        {
            return Ok(None);
        }
        let pushed = self
            .transport
            .push_report_in_epoch(report, received_at_ms, report_epoch);
        // Whatever the transport dropped is known here, so the model preset fold is told now:
        // before the next frame and before any write is planned, with no later traffic needed.
        let synced = self.sync_model_preset_loss(connection);
        let frame = match pushed {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                synced?;
                return Ok(None);
            }
            Err(error) => {
                let error = format!("invalid QC frame: {error}");
                return Err(match synced {
                    Ok(()) => error,
                    Err(publish) => format!("{error}; {publish}"),
                });
            }
        };
        synced?;
        let observation = self.capture_observation_with_ingress(connection, frame.ingress)?;
        Ok(Some((frame, observation)))
    }

    /// Captures an [`ObservationTicket`] carrying the given native ingress timing.
    ///
    /// Fails if `connection` is not the open native connection.
    pub fn capture_observation_with_ingress(
        &mut self,
        connection: u64,
        ingress: Option<IngressTime>,
    ) -> Result<ObservationTicket, String> {
        if !self.accepts_connection(connection) {
            return Err("stale QC native reader".into());
        }
        let sequence = allocate_next_id(&mut self.next_observation, "QC observation")?;
        Ok(ObservationTicket {
            connection,
            epoch: self.lifecycle.as_ref().map(SessionDriver::epoch),
            sequence,
            ingress,
        })
    }

    /// Returns `true` if the ticket was issued by this owner for the current connection and lifecycle epoch.
    pub fn accepts_observation(&self, ticket: &ObservationTicket) -> bool {
        self.accepts_connection(ticket.connection)
            && ticket.sequence > 0
            && ticket.sequence < self.next_observation
            && ticket.epoch.as_ref().is_some_and(|epoch| {
                self.lifecycle
                    .as_ref()
                    .is_some_and(|lifecycle| lifecycle.matches_epoch(epoch))
            })
    }

    /// A current response is useful only to the session that began the wait.
    /// Call under the same owner lock as any resulting cache update or completion.
    pub fn accepts_observation_for(
        &self,
        fence: &SessionFence,
        ticket: &ObservationTicket,
    ) -> bool {
        self.accepts_fence(fence)
            && ticket.connection == fence.connection
            && ticket.epoch.as_ref() == Some(&fence.epoch)
            && self.accepts_observation(ticket)
    }

    /// Registers a write-verification runtime under `context` and returns its identity.
    ///
    /// Fails if the session has changed or if 128 verifications are already active. The context's overall deadline, if any, restricts the runtime.
    pub fn begin_gateway_verification(
        &mut self,
        context: &DispatchContext,
        runtime: GatewayVerificationRuntime,
    ) -> Result<u64, String> {
        let id = self.next_verification;
        let next = next_identity(id, i64::MAX as u64, "QC write verification")?;
        self.start_gateway_verification(id, context, runtime)?;
        self.next_verification = next;
        Ok(id)
    }

    pub(super) fn start_gateway_verification(
        &mut self,
        id: u64,
        context: &DispatchContext,
        mut runtime: GatewayVerificationRuntime,
    ) -> Result<(), String> {
        if !self.accepts_fence(context.session_fence()) {
            return Err("QC write verification session changed".into());
        }
        if id == 0 || self.gateway_verifications.contains_key(&id) {
            return Err("QC write verification identity is already active or invalid".into());
        }
        if self.gateway_verifications.len() >= 128 {
            return Err("QC write verification capacity reached".into());
        }
        if let Some(deadline) = context.deadline {
            runtime.restrict_deadline(deadline);
        }
        let preset_scope = if runtime.changes_preset() {
            None
        } else {
            context
                .operands
                .as_ref()
                .and_then(|operands| operands.verification_scope())
        };
        if let Some(scope) = &preset_scope {
            scope.validate(&self.state)?;
        }
        self.gateway_verifications.insert(
            id,
            GatewayVerificationEntry {
                // The mutation may have changed its own operands. Follow-up
                // reads keep its session, without reasserting pre-write values.
                context: DispatchContext {
                    session: context.session.clone(),
                    operands: None,
                    deadline: context.deadline,
                },
                runtime,
                preset_scope,
            },
        );
        Ok(())
    }

    /// Verify against the original session under the same lock as state reduction.
    /// Host observation tokens remain event hints, not native-attempt correlation.
    pub fn advance_gateway_verification(
        &mut self,
        id: u64,
        observation_sequence: u128,
        now_ms: u64,
    ) -> Result<(GatewayVerificationAction, DispatchContext, u64), String> {
        let entry = self
            .gateway_verifications
            .get(&id)
            .ok_or("QC write verification is no longer active")?;
        if !self.accepts_fence(entry.context.session_fence()) {
            self.gateway_verifications.remove(&id);
            return Err("QC write verification session changed".into());
        }
        if let Some(error) = entry
            .preset_scope
            .as_ref()
            .and_then(|scope| scope.validate(&self.state).err())
        {
            self.gateway_verifications.remove(&id);
            return Err(error);
        }
        let parameter = entry
            .runtime
            .parameter_target()
            .and_then(|(row, column, index)| {
                self.state
                    .single_parameter_target_detail(row, column, index)
            });
        let entry = self
            .gateway_verifications
            .get_mut(&id)
            .expect("validated verification");
        let action = entry.runtime.advance(
            Some(self.state.snapshot()),
            parameter.as_ref(),
            observation_sequence,
            now_ms,
        );
        let context = entry.context.clone();
        let remaining = entry.runtime.remaining_ms(now_ms);
        if matches!(
            action,
            GatewayVerificationAction::Verified | GatewayVerificationAction::TimedOut
        ) {
            self.gateway_verifications.remove(&id);
        }
        Ok((action, context, remaining))
    }

    /// Drops the verification with identity `id`. Does nothing if it is not active.
    pub fn cancel_gateway_verification(&mut self, id: u64) {
        self.gateway_verifications.remove(&id);
    }

    /// Read the library reduced solely from this owner's accepted USB frames.
    /// No local state cache, request ID or quiet interval establishes inventory.
    pub fn retained_library_catalog(
        &self,
        fence: &SessionFence,
        kind: crate::device::native_library::LibraryKind,
        key: &str,
        now_ms: u64,
    ) -> Result<Option<crate::responses::LibraryEntries>, String> {
        if !self.accepts_fence(fence) {
            return Err("QC library session changed".into());
        }
        self.library_catalog.retained(
            kind,
            key,
            |ticket| self.accepts_observation_for(fence, ticket),
            now_ms,
        )
    }

    /// Common host projection for preset and capture/IR retained reads.
    pub fn retained_catalog(
        &self,
        fence: &SessionFence,
        method: &str,
        key: &str,
        now_ms: u64,
    ) -> Result<Option<serde_json::Value>, String> {
        if matches!(method, "device.captures" | "device.irs") {
            let target = crate::device::request::plan_retained_library_read(
                method,
                &serde_json::json!({"folder": key}),
            )?;
            self.retained_library_catalog(fence, target.kind, &target.key, now_ms)?
                .map(serde_json::to_value)
                .transpose()
                .map_err(|error| error.to_string())
        } else {
            self.retained_preset_catalog(fence, method, key, now_ms)
        }
    }

    /// Public catalog reads reuse this session's native records. There is no
    /// request-correlated directory reply or completion marker on the wire.
    /// `None` means the requested projection is not yet available; session,
    /// address and native catalog errors remain errors rather than cache misses.
    pub fn retained_preset_catalog(
        &self,
        fence: &SessionFence,
        method: &str,
        key: &str,
        now_ms: u64,
    ) -> Result<Option<serde_json::Value>, String> {
        Ok(self
            .project_retained_preset_catalog(fence, method, key, now_ms)?
            .ok())
    }

    /// Why [`Self::retained_preset_catalog`] returns `Ok(None)` for the same
    /// arguments, with the counters that tell the empty-catalog cases apart.
    /// Errors are the same errors that read would return.
    pub fn preset_catalog_absence(
        &self,
        fence: &SessionFence,
        method: &str,
        key: &str,
        now_ms: u64,
    ) -> Result<PresetCatalogAbsence, String> {
        let reason = self
            .project_retained_preset_catalog(fence, method, key, now_ms)?
            .err()
            .unwrap_or(PresetCatalogAbsenceReason::Available);
        let census = self.retained_setlist_census(fence, method, key, now_ms);
        let catalog = &self.catalog_observations;
        Ok(PresetCatalogAbsence {
            startup_complete: self.startup_complete(),
            known_setlists: census.map(|(count, _)| count),
            setlist_key_listed: (reason == PresetCatalogAbsenceReason::SetlistKeyMissing)
                .then(|| census.map(|(_, listed)| listed))
                .flatten(),
            reason,
            invalidation: catalog.invalidation(),
            file_frames_observed: catalog.file_frames(),
            catalog_changes_observed: catalog.catalog_changes(),
            file_frames_discarded_by_rebuild: catalog.rebuild_discarded_frames(),
            retained_folders: catalog.folder_count(),
            connection: self.connection,
            session_revision: self
                .lifecycle
                .as_ref()
                .map(|lifecycle| lifecycle.epoch().revision()),
            origin_revision: catalog.origin_revision(),
            startup_rebuilds: self.lifecycle.as_ref().map(SessionDriver::startup_rebuilds),
        })
    }

    /// The explanation of a read that can never succeed in this session, or
    /// `None` while waiting could still change the answer. Before staged
    /// startup completes this is always `None` without projecting anything,
    /// so a caller may poll it on every wakeup of the Booting burst.
    pub fn settled_preset_catalog_absence(
        &self,
        fence: &SessionFence,
        method: &str,
        key: &str,
        now_ms: u64,
    ) -> Result<Option<PresetCatalogAbsence>, String> {
        if !self.startup_complete() {
            return Ok(None);
        }
        let absence = self.preset_catalog_absence(fence, method, key, now_ms)?;
        Ok(absence.setlist_key_unknown().then_some(absence))
    }

    /// Whether this session's staged startup reached `Connected`, which ends
    /// the Booting burst that carries the device's one directory publication.
    fn startup_complete(&self) -> bool {
        self.lifecycle
            .as_ref()
            .is_some_and(SessionDriver::is_connected)
    }

    /// Setlist folders in the retained catalog and whether any listing sits
    /// under `key`, or `None` while the retained catalog cannot be read.
    /// A slot read projects the active setlist whatever key it was given, so
    /// that is the key it looks up here too.
    fn retained_setlist_census(
        &self,
        fence: &SessionFence,
        method: &str,
        key: &str,
        now_ms: u64,
    ) -> Option<(usize, bool)> {
        let accepts = |ticket: &ObservationTicket| self.accepts_observation_for(fence, ticket);
        let (catalog, _) = self
            .catalog_observations
            .retained_catalog(accepts, now_ms)?;
        let active = self.state.snapshot();
        let lookup = if method == "device.listPresetSlots" {
            active.setlist_key.as_str()
        } else {
            key
        };
        let listed = catalog.contains_key(lookup.trim_end_matches('/'));
        let mut library = crate::device::PresetLibrary::default();
        for listing in catalog.into_values() {
            library.ingest(listing);
        }
        Some((library.observed_folders().len(), listed))
    }

    /// The one projection behind both the read and its explanation.
    fn project_retained_preset_catalog(
        &self,
        fence: &SessionFence,
        method: &str,
        key: &str,
        now_ms: u64,
    ) -> Result<Result<serde_json::Value, PresetCatalogAbsenceReason>, String> {
        if !self.accepts_fence(fence) {
            return Err("QC catalog session changed".into());
        }
        if !matches!(
            method,
            "device.listPresetFolders" | "device.listPresets" | "device.listPresetSlots"
        ) {
            return Err("Unsupported QC catalog projection".into());
        }
        let active = self.state.snapshot();
        if method == "device.listPresetSlots" {
            if !active
                .setlist_key
                .trim_end_matches('/')
                .eq_ignore_ascii_case(key.trim_end_matches('/'))
            {
                return Err("QC active setlist changed during slot read".into());
            }
            crate::device::PresetLibrary::validate_writable_slot_target(active)?;
        }
        self.state.native_preset_catalog().health()?;
        let accepts = |ticket: &ObservationTicket| self.accepts_observation_for(fence, ticket);
        let Some((catalog, _)) = self.catalog_observations.retained_catalog(accepts, now_ms) else {
            use catalog_observations::RetainedCatalogGap as Gap;
            let reason = match self.catalog_observations.retention_gate(accepts, now_ms) {
                Err(Gap::NoOrigin) if self.catalog_observations.rebuild_discarded_frames() > 0 => {
                    PresetCatalogAbsenceReason::EpochChanged
                }
                Err(Gap::NoOrigin) | Ok(()) => PresetCatalogAbsenceReason::NoCatalogFrame,
                Err(Gap::Invalidated(_)) => PresetCatalogAbsenceReason::Invalidated,
                Err(Gap::OriginNotAccepted) => PresetCatalogAbsenceReason::EpochChanged,
                Err(Gap::IngressPending) => PresetCatalogAbsenceReason::IngressPending,
            };
            return Ok(Err(reason));
        };
        let mut library = crate::device::PresetLibrary::default();
        for listing in catalog.into_values() {
            library.ingest(listing);
        }
        let projected = match method {
            "device.listPresetFolders" => {
                let folders = library.observed_folders();
                return Ok(if folders.is_empty() {
                    Err(PresetCatalogAbsenceReason::NoFolders)
                } else {
                    Ok(serde_json::json!({"folders": folders}))
                });
            }
            "device.listPresets" => library
                .observed_list(key, active)
                .map(serde_json::to_value)
                .transpose(),
            "device.listPresetSlots" => library
                .observed_slots(active)
                .map(serde_json::to_value)
                .transpose(),
            _ => unreachable!(),
        };
        Ok(projected
            .map_err(|error| error.to_string())?
            .ok_or(PresetCatalogAbsenceReason::SetlistKeyMissing))
    }

    /// Admits a new operation in the current session and returns its ticket.
    ///
    /// The ticket's deadline is `timeout_ms` after `now_ms`. Fails if no session is active.
    pub fn plan_operation(
        &mut self,
        now_ms: u64,
        timeout_ms: u64,
    ) -> Result<OperationTicket, String> {
        let lifecycle = self
            .lifecycle
            .as_ref()
            .filter(|lifecycle| self.open && lifecycle.is_active())
            .ok_or("No active QC session for command admission")?;
        if let Some(refusal) = self.suspension_refusal_for(None) {
            return Err(refusal);
        }
        let sequence = allocate_next_id(&mut self.next_operation, "QC operation")?;
        Ok(OperationTicket {
            connection: self.connection,
            epoch: lifecycle.epoch(),
            sequence,
            deadline: crate::device::correlation::RequestDeadline::new(now_ms, timeout_ms),
            operands: None,
            backup: false,
            workflow: None,
        })
    }

    /// Returns the fence of the current active session, or an error if there is none.
    pub fn session_fence(&self) -> Result<SessionFence, String> {
        let lifecycle = self
            .lifecycle
            .as_ref()
            .filter(|lifecycle| self.open && lifecycle.is_active())
            .ok_or("No active QC session for native dispatch")?;
        Ok(SessionFence {
            connection: self.connection,
            epoch: lifecycle.epoch(),
        })
    }

    /// Returns a dispatch context bound to the current session, without operands or deadline.
    pub fn session_context(&self) -> Result<DispatchContext, String> {
        Ok(DispatchContext {
            session: self.session_fence()?,
            operands: None,
            deadline: None,
        })
    }

    /// Caller-visible address/session identity. Ordinary complete-preset reads
    /// do not replace this identity. This is an expected-state operand, not an
    /// authorization capability; permissions remain the host's responsibility.
    pub fn caller_preset_identity(&self) -> Result<String, String> {
        use base64::Engine;
        let snapshot = self
            .current_preset()
            .ok_or("No Quad Cortex preset has been synchronized in this session")?;
        let bytes = serde_json::to_vec(&(
            self.session_fence()?,
            snapshot.setlist_key.trim_end_matches('/'),
            snapshot.preset_position,
            snapshot.address_revision,
        ))
        .map_err(|error| format!("Could not encode QC preset identity: {error}"))?;
        Ok(format!(
            "qc-preset-v1.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        ))
    }

    /// Checks that `expected` matches the identity of the current preset.
    ///
    /// Fails if `expected` is empty, if no preset is synchronized, or if the preset or session has changed since the identity was issued.
    pub fn assert_caller_preset_identity(&self, expected: &str) -> Result<(), String> {
        if expected.is_empty() {
            return Err("A preset identity from current device state is required".into());
        }
        if self.caller_preset_identity()? != expected {
            return Err(
                "The preset or QC session changed. Refresh the device state before retrying".into(),
            );
        }
        Ok(())
    }

    /// Builds a dispatch context for a planned write.
    ///
    /// Captures the write's operands from current state. If `params` carries `expectedPresetIdentity`, it must match the current preset identity.
    pub fn context_for_write(
        &self,
        method: &str,
        params: &serde_json::Value,
        write: &crate::device::request::PlannedWrite,
    ) -> Result<DispatchContext, String> {
        if let Some(expected) = params.get("expectedPresetIdentity") {
            self.assert_caller_preset_identity(expected.as_str().unwrap_or(""))?;
        }
        Ok(DispatchContext {
            session: self.session_fence()?,
            operands: Some(crate::device::operands::DispatchOperands::capture(
                method,
                params,
                write,
                &self.state,
            )?),
            deadline: None,
        })
    }

    /// Like [`context_for_write`](Self::context_for_write), additionally bounding the whole logical write by the method's write budget measured from `started_ms`.
    pub fn context_for_gateway_write(
        &self,
        method: &str,
        params: &serde_json::Value,
        write: &crate::device::request::PlannedWrite,
        started_ms: u64,
    ) -> Result<DispatchContext, String> {
        let mut context = self.context_for_write(method, params, write)?;
        context.deadline = Some(crate::device::correlation::RequestDeadline::new(
            started_ms,
            crate::device::request::gateway_write_budget_ms(method, write)?,
        ));
        Ok(context)
    }

    /// Plans an operation inside `context`'s session.
    ///
    /// The ticket inherits the context's operands and its deadline is `timeout_ms` after `now_ms`, clipped to the context's overall deadline. Fails if the session changed or the ticket would not be admissible.
    pub fn plan_operation_in_context(
        &mut self,
        context: &DispatchContext,
        now_ms: u64,
        timeout_ms: u64,
    ) -> Result<OperationTicket, String> {
        let mut operation = self.plan_operation_for(&context.session, now_ms, timeout_ms)?;
        operation.deadline = context.phase_deadline(now_ms, timeout_ms);
        operation.operands = context.operands.clone();
        self.validate_operation(&operation, now_ms)?;
        Ok(operation)
    }

    /// Binds a write plan to the response of its preflight read.
    ///
    /// Returns a context with the bound operands together with the preflight decision. Fails if the session changed, the logical write budget has expired, the context has no retained write plan, or binding is rejected.
    pub fn bind_write_preflight(
        &self,
        context: &DispatchContext,
        method: &str,
        params: &serde_json::Value,
        response: &serde_json::Value,
        now_ms: u64,
    ) -> Result<
        (
            DispatchContext,
            crate::device::request::GatewayWritePreflightDecision,
        ),
        String,
    > {
        if !self.accepts_fence(context.session_fence()) {
            return Err("QC write session changed during preflight".into());
        }
        if context
            .deadline
            .is_some_and(|deadline| deadline.expired(now_ms))
        {
            return Err("QC logical write budget expired during preflight".into());
        }
        let mut bound = context.clone();
        let operands = bound
            .operands
            .as_mut()
            .ok_or("QC preflight has no retained write plan")?;
        let decision = operands.bind_preflight(method, params, response, &self.state)?;
        Ok((bound, decision))
    }

    /// Admit a plan only in the session that supplied its state and request IDs.
    /// A fresh caller budget cannot rebind an old plan to a replacement device.
    pub fn plan_operation_for(
        &mut self,
        fence: &SessionFence,
        now_ms: u64,
        timeout_ms: u64,
    ) -> Result<OperationTicket, String> {
        if !self.accepts_fence(fence) {
            return Err("QC session changed after operation planning".into());
        }
        self.plan_operation(now_ms, timeout_ms)
    }

    /// Returns `true` if `fence` names the current open connection and an active lifecycle in the same epoch.
    pub fn accepts_fence(&self, fence: &SessionFence) -> bool {
        self.accepts_connection(fence.connection)
            && self.lifecycle.as_ref().is_some_and(|lifecycle| {
                lifecycle.is_active() && lifecycle.matches_epoch(&fence.epoch)
            })
    }

    /// Begins a logical read scope in the current session with a deadline `timeout_ms` after `started_ms`.
    pub fn begin_gateway_read_scope(
        &self,
        started_ms: u64,
        timeout_ms: u64,
    ) -> Result<GatewayReadScope, String> {
        self.begin_gateway_read_scope_for(&self.session_context()?, started_ms, timeout_ms)
    }

    /// A readback belongs to the write's session, even if it starts later.
    /// Pre-write operands are not read preconditions: the mutation may change them.
    pub fn begin_gateway_read_scope_for(
        &self,
        context: &DispatchContext,
        started_ms: u64,
        timeout_ms: u64,
    ) -> Result<GatewayReadScope, String> {
        if !self.accepts_fence(context.session_fence()) {
            return Err("QC session changed before logical read scope".into());
        }
        Ok(GatewayReadScope {
            context: DispatchContext {
                session: context.session.clone(),
                operands: None,
                deadline: context.deadline,
            },
            deadline: context.phase_deadline(started_ms, timeout_ms),
        })
    }

    /// Returns the milliseconds left in `scope`.
    ///
    /// Fails if the scope's session changed or its budget has expired.
    pub fn gateway_read_scope_remaining(
        &self,
        scope: &GatewayReadScope,
        now_ms: u64,
    ) -> Result<u64, String> {
        if !self.accepts_fence(scope.context.session_fence()) {
            return Err("QC logical read session changed".into());
        }
        let remaining = scope.remaining_ms(now_ms);
        if remaining == 0 {
            return Err("QC logical read budget expired".into());
        }
        Ok(remaining)
    }

    /// Prepares a native dispatch for `operation` in the current session.
    ///
    /// `capture_revision` is the model revision a capture retarget was prepared against, if any. The ticket is validated before it is returned.
    pub fn prepare_dispatch(
        &self,
        operation: Option<&OperationTicket>,
        capture_revision: Option<u64>,
        now_ms: u64,
    ) -> Result<NativeDispatchTicket, String> {
        let ticket = NativeDispatchTicket {
            fence: self.session_fence()?,
            operation: operation.cloned(),
            capture_revision,
        };
        self.validate_dispatch(&ticket, false, now_ms)?;
        Ok(ticket)
    }

    /// Registers a delivery receipt for one message of `report_count` native reports.
    ///
    /// Validates `dispatch` first, and for preset-workflow operations also checks that `message_type` is permitted. Fails if the session or admission changed, or the journal is full.
    pub fn prepare_delivery(
        &mut self,
        dispatch: &NativeDispatchTicket,
        message_type: u16,
        report_count: usize,
        now_ms: u64,
    ) -> Result<DeliveryTicket, String> {
        self.validate_dispatch(dispatch, false, now_ms)?;
        if let Some(operation) = dispatch
            .operation
            .as_ref()
            .filter(|op| op.workflow.is_some())
        {
            self.validate_preset_workflow_message(operation, message_type)?;
        }
        self.deliveries.prepare(
            dispatch.clone(),
            dispatch.fence.clone(),
            dispatch.operation.as_ref().map(OperationTicket::identity),
            message_type,
            report_count,
            now_ms,
        )
    }

    /// Marks the delivery as accepted by the writer queue.
    pub fn delivery_admitted(&mut self, ticket: &DeliveryTicket) -> Result<(), String> {
        self.deliveries.admitted(ticket)
    }

    /// Marks report `index` of the delivery as started.
    ///
    /// Reports must start in order and one at a time. If the dispatch is no longer valid, the delivery is stopped as revoked and the validation error is returned.
    pub fn begin_native_report(
        &mut self,
        ticket: &DeliveryTicket,
        index: usize,
        now_ms: u64,
    ) -> Result<(), String> {
        let dispatch = self.deliveries.dispatch(ticket)?;
        if let Err(error) = self.validate_dispatch(dispatch, index > 0, now_ms) {
            let source_changed = dispatch
                .operation
                .as_ref()
                .is_some_and(|operation| self.preset_workflow_source_changed(operation));
            if source_changed {
                self.revoke_preset_workflow_source();
            }
            self.deliveries
                .stop(ticket, DeliveryStop::AdmissionRevoked, now_ms)?;
            return Err(error);
        }
        let workflow_operation = dispatch
            .operation
            .as_ref()
            .filter(|operation| operation.workflow.is_some())
            .cloned();
        // A favorite or pin that is being sent edits the list the next guard would compare
        // against. The unit answers with a partial acknowledgement (measured on CorOS 4.1.0,
        // 2026-10-07: the changed entry alone, within about 10 to 30 ms), not the whole list, and
        // that echo may not arrive at all. The retained list is therefore unconfirmed from the
        // moment the frame is submitted until a complete list is published or read, and a later
        // request that names its expected state fails closed instead of matching a stale list.
        let retained_list_method = dispatch
            .operation
            .as_ref()
            .and_then(|operation| operation.operands.as_ref())
            .and_then(|operands| operands.retained_list_method())
            .map(str::to_owned);
        let observation = self.observation_fence()?;
        self.deliveries
            .begin_report(ticket, index, now_ms, observation)?;
        if let Some(method) = retained_list_method {
            self.state.invalidate_external_preflight(&method);
        }
        if let Some(operation) = workflow_operation {
            self.preset_workflow_native_started(&operation);
        }
        Ok(())
    }

    /// Records the outcome of report `index`.
    ///
    /// If the session has changed in the meantime, the delivery is first stopped as `SessionChanged`.
    pub fn finish_native_report(
        &mut self,
        ticket: &DeliveryTicket,
        index: usize,
        outcome: ReportOutcome,
        now_ms: u64,
    ) -> Result<(), String> {
        if !self.accepts_fence(&self.deliveries.dispatch(ticket)?.fence) {
            self.deliveries
                .stop(ticket, DeliveryStop::SessionChanged, now_ms)?;
        }
        self.deliveries
            .report_finished(ticket, index, outcome, now_ms)
    }

    /// Stops the delivery for `reason`.
    pub fn stop_delivery(
        &mut self,
        ticket: &DeliveryTicket,
        reason: DeliveryStop,
        now_ms: u64,
    ) -> Result<(), String> {
        self.deliveries.stop(ticket, reason, now_ms)
    }

    /// Returns a snapshot of the delivery's record, or an error if its receipt is no longer retained.
    pub fn delivery_record(&self, ticket: &DeliveryTicket) -> Result<DeliveryRecord, String> {
        self.deliveries.record(ticket)
    }

    /// Subscribes to delivery journal changes.
    ///
    /// The receiver gets a unit message whenever any delivery changes. Fails if too many observers are registered.
    pub fn watch_deliveries(&mut self) -> Result<std::sync::mpsc::Receiver<()>, String> {
        self.deliveries.watch()
    }

    /// Reports whether every one of an operation's `frame_count` frames has been delivered.
    ///
    /// Returns `Ok(false)` while any frame is unsubmitted or in progress. Fails if `frame_count` is zero, if any frame was cancelled or is indeterminate, or if more receipts exist than frames planned. Completion describes native delivery, not device acknowledgement.
    pub fn operation_delivery_complete(
        &self,
        operation: &OperationTicket,
        frame_count: usize,
    ) -> Result<bool, String> {
        self.deliveries.operation_complete(operation, frame_count)
    }

    /// Returns the delivery record if it has completed, and an error naming its status otherwise.
    pub fn require_delivery_complete(
        &self,
        ticket: &DeliveryTicket,
    ) -> Result<DeliveryRecord, String> {
        let record = self.delivery_record(ticket)?;
        if record.status != crate::device::delivery::DeliveryStatus::Completed {
            return Err(format!(
                "QC native delivery is {:?}; device effect is not confirmed",
                record.status
            ));
        }
        Ok(record)
    }

    /// Admission expiry bounds the start of a frame, not its transfer. Once
    /// started, finish the fragments unless the session itself is revoked.
    pub fn validate_dispatch(
        &self,
        ticket: &NativeDispatchTicket,
        began: bool,
        now_ms: u64,
    ) -> Result<(), String> {
        if !self.accepts_fence(&ticket.fence) {
            return Err("QC session changed before native report dispatch".into());
        }
        if !began {
            if ticket
                .capture_revision
                .is_some_and(|revision| !self.state.capture_retarget_revision_is_current(revision))
            {
                return Err("QC model changed before native capture retarget dispatch".into());
            }
            if let Some(operation) = &ticket.operation {
                // Another frame of a request that already reached the device.
                if self.deliveries.operation_started(operation) {
                    if let Some(refusal) = self.partial_suspension_refusal(operation) {
                        return Err(refusal);
                    }
                }
                self.validate_operation(operation, now_ms)?;
            }
        }
        Ok(())
    }

    /// Checks that an operation ticket may still be admitted.
    ///
    /// Fails if its deadline expired, if its session or sequence is no longer current, or if its preset-workflow, backup or operand preconditions no longer hold. Operand checks stop once the operation has begun delivering.
    pub fn validate_operation(&self, ticket: &OperationTicket, now_ms: u64) -> Result<(), String> {
        if ticket.deadline.expired(now_ms) {
            return Err("QC command budget expired before native admission".into());
        }
        if !self.accepts_connection(ticket.connection)
            || ticket.sequence == 0
            || ticket.sequence >= self.next_operation
            || !self.lifecycle.as_ref().is_some_and(|lifecycle| {
                lifecycle.is_active() && lifecycle.matches_epoch(&ticket.epoch)
            })
        {
            return Err("QC session changed before command admission".into());
        }
        // Nothing of this request has reached the device yet. Once a frame
        // has, waiting on it is not refused after the fact; further frames
        // are checked at dispatch.
        if !self.deliveries.operation_started(ticket) {
            if let Some(refusal) = self.suspension_refusal_for(Some(ticket)) {
                return Err(refusal);
            }
        }
        if ticket.workflow.is_some() {
            self.validate_preset_workflow_dispatch(ticket)?;
        }
        if ticket.backup {
            self.validate_backup_dispatch(ticket)?;
        }
        if !self.deliveries.operation_started(ticket) {
            if let Some(operands) = &ticket.operands {
                operands.validate(&self.state)?;
            }
        }
        Ok(())
    }

    /// A transport flag can outlive a failed lifecycle until the next host
    /// drain. Readiness must consult the same owner as the failure transition.
    pub fn is_connected(&self) -> bool {
        self.open
            && self.transport.is_connected()
            && self
                .lifecycle
                .as_ref()
                .is_some_and(SessionDriver::is_active)
    }

    /// Returns `true` if the owner is connected, the transport reports synchronized and the lifecycle has reached the connected phase.
    pub fn is_synchronized(&self) -> bool {
        self.is_connected()
            && self.transport.synchronized()
            && self
                .lifecycle
                .as_ref()
                .is_some_and(SessionDriver::is_connected)
    }

    /// This is the current session's observed preset, including catalog/name
    /// reconciliation. It is a cache, not proof of a particular write or a
    /// correlated readback. Metadata reprojection cannot establish provenance.
    pub fn current_preset(&self) -> Option<&crate::device::GatewaySnapshot> {
        let ticket = self.preset_observation.as_ref()?;
        (self.accepts_observation(ticket) && self.state.snapshot().has_preset)
            .then(|| self.state.snapshot())
    }

    /// Internal generation of authoritative type-15 preset publications.
    /// Unlike the serialized state revision, this does not advance for a
    /// tempo, routing, or other unrelated state update.
    pub fn full_preset_revision(&self) -> u64 {
        self.state.snapshot().full_preset_revision
    }

    /// The decoded observation a DecodedSnapshot preflight consumes. Each
    /// guard gets the projection its own decision arm reads: the scene guard
    /// reads the observed preset, the tempo guards read the folded type-33
    /// preconditions. GatewaySnapshot.mode is the FOOTSWITCH mode, whose
    /// domain includes "PRESET" - it cannot answer a PRESET/GLOBAL tempo-mode
    /// comparison, and reusing it would suppress a real write.
    pub fn decoded_preflight_observation(&self, method: &str) -> Option<serde_json::Value> {
        match method {
            // Retained from what the QC pushed (and from replies to reads the
            // caller made on purpose, such as list_favorites); never read here.
            "device.setTempoMode"
            | "device.setGlobalTempo"
            | "device.setFavorite"
            | "device.setModelPinned" => self.state.external_preflight_observation(method),
            _ => self
                .current_preset()
                .and_then(|snapshot| serde_json::to_value(snapshot).ok())
                .map(|mut observation| {
                    // Guards compare scalar facts. The retained model preset list is for
                    // snapshot readers and would be re-serialized on every write.
                    if let Some(object) = observation.as_object_mut() {
                        object.remove("modelPresetCatalog");
                    }
                    observation
                }),
        }
    }

    /// Applies one inbound state message to the decoded device state.
    ///
    /// Returns `Ok(None)` if the ticket is stale. Otherwise returns the observation, with the resulting state updates stamped with the session state version and any new publication staged. Fails if the message cannot be reduced.
    pub fn observe_state(
        &mut self,
        ticket: &ObservationTicket,
        message_type: u16,
        payload: &[u8],
    ) -> Result<Option<crate::device::state_runtime::StateObservation>, String> {
        if !self.accepts_observation(ticket) {
            return Ok(None);
        }
        // Before reduction, so a state frame that fails to reduce cannot hide
        // a suspension or a resume.
        self.suspension.observe(message_type, payload);
        if message_type == crate::profile::MESSAGE_TYPE_UPDATER_FORWARD {
            if let Some(version) = crate::responses::decode_updater_hardware_version(payload) {
                self.cor_os_version = Some(version);
            }
        }
        let previous_address_revision = self.state.snapshot().address_revision;
        let io_was_invalid = !self.state.snapshot().io_settings_error.is_empty();
        let mut observation = match self.state.ingest(message_type, payload) {
            Ok(observation) => observation,
            Err(error) => {
                if message_type == crate::profile::MESSAGE_TYPE_IO_SETTINGS && !io_was_invalid {
                    let reason = self.state.snapshot().io_settings_error.clone();
                    if !reason.is_empty() {
                        let mut update = crate::state::StateUpdate::empty("ioPorts");
                        update.io_ports = Some(Vec::new());
                        update.io_settings_error = Some(reason);
                        let mut states = [update];
                        self.stamp_state_version(&mut states)?;
                        self.stage_state_publication(ticket, &states, None)?;
                    }
                }
                if let Some(update) = self.state.take_failed_preset_invalidation() {
                    self.preset_observation = None;
                    self.invalidate_preset_workflow(format!(
                        "Retained Grid state was invalidated: {error}"
                    ));
                    let mut states = [update];
                    if let Err(publish) = self
                        .stamp_state_version(&mut states)
                        .and_then(|()| self.stage_state_publication(ticket, &states, None))
                    {
                        return Err(format!("{error}; {publish}"));
                    }
                }
                if message_type == crate::profile::MESSAGE_TYPE_FILE
                    && self.state.native_preset_catalog().health().is_err()
                {
                    self.invalidate_preset_workflow(format!(
                        "Native catalog could not be reduced: {error}"
                    ));
                }
                if message_type == crate::profile::MESSAGE_TYPE_MODEL_PRESET {
                    // The decoder has already invalidated the fold, and no update came out of the
                    // failed message. Publish the invalidation, or no subscriber would learn of it.
                    let update = self.state.model_preset_catalog_update();
                    if let Err(publish) = self.publish_model_preset_catalog(ticket, update) {
                        return Err(format!("{error}; {publish}"));
                    }
                }
                return Err(error);
            }
        };
        self.stamp_state_version(&mut observation.states)?;
        if let Some(message) = &observation.file_message {
            self.catalog_observations.observe(
                ticket,
                message,
                &observation.native_catalog_change,
                self.state.native_preset_catalog(),
            );
        }
        if message_type == crate::profile::MESSAGE_TYPE_RECALL_PRESET
            && observation
                .states
                .iter()
                .any(|state| state.kind == "preset")
        {
            self.preset_observation = Some(ticket.clone());
            self.observe_preset_workflow_snapshot(ticket);
        }
        if message_type == crate::profile::MESSAGE_TYPE_SETLIST_POSITION
            && observation
                .states
                .iter()
                .any(|state| state.kind == "position")
        {
            self.observe_preset_workflow_position(ticket);
        }
        if observation.file_message.is_some() {
            self.observe_preset_workflow_catalog(ticket, &observation);
        }
        self.observe_preset_preparation(ticket, message_type, &observation);
        self.state.snapshot_mut().preset_identity = self.caller_preset_identity().ok();
        let snapshot = self.state.snapshot();
        // Catalog renames can change the address without a type-13 event.
        if snapshot.address_revision != previous_address_revision
            && !observation
                .states
                .iter()
                .any(|state| state.kind == "position")
        {
            observation
                .states
                .push(crate::state::StateUpdate::empty("position"));
        }
        for state in &mut observation.states {
            state.state_version = snapshot.state_version.clone();
            if state.kind == "position" || state.kind == "preset" {
                state.setlist_key = Some(snapshot.setlist_key.clone());
                state.position = Some(snapshot.preset_position);
                state.preset_identity = snapshot.preset_identity.clone();
            }
        }
        self.stage_state_publication(ticket, &observation.states, observation.tempo_clock.clone())?;
        Ok(Some(observation))
    }

    /// Starts the model preset fold's loss baseline at the transport's current total. Called
    /// wherever the decoder is reset, before any ingress can reach the new fold.
    fn seed_model_preset_loss(&mut self) {
        let total = self
            .transport
            .inbound_telemetry()
            .loss_for_type(crate::profile::MESSAGE_TYPE_MODEL_PRESET);
        self.state.seed_model_preset_loss(total);
    }

    /// Feeds the transport's loss total for model preset messages to the fold and publishes the
    /// invalidation when that was the first loss it saw.
    ///
    /// Called from `push_native_report`, the one ingress both hosts use (Windows
    /// `usb.rs read_message_poll`, Android `nativePushReport`), so the fold is invalidated
    /// synchronously when the transport loses data, and every later write is planned against the
    /// invalidated fold under the same owner lock. A counted loss therefore does not leave a
    /// stale admission waiting for some later frame. That is the owner-visible window only: a
    /// report still queued below the owner, a half-received message and the gap between the
    /// owner's admission and the USB write are not counted yet. The publication staged here
    /// still depends on the host draining publications; the retained snapshot and the planner
    /// do not.
    ///
    /// This is the coarse policy of the first version: any loss counted in the session
    /// invalidates the fold until the session ends, with no exemption for the attach, because
    /// the boot update that carries the list arrives only once and cannot be requested again,
    /// and a later append cannot prove the list whole. A report dropped without a known type
    /// cannot be shown not to have been a model preset message. The baseline is the tally
    /// when the decoder was reset (`seed_model_preset_loss`), so a healthy attach starts at a
    /// known total; if the transport counts harmless attach reports as loss, the fold is
    /// disabled for that session.
    fn sync_model_preset_loss(&mut self, connection: u64) -> Result<(), String> {
        let total = self
            .transport
            .inbound_telemetry()
            .loss_for_type(crate::profile::MESSAGE_TYPE_MODEL_PRESET);
        let Some(update) = self.state.observe_model_preset_loss(total) else {
            return Ok(());
        };
        let ticket = self.capture_observation_with_ingress(connection, None)?;
        self.publish_model_preset_catalog(&ticket, update)
    }

    /// Stamps and stages a model preset projection that no observed message carried.
    fn publish_model_preset_catalog(
        &mut self,
        ticket: &ObservationTicket,
        update: crate::state::StateUpdate,
    ) -> Result<(), String> {
        let mut states = [update];
        self.stamp_state_version(&mut states)?;
        self.stage_state_publication(ticket, &states, None)
    }

    fn clear_metadata(&mut self) {
        self.metadata_pending = None;
        self.metadata_active = None;
        self.metadata_latest = None;
    }

    /// Returns `true` if staged state publications are waiting to be taken.
    pub fn state_publications_ready(&self) -> bool {
        self.publications.ready()
    }

    /// The host has one delivery worker. It rechecks each observation under its
    /// publication/reset fence after draining; a returned batch may outlive USB.
    pub fn take_state_publications(
        &mut self,
    ) -> Result<Vec<crate::device::publication::StatePublication>, String> {
        self.publications.take()
    }

    fn stage_state_publication(
        &mut self,
        ticket: &ObservationTicket,
        states: &[crate::state::StateUpdate],
        tempo_clock: Option<crate::responses::TempoClock>,
    ) -> Result<(), String> {
        if states.is_empty() && tempo_clock.is_none() {
            return Ok(());
        }
        let observed_at = crate::time::unix_time_ms();
        let publication = crate::device::publication::StatePublication {
            observation: ticket.clone(),
            observed_at,
            states: states.to_vec(),
            tempo_clock,
        };
        if let Err(error) = self.publications.push(publication) {
            // Loss of a delta cannot masquerade as an intact state stream. Fail
            // this lifecycle so both hosts enter their existing recovery path.
            if let Some(lifecycle) = &mut self.lifecycle {
                lifecycle.transport_failed();
            }
            self.invalidate_preset_workflow(error.clone());
            self.state.reset();
            self.seed_model_preset_loss();
            self.preset_observation = None;
            self.clear_metadata();
            return Err(error);
        }
        Ok(())
    }

    /// Order reductions under the owner lock, including deferred metadata.
    /// Ingress ticket sequence is unsuitable: a metadata parse may finish after
    /// later wire observations have already changed the projected snapshot.
    fn stamp_state_version(
        &mut self,
        states: &mut [crate::state::StateUpdate],
    ) -> Result<(), String> {
        use base64::Engine;
        const MAX_SAFE_REVISION: u64 = 9_007_199_254_740_991;
        if self.next_state_revision > MAX_SAFE_REVISION {
            return Err("QC native state revision exhausted; reconnect the session".into());
        }
        let fence = serde_json::to_vec(&self.session_fence()?)
            .map_err(|error| format!("Could not encode QC state session: {error}"))?;
        let version = crate::generated_payloads::NativeStateVersion {
            session: format!(
                "qc-state-v1.{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(fence)
            ),
            revision: self.next_state_revision,
        };
        self.next_state_revision += 1;
        for state in states {
            state.state_version = Some(version.clone());
            // The projection names its own session, so it can be checked wherever it is copied.
            if let Some(catalog) = &mut state.model_preset_catalog {
                catalog.session.clone_from(&version.session);
            }
        }
        let snapshot = self.state.snapshot_mut();
        if let Some(catalog) = &mut snapshot.model_preset_catalog {
            catalog.session.clone_from(&version.session);
        }
        snapshot.state_version = Some(version);
        Ok(())
    }

    /// ModelRepo is a full snapshot. Keep the parse already in flight and the
    /// newest pending observation; never retain one payload per host wakeup.
    pub fn queue_model_repo(&mut self, ticket: ObservationTicket, payload: Vec<u8>) -> bool {
        if payload.len()
            > crate::compression::max_delivered_payload_bytes_for(
                crate::profile::MESSAGE_TYPE_MODEL_REPO,
            )
            || !self.accepts_observation(&ticket)
        {
            return false;
        }
        if self.metadata_latest.as_ref().is_some_and(|latest| {
            latest.connection == ticket.connection
                && latest.epoch == ticket.epoch
                && latest.sequence >= ticket.sequence
        }) {
            return false;
        }
        self.metadata_latest = Some(ticket.clone());
        self.metadata_pending = Some(ModelRepoWork { ticket, payload });
        true
    }

    /// Returns `true` if no ModelRepo parse is in flight and a still-current payload is waiting.
    pub fn model_repo_ready(&self) -> bool {
        self.metadata_active.is_none()
            && self
                .metadata_pending
                .as_ref()
                .is_some_and(|work| self.accepts_observation(&work.ticket))
    }

    /// Takes the pending ModelRepo payload for parsing.
    ///
    /// Returns `None` if a parse is already in flight, nothing is pending, or the pending payload is stale. The returned work becomes the active parse.
    pub fn take_model_repo(&mut self) -> Option<ModelRepoWork> {
        if self.metadata_active.is_some() {
            return None;
        }
        let work = self.metadata_pending.take()?;
        if !self.accepts_observation(&work.ticket) {
            return None;
        }
        self.metadata_active = Some(work.ticket.clone());
        Some(work)
    }

    /// Completes the active ModelRepo parse with its `catalog` result and installs it.
    ///
    /// Returns `Ok(None)` if `ticket` is not the active parse. Otherwise behaves like [`install_model_catalog`](Self::install_model_catalog).
    pub fn complete_model_repo(
        &mut self,
        ticket: &ObservationTicket,
        catalog: Result<crate::state::ModelCatalog, String>,
        now_ms: u64,
    ) -> Result<Option<Vec<crate::state::StateUpdate>>, String> {
        if self.metadata_active.as_ref() != Some(ticket) {
            return Ok(None);
        }
        self.metadata_active = None;
        self.install_model_catalog(ticket, catalog, now_ms)
    }

    /// Installs a parsed model catalog into the lifecycle and device state.
    ///
    /// Returns `Ok(None)` if the ticket is stale and the resulting state updates otherwise. Fails if the catalog result is an error or the lifecycle rejects it; the latter also clears the session-scoped state.
    pub fn install_model_catalog(
        &mut self,
        ticket: &ObservationTicket,
        catalog: Result<crate::state::ModelCatalog, String>,
        now_ms: u64,
    ) -> Result<Option<Vec<crate::state::StateUpdate>>, String> {
        if !self.accepts_observation(ticket) {
            return Ok(None);
        }
        let lifecycle = self.lifecycle.as_mut().expect("validated lifecycle");
        lifecycle.observe_model_catalog(catalog.as_ref().map_err(|_| ()), now_ms);
        // `observe_model_catalog` fails closed on a parsed-but-fatally-empty
        // catalog the same way it fails on a genuine parse error (see
        // `parsed_catalog_failure_matches_the_wire_failure_only_during_building`
        // in initialization.rs). `catalog` itself is still `Ok(_)` in that
        // case, so it must not be forwarded as the "not ready yet" `Ok(None)`
        // signal below -- callers need `Err` for both failure shapes alike.
        let failure_message = lifecycle.failure().map(|failure| format!("{failure:?}"));
        if let Some(failure_message) = failure_message {
            self.clear_lifecycle_scoped_state(DeliveryWipe::StopAll(now_ms));
            return match catalog {
                Err(message) => Err(message),
                Ok(_) => Err(format!(
                    "Model repository catalog installation failed: {failure_message}"
                )),
            };
        }
        let mut states = self.state.install_model_catalog(catalog?);
        self.stamp_state_version(&mut states)?;
        self.stage_state_publication(ticket, &states, None)?;
        Ok(Some(states))
    }

    /// Feeds one inbound message to the lifecycle driver.
    ///
    /// Returns `false` if the ticket is stale. If the lifecycle changes epoch or stops being active, session-scoped state is cleared. File messages are also reduced into the retained library catalog.
    pub fn observe_lifecycle(
        &mut self,
        ticket: &ObservationTicket,
        message_type: u16,
        payload: &[u8],
        now_ms: u64,
    ) -> bool {
        if !self.accepts_observation(ticket) {
            return false;
        }
        let lifecycle = self.lifecycle.as_mut().expect("validated lifecycle");
        let before = lifecycle.epoch();
        let before_phase = lifecycle.phase();
        lifecycle.observe(message_type, payload, now_ms);
        if lifecycle.epoch() != before || !lifecycle.is_active() {
            let rebuilt = lifecycle.epoch() != before;
            let mut catalog = std::mem::take(&mut self.catalog_observations);
            self.clear_lifecycle_scoped_state(DeliveryWipe::StopAll(now_ms));
            if rebuilt {
                // The same wipe, but keep count of what the rebuild discarded
                // so an empty catalog can say so (diagnostics only).
                catalog.clear_for_rebuild();
                self.catalog_observations = catalog;
            }
        } else {
            if before_phase == crate::device::initialization::DeviceStartupPhase::Booting
                && lifecycle.is_connected()
            {
                // Release on the same ingress lane that stages File records.
                self.library_catalog.release(ticket);
            }
            if message_type == crate::profile::MESSAGE_TYPE_FILE {
                // Windows projects File state on a deferred worker. Reduce the
                // authoritative library here, in native ingress order, before a
                // later Updater frame can release the startup buffer. Deferred
                // state projection must not apply the message a second time.
                match crate::state::decode_file_message(payload) {
                    Ok(message) => {
                        // The reducer stores failures per provider; retained
                        // reads report them without failing unrelated startup.
                        let _ = self.library_catalog.observe(ticket, &message);
                    }
                    Err(error) => self.library_catalog.invalidate(error.to_string()),
                }
            }
        }
        true
    }
}
