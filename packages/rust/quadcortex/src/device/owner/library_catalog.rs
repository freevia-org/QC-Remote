//! The session owner's sole capture/IR catalog, paired with ingress provenance.
//! Local DeviceStateRuntime projections cannot install records in this store.
use super::ObservationTicket;
use crate::device::native_library::{LibraryKind, NativeLibraryCatalog};
use crate::proto::cortex_protobuf_v2 as pa;
use crate::responses::LibraryEntries;
use std::collections::BTreeMap;

struct Origin {
    ticket: ObservationTicket,
    until_ms: u64,
    invalid: bool,
}

#[derive(Default)]
pub(super) struct LibraryCatalog {
    native: NativeLibraryCatalog,
    origins: BTreeMap<LibraryKind, Origin>,
    released: bool,
}

impl LibraryCatalog {
    pub fn clear(&mut self) {
        self.native.clear();
        self.origins.clear();
        self.released = false;
    }

    pub fn invalidate(&mut self, error: String) {
        self.native.invalidate(error);
    }

    pub fn observe(
        &mut self,
        ticket: &ObservationTicket,
        message: &pa::FileMessage,
    ) -> Result<(), String> {
        let changed = if self.released {
            self.native.apply(message)?
        } else {
            self.native.stage_or_apply(message)?
        };
        if !changed {
            return Ok(());
        }
        let Some(kind) = LibraryKind::from_message(message) else {
            return Ok(());
        };
        if !matches!(message.action, 0..=2) {
            return Ok(());
        }
        let Some(pa::file_message::Folder::Folder(folder)) = &message.folder else {
            return Ok(());
        };
        if !matches!(&folder.key, Some(pa::folder_info::Key::Key(key)) if !key.is_empty()) {
            return Ok(());
        }
        self.record_origin(kind, ticket);
        Ok(())
    }

    fn record_origin(&mut self, kind: LibraryKind, ticket: &ObservationTicket) {
        let origin = self.origins.entry(kind).or_insert_with(|| Origin {
            ticket: ticket.clone(),
            until_ms: 0,
            invalid: false,
        });
        origin.invalid |= !origin.ticket.same_session_as(ticket);
        match ticket.ingress {
            Some(time) if time.first_report_at_ms <= time.last_report_at_ms => {
                origin.until_ms = origin.until_ms.max(time.last_report_at_ms);
            }
            _ => origin.invalid = true,
        }
    }

    pub fn release(&mut self, ticket: &ObservationTicket) {
        if self.released {
            return;
        }
        self.released = true;
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            // Reduction errors remain sticky in that provider. They do not
            // suppress a different provider's valid publication.
            if self
                .native
                .publish_staged(kind)
                .is_ok_and(|changed| changed)
            {
                self.record_origin(kind, ticket);
            }
        }
    }

    pub fn retained(
        &self,
        kind: LibraryKind,
        key: &str,
        accepts: impl Fn(&ObservationTicket) -> bool,
        now_ms: u64,
    ) -> Result<Option<LibraryEntries>, String> {
        // Reducer failures remain errors, even if the first publication failed.
        let entries = self.native.folder(kind, key)?;
        let Some(origin) = self.origins.get(&kind) else {
            return Ok(None);
        };
        if origin.invalid || !accepts(&origin.ticket) {
            return Err("QC library publication has invalid session or ingress provenance".into());
        }
        if origin.until_ms > now_ms {
            return Ok(None);
        }
        Ok(entries)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::device::owner::{SessionFence, SessionRuntime};
    use crate::device::session::SessionDriver;
    use crate::device::transport::IngressTime;
    use prost::Message;

    fn owner() -> SessionRuntime {
        let mut owner = SessionRuntime::new(0);
        owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "library-session".into(), 0));
        boot(&mut owner, "library-session", 0, true);
        owner
    }

    fn lifecycle_frame(owner: &mut SessionRuntime, kind: u16, payload: &[u8], now: u64) {
        let ticket = owner
            .capture_observation_with_ingress(
                owner.connection,
                Some(IngressTime {
                    first_report_at_ms: now,
                    last_report_at_ms: now,
                }),
            )
            .unwrap();
        assert!(owner.observe_lifecycle(&ticket, kind, payload, now));
    }

    pub(in crate::device::owner) fn boot(
        owner: &mut SessionRuntime,
        session: &str,
        now: u64,
        finish: bool,
    ) {
        let reset = pa::ResetCommsBuffersMessage {
            session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(
                session.into(),
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let version = pa::VersionMessage {
            action: 1,
            device_type: Some(pa::version_message::DeviceTypeOneOf::DeviceType(
                pa::version_message::DeviceType::Qc as i32,
            )),
            cortex_control_version_valid: Some(
                pa::version_message::CortexControlVersionValid::CortexControlVersionValid(true),
            ),
            ..Default::default()
        }
        .encode_to_vec();
        let models = pa::ModelRepoMessage {
            action: 1,
            model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
                br#"<Models><Category name="Drive"><Model id="1" name="Test" /></Category></Models>"#.to_vec())),
            ..Default::default()
        }.encode_to_vec();
        let stats = pa::ModuleStatsMessage {
            action: 1,
            stats: vec![pa::ModuleStatsItem::default()],
            ..Default::default()
        }
        .encode_to_vec();
        let rebuilding = owner.lifecycle.as_ref().unwrap().phase()
            == crate::device::initialization::DeviceStartupPhase::Disconnected;
        let frames: Vec<_> = [(52, reset), (10, version), (51, models), (35, stats)]
            .into_iter()
            .skip(if rebuilding { 2 } else { 0 })
            .collect();
        assert_eq!(frames.len(), if rebuilding { 2 } else { 4 });
        for (kind, payload) in frames {
            let driver = owner.lifecycle.as_mut().unwrap();
            while let Some(batch) = driver.next_batch(now, now) {
                assert!(driver.dispatched(
                    &batch.ticket,
                    crate::device::session::DispatchOutcome::Accepted
                ));
            }
            lifecycle_frame(owner, kind, &payload, now);
        }
        assert_eq!(
            owner.lifecycle.as_ref().unwrap().phase(),
            crate::device::initialization::DeviceStartupPhase::Booting
        );
        let driver = owner.lifecycle.as_mut().unwrap();
        while let Some(batch) = driver.next_batch(now, now) {
            assert!(driver.dispatched(
                &batch.ticket,
                crate::device::session::DispatchOutcome::Accepted
            ));
        }
        if finish {
            lifecycle_frame(owner, 60, &[8, 1], now);
        }
    }

    fn frame(kind: LibraryKind, key: &str, version: Option<&str>) -> pa::FileMessage {
        pa::FileMessage {
            action: pa::message_action::Enum::Create as i32,
            r#type: Some(pa::file_message::Type::Type(match kind {
                LibraryKind::Capture => 2,
                LibraryKind::Ir => 1,
            })),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key("root".into())),
                name: Some(pa::folder_info::Name::Name("Library".into())),
                files: vec![pa::ProductData {
                    key: Some(pa::product_data::Key::Key(key.into())),
                    name: Some(pa::product_data::Name::Name(key.into())),
                    version: version.map(|v| pa::product_data::Version::Version(v.into())),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    fn observe(owner: &mut SessionRuntime, message: &pa::FileMessage, time: Option<(u64, u64)>) {
        let ticket = owner
            .capture_observation_with_ingress(
                owner.connection,
                time.map(|(first_report_at_ms, last_report_at_ms)| IngressTime {
                    first_report_at_ms,
                    last_report_at_ms,
                }),
            )
            .unwrap();
        assert!(owner.observe_lifecycle(
            &ticket,
            4,
            &message.encode_to_vec(),
            time.map_or(0, |(_, last)| last)
        ));
        assert!(owner
            .observe_state(&ticket, 4, &message.encode_to_vec())
            .unwrap()
            .is_some());
    }

    fn read(
        owner: &SessionRuntime,
        fence: &SessionFence,
        kind: LibraryKind,
        now: u64,
    ) -> Result<Option<LibraryEntries>, String> {
        owner.retained_library_catalog(fence, kind, "root", now)
    }

    #[test]
    fn deferred_projection_cannot_publish_replay_or_invalidate_native_library_rows() {
        let mut owner = SessionRuntime::new(0);
        owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "staged".into(), 0));
        boot(&mut owner, "staged", 0, false);
        let fence = owner.session_fence().unwrap();
        let payload = frame(LibraryKind::Capture, "pending", None).encode_to_vec();
        let ticket = owner
            .capture_observation_with_ingress(
                owner.connection,
                Some(IngressTime {
                    first_report_at_ms: 10,
                    last_report_at_ms: 11,
                }),
            )
            .unwrap();
        // A projection worker alone has no authority to populate the catalog.
        owner.observe_state(&ticket, 4, &payload).unwrap();
        assert!(read(&owner, &fence, LibraryKind::Capture, 11)
            .unwrap()
            .is_none());
        assert!(owner.observe_lifecycle(&ticket, 4, &payload, 11));
        assert!(read(&owner, &fence, LibraryKind::Capture, 11)
            .unwrap()
            .is_none());
        lifecycle_frame(&mut owner, 60, &[8, 1], 12);
        assert_eq!(
            read(&owner, &fence, LibraryKind::Capture, 12)
                .unwrap()
                .unwrap()
                .entries[0]
                .key,
            "pending"
        );
        let mut deletion = frame(LibraryKind::Capture, "pending", None);
        deletion.action = 2;
        observe(&mut owner, &deletion, Some((13, 14)));
        // Simulate a delayed Windows worker completing older File projection.
        owner.observe_state(&ticket, 4, &payload).unwrap();
        assert!(read(&owner, &fence, LibraryKind::Capture, 14)
            .unwrap()
            .unwrap()
            .entries
            .is_empty());
        assert!(owner
            .observe_state(&ticket, 4, &[0x22, 0x02, 0xff])
            .is_err());
        assert!(read(&owner, &fence, LibraryKind::Capture, 14)
            .unwrap()
            .unwrap()
            .entries
            .is_empty());
    }

    #[test]
    fn boot_publication_requires_the_release_frame_and_keeps_its_ingress_time() {
        let mut owner = SessionRuntime::new(0);
        owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "staged".into(), 0));
        boot(&mut owner, "staged", 0, false);
        let fence = owner.session_fence().unwrap();
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            observe(&mut owner, &frame(kind, "pending", None), Some((10, 11)));
            assert!(read(&owner, &fence, kind, 15).unwrap().is_none());
        }
        lifecycle_frame(&mut owner, 35, &[8, 1], 15);
        assert!(read(&owner, &fence, LibraryKind::Capture, 15)
            .unwrap()
            .is_none());
        lifecycle_frame(&mut owner, 60, &[8, 1], 20);
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            assert!(read(&owner, &fence, kind, 19).unwrap().is_none());
            assert_eq!(
                read(&owner, &fence, kind, 20).unwrap().unwrap().entries[0].key,
                "pending"
            );
            let mut deletion = frame(kind, "pending", None);
            deletion.action = 2;
            observe(&mut owner, &deletion, Some((21, 22)));
        }
        lifecycle_frame(&mut owner, 60, &[8, 1], 25);
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            assert!(read(&owner, &fence, kind, 22)
                .unwrap()
                .unwrap()
                .entries
                .is_empty());
            observe(&mut owner, &frame(kind, "live", None), Some((26, 27)));
            assert_eq!(
                read(&owner, &fence, kind, 27).unwrap().unwrap().entries[0].key,
                "live"
            );
        }
    }

    #[test]
    fn boot_release_cannot_repair_missing_staged_or_release_provenance() {
        for invalid_stage in [false, true] {
            let mut owner = SessionRuntime::new(0);
            owner.opened(0).unwrap();
            owner.lifecycle = Some(SessionDriver::start(7, "staged".into(), 0));
            boot(&mut owner, "staged", 0, false);
            let fence = owner.session_fence().unwrap();
            observe(
                &mut owner,
                &frame(LibraryKind::Capture, "pending", None),
                if invalid_stage { None } else { Some((10, 11)) },
            );
            let ticket = owner
                .capture_observation_with_ingress(
                    owner.connection,
                    if invalid_stage {
                        Some(IngressTime {
                            first_report_at_ms: 19,
                            last_report_at_ms: 20,
                        })
                    } else {
                        None
                    },
                )
                .unwrap();
            assert!(owner.observe_lifecycle(&ticket, 60, &[8, 1], 20));
            assert!(read(&owner, &fence, LibraryKind::Capture, 20)
                .unwrap_err()
                .contains("provenance"));
            // The unstaged provider has no dependency on this release frame.
            observe(
                &mut owner,
                &frame(LibraryKind::Ir, "independent", None),
                Some((21, 22)),
            );
            assert_eq!(
                read(&owner, &fence, LibraryKind::Ir, 22)
                    .unwrap()
                    .unwrap()
                    .entries[0]
                    .key,
                "independent"
            );
        }
    }

    #[test]
    fn reconnect_discards_staged_rows_and_requires_a_new_boot_release() {
        let mut owner = SessionRuntime::new(0);
        owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "staged".into(), 0));
        boot(&mut owner, "staged", 0, false);
        observe(
            &mut owner,
            &frame(LibraryKind::Capture, "old", None),
            Some((10, 11)),
        );
        let stale = owner.capture_observation(owner.connection).unwrap();
        owner.opened(12).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 12));
        boot(&mut owner, "replacement", 12, false);
        let fence = owner.session_fence().unwrap();
        observe(
            &mut owner,
            &frame(LibraryKind::Capture, "new", None),
            Some((13, 14)),
        );
        assert!(!owner.observe_lifecycle(&stale, 60, &[8, 1], 15));
        assert!(read(&owner, &fence, LibraryKind::Capture, 15)
            .unwrap()
            .is_none());
        lifecycle_frame(&mut owner, 60, &[8, 1], 16);
        let entries = read(&owner, &fence, LibraryKind::Capture, 16)
            .unwrap()
            .unwrap()
            .entries;
        assert_eq!(
            entries
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>(),
            ["new"]
        );
    }

    #[test]
    fn in_session_rebuild_discards_both_published_and_staged_library_data() {
        for finish_first_boot in [false, true] {
            let mut owner = SessionRuntime::new(0);
            owner.opened(0).unwrap();
            owner.lifecycle = Some(SessionDriver::start(7, "rebuild".into(), 0));
            boot(&mut owner, "rebuild", 0, finish_first_boot);
            let connection = owner.connection;
            let old_fence = owner.session_fence().unwrap();
            observe(
                &mut owner,
                &frame(LibraryKind::Capture, "old", None),
                Some((1, 2)),
            );
            assert_eq!(
                read(&owner, &old_fence, LibraryKind::Capture, 2)
                    .unwrap()
                    .is_some(),
                finish_first_boot
            );
            // The first device disconnect revokes this epoch and its catalog.
            lifecycle_frame(&mut owner, 49, &[16, 0], 3);
            assert_eq!(owner.connection, connection);
            assert!(!owner.accepts_fence(&old_fence));
            boot(&mut owner, "rebuild", 5, false);
            let fence = owner.session_fence().unwrap();
            assert!(read(&owner, &fence, LibraryKind::Capture, 5)
                .unwrap()
                .is_none());
            observe(
                &mut owner,
                &frame(LibraryKind::Capture, "new", None),
                Some((6, 7)),
            );
            assert!(read(&owner, &fence, LibraryKind::Capture, 7)
                .unwrap()
                .is_none());
            lifecycle_frame(&mut owner, 60, &[8, 1], 8);
            let entries = read(&owner, &fence, LibraryKind::Capture, 8)
                .unwrap()
                .unwrap()
                .entries;
            assert_eq!(
                entries
                    .iter()
                    .map(|row| row.key.as_str())
                    .collect::<Vec<_>>(),
                ["new"]
            );
        }
    }

    #[test]
    fn handshake_retry_discards_staged_catalog_and_its_invalid_provenance() {
        use crate::device::transport::ReportLayout;
        for layout in [None, Some(ReportLayout::ReportIdPrefixed)] {
            let mut owner = SessionRuntime::new(0);
            let connection = owner.opened(0).unwrap();
            let first = owner.next_handshake_write(0, "first", layout).unwrap();
            assert_eq!(first.attempt, 1);
            assert_eq!(first.layout, ReportLayout::ReportIdPrefixed);
            let old = owner.capture_observation(connection).unwrap();
            observe(&mut owner, &frame(LibraryKind::Capture, "old", None), None);
            assert!(owner.next_handshake_write(1, "too-early", layout).is_none());
            assert!(owner.accepts_observation(&old));
            let now = crate::profile::HANDSHAKE_ATTEMPT_TIMEOUT_MS;
            let second = owner.next_handshake_write(now, "second", layout).unwrap();
            assert_eq!(second.attempt, 2);
            assert_eq!(second.layout, layout.unwrap_or(ReportLayout::BodyOnly));
            assert!(owner.accepts_connection(connection));
            assert!(!owner.accepts_observation(&old));
            boot(&mut owner, "second", now, false);
            let fence = owner.session_fence().unwrap();
            assert!(read(&owner, &fence, LibraryKind::Capture, now)
                .unwrap()
                .is_none());
            observe(
                &mut owner,
                &frame(LibraryKind::Capture, "new", None),
                Some((now + 1, now + 2)),
            );
            lifecycle_frame(&mut owner, 60, &[8, 1], now + 3);
            let entries = read(&owner, &fence, LibraryKind::Capture, now + 3)
                .unwrap()
                .unwrap()
                .entries;
            assert_eq!(
                entries
                    .iter()
                    .map(|row| row.key.as_str())
                    .collect::<Vec<_>>(),
                ["new"]
            );
        }
    }

    #[test]
    fn public_library_reads_select_native_provider_and_preserve_session_fences() {
        let mut owner = owner();
        let fence = owner.session_fence().unwrap();
        for (kind, folder, item) in [
            (LibraryKind::Capture, "local_nc_root", "capture"),
            (LibraryKind::Ir, "local_ir_root", "ir"),
            (LibraryKind::Ir, "opaque-child", "nested-ir"),
        ] {
            let mut message = frame(kind, item, None);
            let Some(pa::file_message::Folder::Folder(info)) = &mut message.folder else {
                unreachable!()
            };
            info.key = Some(pa::folder_info::Key::Key(folder.into()));
            if folder == "opaque-child" {
                info.parent_key = Some(pa::folder_info::ParentKey::ParentKey(
                    "local_ir_root".into(),
                ));
            }
            owner.state.ingest(4, &message.encode_to_vec()).unwrap();
            let method = if kind == LibraryKind::Capture {
                "device.captures"
            } else {
                "device.irs"
            };
            assert!(owner
                .retained_catalog(&fence, method, folder, 10)
                .unwrap()
                .is_none());
            observe(&mut owner, &message, Some((11, 12)));
            assert!(owner
                .retained_catalog(&fence, method, folder, 11)
                .unwrap()
                .is_none());
            let rows = owner
                .retained_catalog(&fence, method, folder, 12)
                .unwrap()
                .unwrap();
            assert_eq!(rows["entries"][0]["key"], item);
            assert_eq!(rows["entries"].as_array().unwrap().len(), 1);
        }
        assert_eq!(
            owner
                .retained_catalog(&fence, "device.captures", "ignored", 12)
                .unwrap()
                .unwrap()["entries"][0]["key"],
            "capture"
        );
        assert_eq!(
            owner
                .retained_catalog(&fence, "device.irs", "", 12)
                .unwrap()
                .unwrap()["entries"][0]["key"],
            "ir"
        );
        assert!(owner
            .retained_catalog(&fence, "device.irs", "folder/opaque/", 12)
            .unwrap()
            .is_none());
        owner.opened(13).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 13));
        for method in ["device.captures", "device.irs"] {
            assert!(owner
                .retained_catalog(&fence, method, "", 13)
                .unwrap_err()
                .contains("session changed"));
        }
    }

    #[test]
    fn refused_delete_with_a_new_resolver_binding_still_requires_ingress_provenance() {
        let mut owner = owner();
        let fence = owner.session_fence().unwrap();
        let mut factory = frame(LibraryKind::Ir, "unused", None);
        let Some(pa::file_message::Folder::Folder(folder)) = &mut factory.folder else {
            unreachable!()
        };
        folder.files.clear();
        folder.is_factory = Some(pa::folder_info::IsFactory::IsFactory(true));
        observe(&mut owner, &factory, Some((1, 2)));
        assert!(read(&owner, &fence, LibraryKind::Ir, 2).unwrap().is_some());
        factory.action = 2;
        let Some(pa::file_message::Folder::Folder(folder)) = &mut factory.folder else {
            unreachable!()
        };
        folder.is_factory = None;
        // The factory model survives, but its first ordinary resolver lookup
        // binds the node identity. Missing ingress cannot authorize that bind.
        observe(&mut owner, &factory, None);
        assert!(read(&owner, &fence, LibraryKind::Ir, 3)
            .unwrap_err()
            .contains("invalid session or ingress provenance"));
    }

    #[test]
    fn native_ancestor_delete_removes_descendants_only_in_its_provider() {
        let mut owner = owner();
        let fence = owner.session_fence().unwrap();
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            observe(&mut owner, &frame(kind, "top", None), Some((1, 2)));
            let mut child = frame(kind, "nested", None);
            let Some(pa::file_message::Folder::Folder(folder)) = &mut child.folder else {
                unreachable!()
            };
            folder.key = Some(pa::folder_info::Key::Key("child".into()));
            folder.parent_key = Some(pa::folder_info::ParentKey::ParentKey("root".into()));
            observe(&mut owner, &child, Some((3, 4)));
            assert_eq!(
                owner
                    .retained_library_catalog(&fence, kind, "child", 4)
                    .unwrap()
                    .unwrap()
                    .entries[0]
                    .key,
                "nested"
            );
        }
        let mut deletion = frame(LibraryKind::Capture, "unused", None);
        deletion.action = 2;
        let Some(pa::file_message::Folder::Folder(folder)) = &mut deletion.folder else {
            unreachable!()
        };
        folder.files.clear();
        observe(&mut owner, &deletion, Some((5, 6)));
        assert!(owner
            .retained_library_catalog(&fence, LibraryKind::Capture, "child", 6)
            .unwrap()
            .is_none());
        assert_eq!(
            owner
                .retained_library_catalog(&fence, LibraryKind::Ir, "child", 6)
                .unwrap()
                .unwrap()
                .entries[0]
                .key,
            "nested"
        );
    }

    #[test]
    fn native_library_root_delete_keeps_a_readable_empty_container() {
        let mut owner = owner();
        let fence = owner.session_fence().unwrap();
        for (kind, method) in [
            (LibraryKind::Capture, "device.captures"),
            (LibraryKind::Ir, "device.irs"),
        ] {
            let mut message = frame(kind, "remove", None);
            let Some(pa::file_message::Folder::Folder(folder)) = &mut message.folder else {
                unreachable!()
            };
            folder.key = Some(pa::folder_info::Key::Key(kind.root_key().into()));
            observe(&mut owner, &message, Some((1, 2)));
            message.action = 2;
            let Some(pa::file_message::Folder::Folder(folder)) = &mut message.folder else {
                unreachable!()
            };
            folder.files.clear();
            observe(&mut owner, &message, Some((3, 4)));
            assert!(owner
                .retained_catalog(&fence, method, "", 3)
                .unwrap()
                .is_none());
            let result = owner
                .retained_catalog(&fence, method, "", 4)
                .unwrap()
                .unwrap();
            assert!(result["entries"].as_array().unwrap().is_empty());
        }
    }

    #[test]
    fn only_accepted_native_ingress_can_supply_retained_library_rows() {
        let mut owner = owner();
        let fence = owner.session_fence().unwrap();
        let fake = frame(LibraryKind::Capture, "local-fake", Some("99"));
        owner.state.ingest(4, &fake.encode_to_vec()).unwrap();
        assert!(read(&owner, &fence, LibraryKind::Capture, 100)
            .unwrap()
            .is_none());
        observe(
            &mut owner,
            &frame(LibraryKind::Capture, "native-a", Some("2")),
            Some((1, 2)),
        );
        observe(
            &mut owner,
            &frame(LibraryKind::Capture, "native-b", None),
            Some((3, 4)),
        );
        owner.state.ingest(4, &fake.encode_to_vec()).unwrap();
        let rows = read(&owner, &fence, LibraryKind::Capture, 4)
            .unwrap()
            .unwrap()
            .entries;
        assert_eq!(
            rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
            ["native-a", "native-b"]
        );
        assert_eq!(rows[0].version.as_deref(), Some("2"));
        assert_eq!(rows[1].version, None);
        assert_eq!(rows[1].position, None);
        assert!(read(&owner, &fence, LibraryKind::Ir, 4).unwrap().is_none());
        let mut deleted = frame(LibraryKind::Capture, "native-a", None);
        deleted.action = 2;
        observe(&mut owner, &deleted, Some((5, 6)));
        assert_eq!(
            read(&owner, &fence, LibraryKind::Capture, 6)
                .unwrap()
                .unwrap()
                .entries[0]
                .key,
            "native-b"
        );
        assert_eq!(
            read(&owner, &fence, LibraryKind::Capture, 6)
                .unwrap()
                .unwrap()
                .entries
                .len(),
            1
        );
    }

    #[test]
    fn missing_or_reversed_ingress_is_sticky_per_provider_until_session_reset() {
        for invalid in [None, Some((9, 8))] {
            let mut owner = owner();
            let fence = owner.session_fence().unwrap();
            observe(
                &mut owner,
                &frame(LibraryKind::Capture, "uncertain", None),
                invalid,
            );
            assert!(read(&owner, &fence, LibraryKind::Capture, 10)
                .unwrap_err()
                .contains("provenance"));
            observe(
                &mut owner,
                &frame(LibraryKind::Capture, "later", Some("2")),
                Some((11, 12)),
            );
            assert!(read(&owner, &fence, LibraryKind::Capture, 12).is_err());
            observe(
                &mut owner,
                &frame(LibraryKind::Ir, "ir", None),
                Some((13, 14)),
            );
            assert_eq!(
                read(&owner, &fence, LibraryKind::Ir, 14)
                    .unwrap()
                    .unwrap()
                    .entries[0]
                    .key,
                "ir"
            );
            owner.opened(15).unwrap();
            owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 15));
            boot(&mut owner, "replacement", 15, true);
            assert!(read(&owner, &fence, LibraryKind::Capture, 15)
                .unwrap_err()
                .contains("session changed"));
            let current = owner.session_fence().unwrap();
            assert!(read(&owner, &current, LibraryKind::Capture, 15)
                .unwrap()
                .is_none());
            observe(
                &mut owner,
                &frame(LibraryKind::Capture, "new-session", None),
                Some((16, 17)),
            );
            assert_eq!(
                read(&owner, &current, LibraryKind::Capture, 17)
                    .unwrap()
                    .unwrap()
                    .entries[0]
                    .key,
                "new-session"
            );
        }
    }

    #[test]
    fn future_observations_and_old_session_tickets_cannot_satisfy_current_reads() {
        let mut owner = owner();
        let fence = owner.session_fence().unwrap();
        observe(
            &mut owner,
            &frame(LibraryKind::Capture, "future", None),
            Some((10, 20)),
        );
        assert!(read(&owner, &fence, LibraryKind::Capture, 19)
            .unwrap()
            .is_none());
        assert_eq!(
            read(&owner, &fence, LibraryKind::Capture, 20)
                .unwrap()
                .unwrap()
                .entries[0]
                .key,
            "future"
        );
        let stale = owner.capture_observation(owner.connection).unwrap();
        owner.opened(21).unwrap();
        owner.lifecycle = Some(SessionDriver::start(9, "new".into(), 21));
        assert!(owner
            .observe_state(
                &stale,
                4,
                &frame(LibraryKind::Capture, "stale", None).encode_to_vec()
            )
            .unwrap()
            .is_none());
        assert!(read(
            &owner,
            &owner.session_fence().unwrap(),
            LibraryKind::Capture,
            22
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn malformed_native_file_invalidates_both_library_providers() {
        let mut owner = owner();
        let fence = owner.session_fence().unwrap();
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            observe(&mut owner, &frame(kind, "valid", None), Some((1, 2)));
        }
        let ticket = owner.capture_observation(owner.connection).unwrap();
        assert!(owner.observe_lifecycle(&ticket, 4, &[0x22, 0x02, 0xff], 3));
        assert!(owner
            .observe_state(&ticket, 4, &[0x22, 0x02, 0xff])
            .is_err());
        assert!(read(&owner, &fence, LibraryKind::Capture, 3).is_err());
        assert!(read(&owner, &fence, LibraryKind::Ir, 3).is_err());
    }

    #[test]
    fn ignored_factory_folder_delete_does_not_remove_rows_or_poison_provenance() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut owner = owner();
            let fence = owner.session_fence().unwrap();
            observe(&mut owner, &frame(kind, "keep", None), Some((1, 2)));
            let mut deletion = frame(kind, "keep", None);
            deletion.action = 2;
            let Some(pa::file_message::Folder::Folder(folder)) = &mut deletion.folder else {
                unreachable!()
            };
            folder.files.clear();
            folder.is_factory = Some(pa::folder_info::IsFactory::IsFactory(true));
            // This operation is ignored by the recovered handler, so missing
            // ingress on it cannot taint records it did not change.
            observe(&mut owner, &deletion, None);
            assert_eq!(
                read(&owner, &fence, kind, 2).unwrap().unwrap().entries[0].key,
                "keep"
            );
            let Some(pa::file_message::Folder::Folder(folder)) = &mut deletion.folder else {
                unreachable!()
            };
            folder.is_factory = None;
            observe(&mut owner, &deletion, Some((3, 4)));
            assert!(read(&owner, &fence, kind, 4).unwrap().is_none());
        }
    }

    #[test]
    fn preset_capacity_failure_does_not_block_independent_library_publication() {
        let mut owner = owner();
        let fence = owner.session_fence().unwrap();
        for index in 0..=crate::device::native_catalog::MAX_PRESET_FOLDERS {
            let message = pa::FileMessage {
                action: 0,
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key(format!(
                        "/media/p4/Presets/Setlist{index}"
                    ))),
                    ..Default::default()
                })),
                ..Default::default()
            };
            let result = owner.state.ingest(4, &message.encode_to_vec());
            assert_eq!(
                result.is_err(),
                index == crate::device::native_catalog::MAX_PRESET_FOLDERS
            );
        }
        assert!(owner.state.native_preset_catalog().health().is_err());
        observe(
            &mut owner,
            &frame(LibraryKind::Capture, "still-valid", Some("2")),
            Some((1, 2)),
        );
        assert_eq!(
            read(&owner, &fence, LibraryKind::Capture, 2)
                .unwrap()
                .unwrap()
                .entries[0]
                .version
                .as_deref(),
            Some("2")
        );
    }
}
