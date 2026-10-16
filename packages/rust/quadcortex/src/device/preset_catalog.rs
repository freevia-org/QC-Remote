//! Preset mutation predicates over retained native File publications. The owner
//! validates the session; this runtime owns freshness, quiet collection, retry
//! cadence and result reconciliation. No host-supplied success flag is accepted.
//!
//! Host policy (not a Cortex Control frame): the post-mutation verification is
//! passive first (the unit's own File publication, including any pushed after the
//! native attempt began and before this stage started) and then sends at most a
//! few directory reads; see `device::catalog`. The unit answers an on-demand
//! directory read (ESTABLISHED, CorOS 4.1.0, 2026-10-07), so the old doubt is
//! settled; the first listing after a mutation can lag. Timings are BELIEVED, no
//! measurement in the repository.
use crate::device::catalog::{CatalogVerificationAction, CatalogVerificationRuntime};
use crate::device::correlation::RequestDeadline;
use crate::device::native_catalog::NativePresetCatalog;
use crate::device::request::{
    duplicate_setlist_contents_mismatch, PresetMutationPlan, PresetMutationRecord,
};
use crate::device::transport::IngressTime;
use crate::device::{GatewaySnapshot, PresetLibrary};
use crate::profile;
use crate::proto::cortex_protobuf_v2 as pa;
use crate::state::project_preset_folder;

/// The outcome of verifying a preset mutation against the preset catalog observed after the
/// attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct PresetCatalogResolution {
    /// Key of the setlist the result refers to. For a created or duplicated setlist this is the key
    /// of the newly observed folder.
    pub setlist_key: String,
    /// Name of the saved preset or created setlist as observed in the catalog, which may differ
    /// from the requested name.
    pub saved_name: String,
    /// Human-readable description of the verified result.
    pub detail: String,
    /// Records of the saved presets, reconciled with the slots observed in the post-attempt
    /// catalog.
    pub saved_presets: Vec<PresetMutationRecord>,
}

pub(crate) struct PresetMutationCatalog {
    plan: PresetMutationPlan,
    schedule: CatalogVerificationRuntime,
    /// Only a File message whose first report is later than this can be evidence. It is the
    /// moment the native attempt began (not the moment verification began), so a publication
    /// the unit pushed while the host was still settling counts: verification is passive-first
    /// and the unit's own push is its best evidence.
    evidence_after_ms: u64,
    last_sequence: u64,
    native: NativePresetCatalog,
    explicit_folder_deletion: bool,
    explicit_product_deletion: bool,
    last_file_ms: Option<u64>,
    // A directory READ carries no request ID, so inbound File traffic is
    // the only evidence CorOS answered. Until some arrives, a second READ
    // cannot observe anything the first will not; it only queues another
    // full-library dump (~587 frames, ~9.6 s) behind it.
    requested_at_ms: Option<u64>,
    changed: bool,
    overflow: bool,
    mismatch: Option<String>,
    resolution: Option<PresetCatalogResolution>,
}

impl PresetMutationCatalog {
    pub fn new(
        plan: &PresetMutationPlan,
        started_ms: u64,
        observation_floor: u64,
        evidence_after_ms: u64,
        deadline: RequestDeadline,
    ) -> Self {
        let policy = plan
            .folder_copy
            .as_ref()
            .map_or(plan.catalog_policy, |folder| folder.catalog_policy);
        let mut schedule = CatalogVerificationRuntime::with_policy(started_ms, policy);
        schedule.restrict_deadline(deadline);
        Self {
            plan: plan.clone(),
            schedule,
            evidence_after_ms,
            last_sequence: observation_floor,
            native: NativePresetCatalog::default(),
            explicit_folder_deletion: false,
            explicit_product_deletion: false,
            last_file_ms: None,
            requested_at_ms: None,
            changed: false,
            overflow: false,
            mismatch: None,
            resolution: None,
        }
    }

    /// Consume the actual post-attempt envelope. The accumulator starts empty,
    /// so retained pre-attempt values and presentation overlays cannot enter it.
    pub fn observe(
        &mut self,
        sequence: u64,
        ingress: Option<IngressTime>,
        message: &pa::FileMessage,
    ) {
        let Some(ingress) = ingress else { return };
        if sequence <= self.last_sequence
            || ingress.first_report_at_ms <= self.evidence_after_ms
            || ingress.first_report_at_ms > ingress.last_report_at_ms
            || self.schedule.remaining_ms(ingress.last_report_at_ms) == 0
        {
            return;
        }
        self.last_sequence = sequence;
        let change = match self.native.apply(message) {
            Ok(change) => change,
            Err(error) => {
                self.overflow = true;
                // NativePresetCatalog returns host-authored capacity errors,
                // not wire names. Keep the actual exhausted resource.
                self.mismatch = Some(error);
                return;
            }
        };
        let file_type = match message.r#type {
            Some(pa::file_message::Type::Type(value)) => Some(value),
            _ => None,
        };
        let source = match &message.folder {
            Some(pa::file_message::Folder::Folder(folder)) => Some(folder),
            _ => None,
        };
        if let Some(source) = source.filter(|folder| {
            project_preset_folder(folder, file_type)
                .is_some_and(|listing| PresetLibrary::is_preset_setlist_listing(&listing))
        }) {
            let Some(pa::folder_info::Key::Key(key)) = &source.key else {
                unreachable!()
            };
            if message.action == pa::message_action::Enum::Delete as i32 {
                if let Some(deletion) = &self.plan.setlist_deletion {
                    if source.files.is_empty() && same_key(key, &deletion.setlist_key) {
                        self.explicit_folder_deletion = true;
                        self.changed = true;
                    }
                }
                if let Some(deletion) = &self.plan.deletion {
                    if same_key(key, &deletion.setlist_key) && source.files.iter().any(|file| {
                        matches!(&file.key, Some(pa::product_data::Key::Key(key)) if key == &deletion.product_key)
                    }) {
                        self.explicit_product_deletion = true;
                        self.changed = true;
                    }
                }
            }
        }
        if let Some(deletion) = &self.plan.setlist_deletion {
            if self.native.folder(&deletion.setlist_key).is_some() {
                self.explicit_folder_deletion = false;
            }
        }
        if let Some(deletion) = &self.plan.deletion {
            if self
                .native
                .folder(&deletion.setlist_key)
                .is_some_and(|folder| {
                    folder
                        .files
                        .iter()
                        .any(|file| file.product_key.as_deref() == Some(&deletion.product_key))
                })
            {
                self.explicit_product_deletion = false;
            }
        }
        self.changed |= !change.updated.is_empty() || !change.removed.is_empty();
        if self.changed
            || self.native.folders().next().is_some()
            || self.explicit_folder_deletion
            || self.explicit_product_deletion
        {
            self.last_file_ms = Some(ingress.last_report_at_ms);
            // CorOS answered: the outstanding request is discharged.
            self.requested_at_ms = None;
        }
    }

    pub fn take_action(&mut self, now_ms: u64) -> CatalogVerificationAction {
        if self.overflow || self.schedule.remaining_ms(now_ms) == 0 {
            return CatalogVerificationAction::Failed {
                observed_generations: self.schedule.observed_generations(),
            };
        }
        if self.changed {
            // A queued future timestamp cannot complete a generation early.
            let last_file = self.last_file_ms.unwrap_or(now_ms);
            let quiet_until = last_file.saturating_add(if self.plan.needs_directory_catalog() {
                profile::PRESET_LIBRARY_SETTLEMENT_QUIET_MS
            } else {
                0
            });
            if now_ms < quiet_until {
                return CatalogVerificationAction::Wait {
                    delay_ms: quiet_until
                        .saturating_sub(now_ms)
                        .min(self.schedule.remaining_ms(now_ms)),
                };
            }
            match self.resolve() {
                Ok(resolution) => {
                    self.resolution = Some(resolution);
                    self.schedule.generation_observed(now_ms, true);
                }
                Err(mismatch) => {
                    self.mismatch = Some(mismatch);
                    self.schedule.generation_observed(now_ms, false);
                }
            }
            self.changed = false;
        }
        // READ has no generation identifier or completion marker. Sending
        // another request cannot erase already observed ambiguity or make an
        // omitted folder disappear from the evidence.
        let streaming = self.last_file_ms.is_some_and(|last| {
            now_ms < last.saturating_add(profile::PRESET_LIBRARY_SETTLEMENT_QUIET_MS)
        });
        let unanswered = self
            .requested_at_ms
            .is_some_and(|since| now_ms < since.saturating_add(self.schedule.maximum_retry_ms()));
        // Suppress only a repeat REQUEST. A completed or failed verification
        // must still surface immediately.
        if self.schedule.advance(now_ms) == CatalogVerificationAction::Request
            && (streaming || unanswered)
        {
            // Bounded by the ladder's own cooldown, so a dropped request still
            // releases rather than deadlocking the verification budget.
            return CatalogVerificationAction::Wait {
                delay_ms: profile::PRESET_LIBRARY_SETTLEMENT_QUIET_MS
                    .min(self.schedule.remaining_ms(now_ms)),
            };
        }
        let action = self.schedule.take_action(now_ms);
        if action == CatalogVerificationAction::Request {
            self.requested_at_ms = Some(now_ms);
        }
        action
    }

    fn resolve(&self) -> Result<PresetCatalogResolution, String> {
        let mut result = PresetCatalogResolution {
            setlist_key: self.plan.setlist_key.clone(),
            saved_name: self.plan.saved_name.clone(),
            detail: self.plan.detail.clone(),
            saved_presets: self.plan.saved_presets.clone(),
        };
        if self.plan.setlist_deletion.is_some() && !self.explicit_folder_deletion {
            return Err("Setlist deletion pending: no post-attempt DELETE of the target folder was observed".into());
        }
        if let Some(copy) = &self.plan.folder_copy {
            let candidates: Vec<_> = self
                .native
                .folders()
                .filter(|folder| {
                    !folder.is_factory
                        && !copy.before_folder_keys.iter().any(|key| {
                            key.trim_end_matches('/')
                                .eq_ignore_ascii_case(folder.key.trim_end_matches('/'))
                        })
                        && (copy.operation != "create" || folder.name == self.plan.saved_name)
                })
                .collect();
            let [created] = candidates.as_slice() else {
                return Err(format!(
                    "Expected one new setlist key, observed {}",
                    candidates.len()
                ));
            };
            if copy.operation != "create" {
                if created.files.len() != 256 {
                    return Err("Setlist copy pending: all 256 destination slots have not been explicitly observed".into());
                }
                let projection =
                    PresetLibrary::project_list(created, &GatewaySnapshot::default(), vec![]);
                if let Some(mismatch) =
                    duplicate_setlist_contents_mismatch(&copy.expected_presets, &projection)
                {
                    return Err(mismatch);
                }
            }
            result.setlist_key = created.key.clone();
            result.saved_name = created.name.clone();
            result.detail = if copy.operation == "create" {
                format!(
                    "Created setlist {} and verified the fresh catalog",
                    created.name
                )
            } else {
                format!(
                    "Duplicated {} as {} and verified the fresh catalog",
                    copy.source_name, created.name
                )
            };
        }
        if let Some(deletion) = &self.plan.deletion {
            if !self.explicit_product_deletion {
                let listing = self
                    .native
                    .folder(&deletion.setlist_key)
                    .ok_or("No post-attempt records of the deleted preset's setlist")?;
                if listing.files.len() != 256 {
                    return Err("Preset deletion pending: omission from a partial update does not prove absence".into());
                }
                if listing
                    .files
                    .iter()
                    .any(|file| file.product_key.as_deref() == Some(&deletion.product_key))
                {
                    return Err(
                        "The post-attempt catalog still contains the deleted device key".into(),
                    );
                }
            }
        }
        if !result.saved_presets.is_empty() {
            let observed = self
                .native
                .folders()
                .flat_map(|folder| {
                    folder
                        .files
                        .iter()
                        // A new desktop Product model defaults absent instrument
                        // to "" (enum 0). The QC also omits this field in saved
                        // records. Identity/name/slot must still come from these
                        // post-attempt publications; zero is a model default,
                        // not an assertion that a wire field was supplied.
                        .filter(|file| file.product_key.is_some())
                        .map(|file| PresetMutationRecord {
                            setlist_key: folder.key.clone(),
                            position: file.position,
                            name: file.name.clone(),
                            instrument: file.instrument,
                            allow_collision_suffix: false,
                        })
                })
                .collect::<Vec<_>>();
            crate::device::preset_save::reconcile_saved_preset_records(
                &mut result.saved_presets,
                &observed,
            )?;
            if let Some(saved) = result.saved_presets.first() {
                result.saved_name.clone_from(&saved.name);
            }
        }
        Ok(result)
    }

    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.schedule.remaining_ms(now_ms)
    }
    /// Whether verification ended because a retained model or byte capacity was
    /// exceeded. That is a real failure, unlike a window that merely expired.
    pub fn overflowed(&self) -> bool {
        self.overflow
    }
    pub fn resolution(&self) -> Option<&PresetCatalogResolution> {
        self.resolution.as_ref()
    }
    pub fn failure_message(&self) -> String {
        if self.overflow {
            if let Some(reason) = self.mismatch.as_deref() {
                if reason != "Native preset catalog exceeded its folder capacity" {
                    return format!("Preset catalog verification failed: {reason}");
                }
            }
            return "Preset catalog verification exceeded its retained folder capacity".into();
        }
        let mut message = self.schedule.failure_message("preset mutation catalog");
        if let Some(mismatch) = &self.mismatch {
            message.push(' ');
            message.push_str(mismatch);
        }
        message
    }
}

fn same_key(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::catalog::CatalogVerificationPolicy;
    use crate::device::native_catalog::MAX_PRESET_FOLDERS;
    use crate::device::request::{FolderCopyVerification, PresetDeletionVerification};
    use crate::state::{PresetFileListing, PresetFolderListing};

    const KEY: &str = "/media/p4/Presets/Test";
    fn record() -> PresetMutationRecord {
        PresetMutationRecord {
            setlist_key: KEY.into(),
            position: 2,
            name: "Saved".into(),
            instrument: 0,
            allow_collision_suffix: true,
        }
    }
    fn plan() -> PresetMutationPlan {
        PresetMutationPlan {
            stages: vec![],
            cleanup_stages: vec![],
            detail: "mutation".into(),
            saved_name: "Saved".into(),
            setlist_key: KEY.into(),
            position: 2,
            instrument: 0,
            catalog_policy: CatalogVerificationPolicy {
                timeout_ms: 2_000,
                passive_window_ms: 0,
                max_requests: 100,
                initial_retry_ms: 10,
                maximum_retry_ms: 20,
            },
            saved_presets: vec![record()],
            folder_copy: None,
            setlist_deletion: None,
            deletion: None,
        }
    }
    fn folder(key: &str, name: Option<&str>) -> PresetFolderListing {
        PresetFolderListing {
            order: None,
            key: key.into(),
            name: key.rsplit('/').next().unwrap().into(),
            file_type: Some(0),
            is_factory: false,
            is_plugin: false,
            files: name
                .into_iter()
                .map(|name| PresetFileListing {
                    position: 2,
                    name: name.into(),
                    instrument: 3,
                    instrument_present: true,
                    product_key: Some("device-key".into()),
                    ..Default::default()
                })
                .collect(),
        }
    }
    fn runtime(plan: &PresetMutationPlan) -> PresetMutationCatalog {
        PresetMutationCatalog::new(plan, 10, 1, 10, RequestDeadline::new(10, 2_000))
    }
    fn publication(folder: Option<&PresetFolderListing>) -> pa::FileMessage {
        let Some(folder) = folder else {
            return pa::FileMessage::default();
        };
        pa::FileMessage {
            action: pa::message_action::Enum::Create as i32,
            r#type: folder.file_type.map(pa::file_message::Type::Type),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(folder.key.clone())),
                name: Some(pa::folder_info::Name::Name(folder.name.clone())),
                is_plugin: Some(pa::folder_info::IsPlugin::IsPlugin(folder.is_plugin)),
                is_factory: Some(pa::folder_info::IsFactory::IsFactory(folder.is_factory)),
                files: folder
                    .files
                    .iter()
                    .map(|file| pa::ProductData {
                        index: Some(pa::product_data::Index::Index(file.position as i32)),
                        key: file.product_key.clone().map(pa::product_data::Key::Key),
                        name: Some(pa::product_data::Name::Name(file.name.clone())),
                        instrument: file
                            .instrument_present
                            .then_some(pa::product_data::Instrument::Instrument(file.instrument)),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })),
            ..Default::default()
        }
    }
    fn observe(
        runtime: &mut PresetMutationCatalog,
        sequence: u64,
        now: u64,
        folder: Option<&PresetFolderListing>,
    ) {
        runtime.observe(
            sequence,
            Some(IngressTime {
                first_report_at_ms: now,
                last_report_at_ms: now,
            }),
            &publication(folder),
        );
    }
    fn observe_delete(
        runtime: &mut PresetMutationCatalog,
        sequence: u64,
        now: u64,
        key: &str,
        product_key: Option<&str>,
    ) {
        let mut message = publication(Some(&folder(key, None)));
        message.action = pa::message_action::Enum::Delete as i32;
        let Some(pa::file_message::Folder::Folder(folder)) = &mut message.folder else {
            unreachable!()
        };
        folder.files = product_key
            .into_iter()
            .map(|key| pa::ProductData {
                key: Some(pa::product_data::Key::Key(key.into())),
                ..Default::default()
            })
            .collect();
        runtime.observe(
            sequence,
            Some(IngressTime {
                first_report_at_ms: now,
                last_report_at_ms: now,
            }),
            &message,
        );
    }
    fn full_folder(key: &str, name: &str) -> PresetFolderListing {
        let mut listing = folder(key, Some(name));
        for position in 0..256 {
            if position != 2 {
                listing.files.push(PresetFileListing {
                    position,
                    name: String::new(),
                    instrument: 0,
                    instrument_present: false,
                    product_key: None,
                    ..Default::default()
                });
            }
        }
        listing
    }
    fn copy_plan(operation: &str) -> PresetMutationPlan {
        let mut plan = plan();
        plan.saved_presets.clear();
        let mut expected = record();
        expected.instrument = 3;
        plan.folder_copy = Some(FolderCopyVerification {
            operation: operation.into(),
            source_key: KEY.into(),
            source_name: "Test".into(),
            before_folder_keys: vec![KEY.into()],
            expected_presets: vec![expected],
            catalog_policy: plan.catalog_policy,
        });
        plan
    }

    #[test]
    fn read_action_cannot_confirm_a_save_but_fresh_identity_uses_instrument_default() {
        let mut runtime = runtime(&plan());
        let mut message = publication(Some(&folder(KEY, Some("Saved"))));
        message.action = pa::message_action::Enum::Read as i32;
        runtime.observe(
            2,
            Some(IngressTime {
                first_report_at_ms: 11,
                last_report_at_ms: 11,
            }),
            &message,
        );
        assert_eq!(runtime.take_action(11), CatalogVerificationAction::Request);
        let mut missing = folder(KEY, Some("Saved"));
        missing.files[0].instrument_present = false;
        observe(&mut runtime, 3, 12, Some(&missing));
        assert_eq!(runtime.take_action(12), CatalogVerificationAction::Complete);
        let saved = &runtime.resolution().unwrap().saved_presets[0];
        assert_eq!(saved.name, "Saved");
        assert_eq!(saved.instrument, 0);
        assert!(!runtime.native.folder(KEY).unwrap().files[0].instrument_present);
    }

    #[test]
    fn paired_update_supplies_destination_proof_without_reusing_source_records() {
        let mut runtime = runtime(&plan());
        let mut message = publication(Some(&folder("/media/p4/Presets/Source", Some("Other"))));
        let Some(pa::file_message::Folder::Folder(destination)) =
            publication(Some(&folder(KEY, Some("Saved")))).folder
        else {
            unreachable!()
        };
        message.to_folder = Some(pa::file_message::ToFolder::ToFolder(destination));
        message.action = pa::message_action::Enum::Update as i32;
        runtime.observe(
            2,
            Some(IngressTime {
                first_report_at_ms: 11,
                last_report_at_ms: 11,
            }),
            &message,
        );
        assert_eq!(runtime.take_action(11), CatalogVerificationAction::Complete);
        assert_eq!(
            runtime.resolution().unwrap().saved_presets[0].setlist_key,
            KEY
        );
    }

    #[test]
    fn target_recreation_cancels_explicit_folder_deletion_before_settlement() {
        let mut plan = plan();
        plan.saved_presets.clear();
        plan.setlist_deletion = Some(crate::device::request::SetlistDeletionVerification {
            setlist_key: KEY.into(),
        });
        let mut runtime = runtime(&plan);
        observe_delete(&mut runtime, 2, 11, KEY, None);
        observe(&mut runtime, 3, 12, Some(&folder(KEY, None)));
        assert!(matches!(
            runtime.take_action(262),
            CatalogVerificationAction::Wait { .. }
        ));
        assert!(runtime.resolution().is_none());
        observe_delete(&mut runtime, 4, 263, KEY, None);
        assert_eq!(
            runtime.take_action(513),
            CatalogVerificationAction::Complete
        );
    }

    #[test]
    fn save_reconciles_a_later_generation_without_replaying_a_mutation() {
        let mut runtime = runtime(&plan());
        assert_eq!(runtime.take_action(10), CatalogVerificationAction::Request);
        observe(&mut runtime, 2, 11, Some(&folder(KEY, Some("Previous"))));
        assert!(matches!(
            runtime.take_action(11),
            CatalogVerificationAction::Wait { .. }
        ));
        assert!(runtime.resolution().is_none());
        // A stale generation does not earn an immediate replay: the directory
        // dump may still be streaming, and a second READ would only queue
        // another full-library generation behind the first.
        assert!(matches!(
            runtime.take_action(31),
            CatalogVerificationAction::Wait { .. }
        ));
        assert_eq!(
            runtime.take_action(11 + profile::PRESET_LIBRARY_SETTLEMENT_QUIET_MS),
            CatalogVerificationAction::Request
        );
        observe(
            &mut runtime,
            3,
            12 + profile::PRESET_LIBRARY_SETTLEMENT_QUIET_MS,
            Some(&folder(KEY, Some("Saved_2"))),
        );
        assert_eq!(
            runtime.take_action(12 + profile::PRESET_LIBRARY_SETTLEMENT_QUIET_MS),
            CatalogVerificationAction::Complete
        );
        let resolved = runtime.resolution().unwrap();
        assert_eq!(resolved.saved_name, "Saved_2");
        assert_eq!(resolved.saved_presets[0].instrument, 3);
    }

    #[test]
    fn deletion_requires_fresh_absence_of_the_exact_device_key() {
        let mut plan = plan();
        plan.saved_presets.clear();
        plan.deletion = Some(PresetDeletionVerification {
            setlist_key: KEY.into(),
            product_key: "device-key".into(),
            expected_name: "Saved".into(),
            expected_position: 2,
        });
        let mut runtime = runtime(&plan);
        observe(&mut runtime, 2, 11, Some(&folder(KEY, Some("Renamed"))));
        assert!(matches!(
            runtime.take_action(11),
            CatalogVerificationAction::Wait { .. }
        ));
        observe(&mut runtime, 3, 12, Some(&folder(KEY, None)));
        assert!(matches!(
            runtime.take_action(12),
            CatalogVerificationAction::Wait { .. }
        ));
        observe_delete(&mut runtime, 4, 13, KEY, Some("device-key"));
        assert_eq!(runtime.take_action(13), CatalogVerificationAction::Complete);
    }

    #[test]
    fn copy_waits_for_quiet_and_full_inventory_not_an_empty_new_folder() {
        let mut runtime = runtime(&copy_plan("copy"));
        let key = "/media/p4/Presets/Copy";
        observe(&mut runtime, 2, 11, Some(&folder(key, None)));
        assert!(matches!(
            runtime.take_action(260),
            CatalogVerificationAction::Wait { .. }
        ));
        assert!(matches!(
            runtime.take_action(261),
            CatalogVerificationAction::Wait { .. }
        ));
        assert!(runtime.resolution().is_none());
        observe(&mut runtime, 3, 270, Some(&full_folder(key, "Saved")));
        // Non-preset traffic extends a running catalog's quiet interval.
        observe(&mut runtime, 4, 300, None);
        assert!(matches!(
            runtime.take_action(520),
            CatalogVerificationAction::Wait { .. }
        ));
        assert_eq!(
            runtime.take_action(550),
            CatalogVerificationAction::Complete
        );
        assert_eq!(runtime.resolution().unwrap().setlist_key, key);
    }

    #[test]
    fn another_read_cannot_erase_ambiguous_copy_candidates() {
        let mut runtime = runtime(&copy_plan("copy"));
        observe(
            &mut runtime,
            2,
            11,
            Some(&full_folder("/media/p4/Presets/A", "Saved")),
        );
        observe(
            &mut runtime,
            3,
            12,
            Some(&full_folder("/media/p4/Presets/B", "Saved")),
        );
        assert!(matches!(
            runtime.take_action(262),
            CatalogVerificationAction::Wait { .. }
        ));
        assert!(runtime.failure_message().contains("observed 2"));
        assert_eq!(runtime.take_action(282), CatalogVerificationAction::Request);
        observe(
            &mut runtime,
            4,
            283,
            Some(&full_folder("/media/p4/Presets/A", "Saved")),
        );
        assert!(matches!(
            runtime.take_action(533),
            CatalogVerificationAction::Wait { .. }
        ));
        observe_delete(&mut runtime, 5, 534, "/media/p4/Presets/B", None);
        assert_eq!(
            runtime.take_action(784),
            CatalogVerificationAction::Complete
        );
    }

    #[test]
    fn create_selects_the_planned_name_without_accepting_an_unrelated_new_folder() {
        let mut runtime = runtime(&copy_plan("create"));
        observe(
            &mut runtime,
            2,
            11,
            Some(&folder("/media/p4/Presets/Unrelated", None)),
        );
        observe(
            &mut runtime,
            3,
            12,
            Some(&folder("/media/p4/Presets/Saved", None)),
        );
        assert_eq!(
            runtime.take_action(262),
            CatalogVerificationAction::Complete
        );
        assert_eq!(runtime.resolution().unwrap().saved_name, "Saved");
    }

    #[test]
    fn stale_duplicate_and_late_publications_cannot_establish_success() {
        let mut runtime =
            PresetMutationCatalog::new(&plan(), 10, 1, 10, RequestDeadline::new(10, 30));
        observe(&mut runtime, 1, 11, Some(&folder(KEY, Some("Saved"))));
        observe(&mut runtime, 2, 10, Some(&folder(KEY, Some("Saved"))));
        runtime.observe(3, None, &publication(Some(&folder(KEY, Some("Saved")))));
        assert_eq!(runtime.take_action(12), CatalogVerificationAction::Request);
        observe(&mut runtime, 4, 20, Some(&folder(KEY, Some("Previous"))));
        observe(&mut runtime, 4, 21, Some(&folder(KEY, Some("Saved"))));
        assert!(matches!(
            runtime.take_action(21),
            CatalogVerificationAction::Wait { .. }
        ));
        observe(&mut runtime, 5, 39, Some(&folder(KEY, Some("Saved"))));
        assert!(matches!(
            runtime.take_action(40),
            CatalogVerificationAction::Failed { .. }
        ));
        assert!(runtime.resolution().is_none());
    }

    #[test]
    fn byte_capacity_verification_reports_bytes_without_calling_one_folder_a_folder_overflow() {
        use prost::Message as _;
        let mut pending = runtime(&plan());
        let large = "x".repeat(1024 * 1024);
        for index in 0..64u32 {
            let mut listing = folder(KEY, Some(&large));
            listing.files[0].position = index;
            listing.files[0].product_key = Some(format!("bounded-key-{index}"));
            let mut message = publication(Some(&listing));
            message.action = pa::message_action::Enum::Update as i32;
            assert!(message.encode_to_vec().len() < crate::profile::MAX_INFLATED_BYTES);
            pending.observe(
                index as u64 + 2,
                Some(IngressTime {
                    first_report_at_ms: index as u64 + 11,
                    last_report_at_ms: index as u64 + 11,
                }),
                &message,
            );
            if pending.overflowed() {
                break;
            }
        }
        assert!(
            pending.overflowed(),
            "bounded valid deltas must reach the provider byte ceiling"
        );
        assert_eq!(pending.native.folders().count(), 1);
        assert!(matches!(
            pending.take_action(500),
            CatalogVerificationAction::Failed { .. }
        ));
        let failure = pending.failure_message();
        assert!(failure.contains("retained-byte capacity"), "{failure}");
        assert!(
            failure.contains(&crate::device::native_catalog::MAX_PRESET_CATALOG_BYTES.to_string()),
            "{failure}"
        );
        assert!(!failure.contains("folder capacity"), "{failure}");
        assert!(pending.resolution().is_none());

        // Ordinary fresh evidence still verifies; the folder-count diagnostic
        // is independently pinned by the pre-existing capacity test below.
        let mut healthy = runtime(&plan());
        observe(&mut healthy, 2, 11, Some(&folder(KEY, Some("Saved"))));
        assert_eq!(healthy.take_action(11), CatalogVerificationAction::Complete);
        assert_eq!(healthy.resolution().unwrap().saved_name, "Saved");
    }

    #[test]
    fn folder_capacity_failure_cannot_verify_a_truncated_candidate_set() {
        let mut runtime = runtime(&copy_plan("create"));
        for index in 0..=MAX_PRESET_FOLDERS {
            observe(
                &mut runtime,
                index as u64 + 2,
                index as u64 + 11,
                Some(&folder(&format!("/media/p4/Presets/{index}"), None)),
            );
        }
        assert!(matches!(
            runtime.take_action(500),
            CatalogVerificationAction::Failed { .. }
        ));
        assert!(runtime.failure_message().contains("capacity"));
        assert_eq!(
            runtime.failure_message(),
            "Preset catalog verification exceeded its retained folder capacity"
        );
    }
}
