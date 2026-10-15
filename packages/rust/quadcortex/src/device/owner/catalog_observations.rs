//! Retained native records with cumulative session and ingress provenance.
//! Public reads and pre-attempt admission never consume mutable presentation rows.
use super::ObservationTicket;
use crate::device::native_catalog::{NativeCatalogChange, NativePresetCatalog};
use crate::proto::cortex_protobuf_v2 as pa;
use crate::state::{project_preset_folder, PresetFileListing, PresetFolderListing};
use serde::Serialize;
use std::collections::BTreeMap;

/// Why retained catalog evidence stopped being usable. The first cause wins;
/// once invalid, the catalog stays invalid until it is cleared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CatalogInvalidation {
    /// A File frame from a different session reached the retained catalog.
    CrossSessionFrame,
    /// A File frame carried no native ingress timing.
    IngressTimingMissing,
    /// A File frame's first report arrived after its last report.
    IngressTimingInverted,
}

/// Which retention gate kept [`CatalogObservations::retained_catalog`] empty,
/// checked in the same order that function checks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedCatalogGap {
    /// No File frame changed the native catalog since the last clear.
    NoOrigin,
    /// The retained evidence was invalidated.
    Invalidated(CatalogInvalidation),
    /// The frame that started the retained catalog belongs to another session
    /// epoch than the caller's fence.
    OriginNotAccepted,
    /// The newest contributing frame finished arriving after `now_ms`.
    IngressPending,
}

struct FolderObservation {
    header: PresetFolderListing,
    slots: BTreeMap<u32, PresetFileListing>,
}

#[derive(Default)]
pub(super) struct CatalogObservations {
    folders: BTreeMap<String, FolderObservation>,
    /// Next first-seen sequence number. Monotonic between clears, so a folder that is removed
    /// and published again is placed after the folders that stayed.
    next_folder_order: u32,
    retained_origin: Option<ObservationTicket>,
    retained_until_ms: u64,
    invalidation: Option<CatalogInvalidation>,
    /// File frames reduced here since the last clear, and how many of them
    /// changed the native catalog. Diagnostics only.
    file_frames: u64,
    catalog_changes: u64,
    /// File frames discarded by in-session epoch changes (rebuilds) on this
    /// connection. Survives [`Self::clear_for_rebuild`]; reset by
    /// [`Self::clear`]. Diagnostics only.
    rebuild_discarded_frames: u64,
}

impl CatalogObservations {
    pub fn clear(&mut self) {
        self.folders.clear();
        self.next_folder_order = 0;
        self.retained_origin = None;
        self.retained_until_ms = 0;
        self.invalidation = None;
        self.file_frames = 0;
        self.catalog_changes = 0;
        self.rebuild_discarded_frames = 0;
    }

    /// Clears like [`Self::clear`] for an in-session epoch change, but keeps a
    /// tally of the File frames that change discarded, so an empty catalog can
    /// say it was emptied by a rebuild rather than never published.
    pub fn clear_for_rebuild(&mut self) {
        let discarded = self
            .rebuild_discarded_frames
            .saturating_add(self.file_frames);
        self.clear();
        self.rebuild_discarded_frames = discarded;
    }

    /// File frames reduced since the last clear.
    pub fn file_frames(&self) -> u64 {
        self.file_frames
    }

    /// File frames since the last clear that changed the native catalog.
    pub fn catalog_changes(&self) -> u64 {
        self.catalog_changes
    }

    /// File frames that in-session rebuilds discarded on this connection.
    pub fn rebuild_discarded_frames(&self) -> u64 {
        self.rebuild_discarded_frames
    }

    /// Folders currently retained, whether or not they are usable.
    pub fn folder_count(&self) -> usize {
        self.folders.len()
    }

    /// The retained catalog's first invalidation, if any.
    pub fn invalidation(&self) -> Option<CatalogInvalidation> {
        self.invalidation
    }

    /// The epoch revision of the frame that started the retained catalog.
    pub fn origin_revision(&self) -> Option<u64> {
        self.retained_origin
            .as_ref()
            .and_then(|origin| origin.epoch.as_ref())
            .map(|epoch| epoch.revision())
    }

    /// The gate that [`Self::retained_catalog`] fails, or `Ok(())` when it
    /// would return a catalog. Both share this check so they cannot disagree.
    pub fn retention_gate(
        &self,
        accepts: impl Fn(&ObservationTicket) -> bool,
        now_ms: u64,
    ) -> Result<(), RetainedCatalogGap> {
        let origin = self
            .retained_origin
            .as_ref()
            .ok_or(RetainedCatalogGap::NoOrigin)?;
        if let Some(reason) = self.invalidation {
            return Err(RetainedCatalogGap::Invalidated(reason));
        }
        if !accepts(origin) {
            return Err(RetainedCatalogGap::OriginNotAccepted);
        }
        if self.retained_until_ms > now_ms {
            return Err(RetainedCatalogGap::IngressPending);
        }
        Ok(())
    }

    /// Retained planning evidence may predate the request. Every action that
    /// contributed to it must nevertheless belong to this session and have
    /// ordered ingress. Unknown provenance invalidates planning until reset;
    /// later partial updates cannot repair the provenance of inherited fields.
    pub fn retained_catalog(
        &self,
        accepts: impl Fn(&ObservationTicket) -> bool,
        now_ms: u64,
    ) -> Option<(BTreeMap<String, PresetFolderListing>, u64)> {
        self.retention_gate(accepts, now_ms).ok()?;
        Some((
            self.folders
                .keys()
                .filter_map(|key| {
                    self.retained_listing(key)
                        .map(|folder| (key.clone(), folder))
                })
                .collect(),
            self.retained_until_ms,
        ))
    }

    fn invalidate(&mut self, reason: CatalogInvalidation) {
        self.invalidation.get_or_insert(reason);
    }

    pub fn retained_listing(&self, key: &str) -> Option<PresetFolderListing> {
        let folder = self.folders.get(key.trim_end_matches('/'))?;
        // The header carries the first-seen sequence number assigned at insertion.
        let mut listing = folder.header.clone();
        listing.files = folder.slots.values().cloned().collect();
        Some(listing)
    }

    pub fn observe(
        &mut self,
        ticket: &ObservationTicket,
        message: &pa::FileMessage,
        change: &NativeCatalogChange,
        native: &NativePresetCatalog,
    ) {
        self.file_frames = self.file_frames.saturating_add(1);
        if !change.updated.is_empty() || !change.removed.is_empty() {
            self.catalog_changes = self.catalog_changes.saturating_add(1);
            if let Some(origin) = &self.retained_origin {
                if !origin.same_session_as(ticket) {
                    self.invalidate(CatalogInvalidation::CrossSessionFrame);
                }
            } else {
                self.retained_origin = Some(ticket.clone());
            }
            match ticket.ingress {
                Some(time) if time.first_report_at_ms <= time.last_report_at_ms => {
                    self.retained_until_ms = self.retained_until_ms.max(time.last_report_at_ms);
                }
                Some(_) => self.invalidate(CatalogInvalidation::IngressTimingInverted),
                None => self.invalidate(CatalogInvalidation::IngressTimingMissing),
            }
        }
        if let Some((source, destination)) = &change.renamed {
            if let Some(mut folder) = self.folders.remove(source) {
                if let Some(current) = native.folder(destination) {
                    folder.header.key = destination.clone();
                    folder.header.name = match &message.to_folder {
                        Some(pa::file_message::ToFolder::ToFolder(wire)) => match &wire.name {
                            Some(pa::folder_info::Name::Name(name)) => name.clone(),
                            _ => destination.rsplit('/').next().unwrap_or_default().into(),
                        },
                        _ => current.name.clone(),
                    };
                    for observation in folder.slots.values_mut() {
                        if observation.product_key.is_some() {
                            observation.product_key =
                                Some(format!("{}/{}.pb", destination, observation.name));
                        }
                    }
                    self.folders.insert(destination.clone(), folder);
                }
            }
            return;
        }
        for key in &change.removed {
            self.folders.remove(key);
        }
        let Some(pa::file_message::Folder::Folder(source)) = &message.folder else {
            return;
        };
        let destination = match &message.to_folder {
            Some(pa::file_message::ToFolder::ToFolder(folder))
                if !source.files.is_empty()
                    && !folder.files.is_empty()
                    && matches!(
                        pa::message_action::Enum::try_from(message.action),
                        Ok(pa::message_action::Enum::Create | pa::message_action::Enum::Update)
                    ) =>
            {
                Some(folder)
            }
            _ => None,
        };
        let file_type = match message.r#type {
            Some(pa::file_message::Type::Type(value)) => Some(value),
            _ => None,
        };
        for wire in std::iter::once(source).chain(destination) {
            let Some(mut incoming) = project_preset_folder(wire, file_type) else {
                continue;
            };
            if !crate::device::PresetLibrary::is_preset_setlist_listing(&incoming) {
                continue;
            }
            incoming.key = incoming.key.trim_end_matches('/').to_string();
            if !change.updated.contains(&incoming.key) {
                continue;
            }
            let Some(current) = native.folder(&incoming.key) else {
                continue;
            };
            let supplied = std::mem::take(&mut incoming.files);
            // The sequence number is assigned on first insertion only; later frames, renames
            // and header updates keep it, so it is the order the device first published the
            // folder in, which is the order its own directory lists them.
            let next_order = &mut self.next_folder_order;
            let folder = self.folders.entry(incoming.key.clone()).or_insert_with(|| {
                let mut header = incoming.clone();
                header.order = Some(*next_order);
                *next_order = next_order.saturating_add(1);
                FolderObservation {
                    header,
                    slots: BTreeMap::new(),
                }
            });
            // Only this wire header can change retained header evidence. A
            // programmatically changed native/presentation cache is not input.
            if wire.name.is_some() {
                folder.header.name = incoming.name;
            }
            if wire.is_factory.is_some() {
                folder.header.is_factory = incoming.is_factory;
            }
            if message.action == pa::message_action::Enum::Delete as i32 {
                for deleted in &wire.files {
                    let Some(pa::product_data::Key::Key(key)) = &deleted.key else {
                        continue;
                    };
                    if key.is_empty() {
                        continue;
                    }
                    for observation in folder.slots.values_mut() {
                        if observation.product_key.as_ref() == Some(key) {
                            let record = PresetFileListing {
                                position: observation.position,
                                name: String::new(),
                                instrument: 0,
                                instrument_present: false,
                                product_key: None,
                                author: None,
                                date: None,
                                date_ms_since_epoch: None,
                            };
                            *observation = record;
                        }
                    }
                }
            } else {
                for mut record in supplied {
                    let Some(_) = current
                        .files
                        .iter()
                        .find(|slot| slot.position == record.position)
                    else {
                        continue;
                    };
                    if record.product_key.is_none() {
                        // Explicit keyless slot records clear the model, even
                        // if the incoming record also contains a display name.
                        record.name.clear();
                        record.instrument = 0;
                        record.instrument_present = false;
                    }
                    let mut retained = record.clone();
                    if retained.product_key.is_some() && !retained.instrument_present {
                        if let Some(previous) = folder.slots.get(&record.position) {
                            retained.instrument = previous.instrument;
                            retained.instrument_present = previous.instrument_present;
                        }
                    }
                    folder.slots.insert(record.position, retained);
                }
            }
        }
    }
}
