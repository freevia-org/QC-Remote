//! Native preset directory reduction. File publications are operations on
//! folder/slot models, not complete snapshots. Keep this state separate from
//! presentation overlays and locally installed catalog rows.
use crate::device::PresetLibrary;
use crate::proto::cortex_protobuf_v2 as pa;
use crate::state::{project_preset_folder, PresetFileListing, PresetFolderListing};
use std::collections::BTreeMap;

/// Most preset folders [`NativePresetCatalog`] retains. A message that would
/// exceed it makes [`NativePresetCatalog::apply`] fail.
pub const MAX_PRESET_FOLDERS: usize = 128;

/// Software ceiling for owned string bytes retained by the native preset fold.
/// This matches the Capture/IR provider budget; it is not a firmware name or
/// inventory limit. Map keys and repeated owned fields are counted separately.
pub const MAX_PRESET_CATALOG_BYTES: usize = super::native_library::MAX_LIBRARY_BYTES;

/// Keys affected by one [`NativePresetCatalog::apply`] call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NativeCatalogChange {
    /// Keys whose supplied records or metadata were applied, even if unchanged.
    pub updated: Vec<String>,
    /// Explicit folder deletion or the old key of a folder rename.
    pub removed: Vec<String>,
    /// `(old key, new key)` when the message renamed a folder.
    pub renamed: Option<(String, String)>,
}

/// Preset folder listings reduced from native File messages.
///
/// Each message updates the folder and slot models it names; it is not a
/// complete snapshot. After a failed [`NativePresetCatalog::apply`] the catalog
/// stays invalid, and `apply` keeps failing, until [`NativePresetCatalog::clear`].
#[derive(Default)]
pub struct NativePresetCatalog {
    folders: BTreeMap<String, PresetFolderListing>,
    error: Option<String>,
}

impl NativePresetCatalog {
    /// Drops every folder and any recorded error.
    pub fn clear(&mut self) {
        self.folders.clear();
        self.error = None;
    }

    /// The folder stored under `key`, ignoring trailing slashes, or `None` if
    /// there is none.
    pub fn folder(&self, key: &str) -> Option<&PresetFolderListing> {
        self.folders.get(key.trim_end_matches('/'))
    }

    /// All retained folders, ordered by key.
    pub fn folders(&self) -> impl Iterator<Item = &PresetFolderListing> {
        self.folders.values()
    }

    /// `Ok` while the catalog is usable, or the recorded error once it has been
    /// invalidated.
    pub fn health(&self) -> Result<(), String> {
        self.error
            .as_ref()
            .map_or(Ok(()), |error| Err(error.clone()))
    }

    /// Marks the catalog invalid with `error`. If it is already invalid the
    /// first error is kept.
    pub fn invalidate(&mut self, error: String) {
        self.error.get_or_insert(error);
    }

    /// Reduce one decoded native File message. This does not establish ingress
    /// freshness or persistence; the session owner must retain those separately.
    pub fn apply(&mut self, message: &pa::FileMessage) -> Result<NativeCatalogChange, String> {
        self.health()?;
        // A single bounded frame may update either or both folder models. Stage
        // the complete bounded next fold so overflow cannot publish one side or
        // remove a rename source before refusing the destination. The old fold
        // remains available for diagnostics, but health stays failed until reset.
        let mut next = Self {
            folders: self.folders.clone(),
            error: None,
        };
        let result = next.apply_inner(message).and_then(|change| {
            next.check_byte_capacity()?;
            Ok(change)
        });
        if result.is_ok() {
            self.folders = next.folders;
        }
        if let Err(error) = &result {
            // Only the staged fold can exceed the ceiling. Keep the unchanged,
            // already-bounded old fold for diagnostics, never for planning:
            // health remains failed until an explicit clear/reset.
            self.invalidate(error.clone());
        }
        result
    }

    fn check_rename_byte_capacity(
        &self,
        source_key: &str,
        target: &PresetFolderListing,
    ) -> Result<(), String> {
        let Some(source) = self.folders.get(source_key) else {
            return Ok(());
        };
        // Rename synthesizes one path per retained keyed slot. A bounded input
        // key can therefore multiply far beyond the wire budget: count those
        // borrowed lengths before any format!/replacement allocation occurs.
        let mut bytes = target
            .key
            .len()
            .saturating_mul(2)
            .saturating_add(target.name.len());
        for file in &source.files {
            bytes = bytes
                .saturating_add(file.name.len())
                .saturating_add(file.author.as_ref().map_or(0, String::len))
                .saturating_add(file.date.as_ref().map_or(0, String::len));
            if file.product_key.is_some() {
                bytes = bytes
                    .saturating_add(target.key.len())
                    .saturating_add(file.name.len())
                    .saturating_add(4);
            }
        }
        for (key, folder) in &self.folders {
            if key == source_key || key == &target.key {
                continue;
            }
            bytes = bytes
                .saturating_add(key.len())
                .saturating_add(folder.key.len())
                .saturating_add(folder.name.len());
            for file in &folder.files {
                bytes = bytes
                    .saturating_add(file.name.len())
                    .saturating_add(file.product_key.as_ref().map_or(0, String::len))
                    .saturating_add(file.author.as_ref().map_or(0, String::len))
                    .saturating_add(file.date.as_ref().map_or(0, String::len));
            }
        }
        if bytes > MAX_PRESET_CATALOG_BYTES {
            return Err(format!(
                "Native preset catalog exceeded its software retained-byte capacity of {MAX_PRESET_CATALOG_BYTES} bytes"
            ));
        }
        Ok(())
    }

    fn check_byte_capacity(&self) -> Result<(), String> {
        let mut bytes = 0usize;
        for (key, folder) in &self.folders {
            bytes = bytes.saturating_add(key.len());
            bytes = bytes.saturating_add(folder.key.len());
            bytes = bytes.saturating_add(folder.name.len());
            for file in &folder.files {
                for text in [
                    Some(&file.name),
                    file.product_key.as_ref(),
                    file.author.as_ref(),
                    file.date.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    bytes = bytes.saturating_add(text.len());
                }
            }
            if bytes > MAX_PRESET_CATALOG_BYTES {
                return Err(format!(
                    "Native preset catalog exceeded its software retained-byte capacity of {MAX_PRESET_CATALOG_BYTES} bytes"
                ));
            }
        }
        Ok(())
    }

    fn apply_inner(&mut self, message: &pa::FileMessage) -> Result<NativeCatalogChange, String> {
        let mut change = NativeCatalogChange::default();
        let Some(pa::file_message::Folder::Folder(source)) = &message.folder else {
            return Ok(change);
        };
        let file_type = match message.r#type {
            Some(pa::file_message::Type::Type(value)) => Some(value),
            _ => None,
        };
        let Some(source_listing) = eligible(source, file_type) else {
            return Ok(change);
        };
        let destination = match &message.to_folder {
            Some(pa::file_message::ToFolder::ToFolder(folder)) => Some(folder),
            _ => None,
        };
        let action = pa::message_action::Enum::try_from(message.action).ok();
        let source_key = source_listing.key.clone();
        match (action, source.files.is_empty()) {
            (Some(pa::message_action::Enum::Create), true) => {
                // CREATE must preserve an existing model rather than replace it.
                self.reserve_key(&source_key)?;
                self.folders
                    .entry(source_key.clone())
                    .or_insert(source_listing);
                change.updated.push(source_key);
            }
            (Some(pa::message_action::Enum::Update), true) => {
                // Empty-folder UPDATE is a rename with a destination FolderInfo,
                // not an empty replacement inventory.
                let Some(destination) = destination else {
                    return Ok(change);
                };
                let Some(target) = eligible(destination, file_type) else {
                    return Ok(change);
                };
                self.check_rename_byte_capacity(&source_key, &target)?;
                let Some(mut current) = self.folders.remove(&source_key) else {
                    return Ok(change);
                };
                current.key = target.key.clone();
                current.name = target.name;
                for file in &mut current.files {
                    if file.product_key.is_some() {
                        file.product_key = Some(format!("{}/{}.pb", current.key, file.name));
                    }
                }
                self.folders.insert(current.key.clone(), current);
                if source_key != target.key {
                    change.removed.push(source_key.clone());
                }
                change.renamed = Some((source_key, target.key.clone()));
                change.updated.push(target.key);
            }
            (Some(pa::message_action::Enum::Create | pa::message_action::Enum::Update), false) => {
                let target = destination
                    .filter(|folder| !folder.files.is_empty())
                    .and_then(|folder| {
                        eligible(folder, file_type).map(|listing| (folder, listing))
                    });
                // Check capacity before either side can change.
                let mut keys = vec![source_key.as_str()];
                if let Some((_, listing)) = &target {
                    if listing.key != source_key {
                        keys.push(&listing.key);
                    }
                }
                if self.folders.len()
                    + keys
                        .iter()
                        .filter(|key| !self.folders.contains_key(**key))
                        .count()
                    > MAX_PRESET_FOLDERS
                {
                    return Err("Native preset catalog exceeded its folder capacity".into());
                }
                self.upsert_products(source, source_listing);
                change.updated.push(source_key);
                // Source and destination record lists are visited in order; missing
                // source slots are never inferred from destination records.
                if let Some((folder, listing)) = target {
                    let key = listing.key.clone();
                    self.upsert_products(folder, listing);
                    if !change.updated.contains(&key) {
                        change.updated.push(key);
                    }
                }
            }
            (Some(pa::message_action::Enum::Delete), true) => {
                self.folders.remove(&source_key);
                change.removed.push(source_key);
            }
            (Some(pa::message_action::Enum::Delete), false) => {
                if let Some(current) = self.folders.get_mut(&source_key) {
                    for file in &source.files {
                        let Some(pa::product_data::Key::Key(key)) = &file.key else {
                            continue;
                        };
                        if key.is_empty() {
                            continue;
                        }
                        for slot in &mut current.files {
                            if slot.product_key.as_ref() == Some(key) {
                                *slot = empty_slot(slot.position);
                            }
                        }
                    }
                    change.updated.push(source_key);
                }
            }
            // READ is the request action, not a provider update. Keep unknown
            // actions inert, as do the concrete preset parsers in the binary.
            _ => {}
        }
        Ok(change)
    }

    fn reserve_key(&self, key: &str) -> Result<(), String> {
        if self.folders.len() >= MAX_PRESET_FOLDERS && !self.folders.contains_key(key) {
            return Err("Native preset catalog exceeded its folder capacity".into());
        }
        Ok(())
    }

    fn upsert_products(&mut self, wire: &pa::FolderInfo, mut incoming: PresetFolderListing) {
        let supplied = std::mem::take(&mut incoming.files);
        let current = self.folders.entry(incoming.key.clone()).or_insert(incoming);
        // Header absence must not replace established folder metadata with a
        // derived name or false flag while applying a single slot update.
        if let Some(pa::folder_info::Name::Name(name)) = &wire.name {
            current.name = name.clone();
        }
        if let Some(pa::folder_info::IsFactory::IsFactory(value)) = wire.is_factory {
            current.is_factory = value;
        }
        for mut file in supplied {
            // `current.files` is kept sorted by position at all times (every
            // path that builds or mutates it -- `project_preset_folder`'s
            // initial projection, this insert, and the Delete branch's
            // in-place `empty_slot` replacement -- preserves that order), so
            // a binary search locates an existing slot in O(log n) instead of
            // the previous O(n) linear scan over up to 256 slots, and a new
            // slot is inserted directly at its sorted position instead of
            // appended with the whole vector re-sorted afterwards on every
            // message regardless of whether anything was actually
            // out of order.
            let search = current
                .files
                .binary_search_by_key(&file.position, |slot| slot.position);
            if file.product_key.is_none() {
                file = empty_slot(file.position);
            } else if !file.instrument_present {
                // Instrument is only written when its optional field is
                // present. This retained value is state, not fresh field evidence.
                if let Ok(index) = search {
                    file.instrument = current.files[index].instrument;
                    file.instrument_present = current.files[index].instrument_present;
                }
            }
            match search {
                Ok(index) => current.files[index] = file,
                Err(insert_at) => current.files.insert(insert_at, file),
            }
        }
    }
}

fn eligible(folder: &pa::FolderInfo, file_type: Option<i32>) -> Option<PresetFolderListing> {
    let mut listing = project_preset_folder(folder, file_type)?;
    if !PresetLibrary::is_preset_setlist_listing(&listing) {
        return None;
    }
    listing.key = listing.key.trim_end_matches('/').to_string();
    Some(listing)
}

fn empty_slot(position: u32) -> PresetFileListing {
    PresetFileListing {
        position,
        name: String::new(),
        instrument: 0,
        instrument_present: false,
        product_key: None,
        author: None,
        date: None,
        date_ms_since_epoch: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::state_runtime::DeviceStateRuntime;
    use prost::Message;
    const SOURCE: &str = "/media/p4/Presets/Source";
    const TARGET: &str = "/media/p4/Presets/Target";

    #[test]
    fn cumulative_preset_metadata_is_bounded_and_reset_recovers() {
        let mut catalog = NativePresetCatalog::default();
        let large = "a".repeat(512 * 1024);
        let listing = |index: i32, author: &str| {
            let mut row = product(index, Some(&format!("key-{index}")), "Name");
            row.author = Some(pa::product_data::Author::Author(author.into()));
            event(pa::message_action::Enum::Update, SOURCE, vec![row])
        };
        for index in 0..63 {
            let message = listing(index, &large);
            assert!(message.encoded_len() < crate::profile::MAX_FRAME_BYTES);
            catalog.apply(&message).unwrap();
        }
        // Replacing a large slot and deleting another release their old metadata.
        catalog.apply(&listing(0, "small")).unwrap();
        catalog
            .apply(&event(
                pa::message_action::Enum::Delete,
                SOURCE,
                vec![product(1, Some("key-1"), "")],
            ))
            .unwrap();
        catalog.apply(&listing(63, &large)).unwrap();
        catalog.apply(&listing(64, &large)).unwrap();
        let error = catalog.apply(&listing(65, &large)).unwrap_err();
        assert!(error.contains("retained-byte capacity"), "{error}");
        let retained = catalog.folder(SOURCE).unwrap();
        assert_eq!(retained.files.len(), 65);
        assert!(retained.files.iter().all(|slot| slot.position != 65));
        assert_eq!(retained.files[0].author.as_deref(), Some("small"));
        assert!(retained.files[1].author.is_none());
        catalog.check_byte_capacity().unwrap();
        assert!(catalog.health().is_err());
        assert!(catalog.apply(&listing(0, "normal")).is_err());
        catalog.clear();
        catalog.apply(&listing(0, "normal")).unwrap();
        assert_eq!(
            catalog.folder(SOURCE).unwrap().files[0].author.as_deref(),
            Some("normal")
        );
    }

    #[test]
    fn rename_metadata_multiplication_is_refused_before_key_rebuild() {
        let mut catalog = NativePresetCatalog::default();
        catalog
            .apply(&event(
                pa::message_action::Enum::Create,
                SOURCE,
                (0..256)
                    .map(|index| product(index, Some("key"), "Name"))
                    .collect(),
            ))
            .unwrap();
        let mut rename = event(pa::message_action::Enum::Update, SOURCE, vec![]);
        let long_key = format!("/media/p4/Presets/{}", "x".repeat(512 * 1024));
        rename.to_folder = Some(pa::file_message::ToFolder::ToFolder(folder(
            &long_key,
            vec![],
        )));
        assert!(rename.encoded_len() < crate::profile::MAX_FRAME_BYTES);
        let error = catalog.apply(&rename).unwrap_err();
        assert!(error.contains("retained-byte capacity"), "{error}");
        assert_eq!(catalog.folder(SOURCE).unwrap().files.len(), 256);
        assert!(catalog.folder(&long_key).is_none());
        assert!(catalog.folder(SOURCE).unwrap().files.iter().all(|slot| slot
            .product_key
            .as_deref()
            == Some("key")
            && slot.name == "Name"));
        catalog.check_byte_capacity().unwrap();
        assert!(catalog.health().is_err());
        catalog.clear();
        catalog
            .apply(&event(
                pa::message_action::Enum::Create,
                SOURCE,
                vec![product(0, Some("key"), "Name")],
            ))
            .unwrap();
        rename.to_folder = Some(pa::file_message::ToFolder::ToFolder(folder(TARGET, vec![])));
        catalog.apply(&rename).unwrap();
        assert_eq!(
            catalog.folder(TARGET).unwrap().files[0]
                .product_key
                .as_deref(),
            Some("/media/p4/Presets/Target/Name.pb")
        );
    }

    fn product(index: i32, key: Option<&str>, name: &str) -> pa::ProductData {
        pa::ProductData {
            index: Some(pa::product_data::Index::Index(index)),
            key: key.map(|key| pa::product_data::Key::Key(key.into())),
            name: Some(pa::product_data::Name::Name(name.into())),
            ..Default::default()
        }
    }
    fn folder(key: &str, files: Vec<pa::ProductData>) -> pa::FolderInfo {
        pa::FolderInfo {
            key: Some(pa::folder_info::Key::Key(key.into())),
            files,
            ..Default::default()
        }
    }
    fn event(
        action: pa::message_action::Enum,
        key: &str,
        files: Vec<pa::ProductData>,
    ) -> pa::FileMessage {
        pa::FileMessage {
            action: action as i32,
            r#type: Some(pa::file_message::Type::Type(0)),
            folder: Some(pa::file_message::Folder::Folder(folder(key, files))),
            ..Default::default()
        }
    }
    fn seed(catalog: &mut NativePresetCatalog) {
        catalog
            .apply(&event(
                pa::message_action::Enum::Create,
                SOURCE,
                vec![
                    product(1, Some("source-one"), "One"),
                    product(2, Some("source-two"), "Two"),
                ],
            ))
            .unwrap();
    }

    #[test]
    fn partial_update_and_explicit_empty_slot_preserve_unmentioned_products() {
        let mut catalog = NativePresetCatalog::default();
        seed(&mut catalog);
        catalog
            .apply(&event(
                pa::message_action::Enum::Update,
                SOURCE,
                vec![product(1, Some("new-one"), "New one")],
            ))
            .unwrap();
        assert_eq!(
            catalog.folder(SOURCE).unwrap().files[1]
                .product_key
                .as_deref(),
            Some("source-two")
        );
        catalog
            .apply(&event(
                pa::message_action::Enum::Update,
                SOURCE,
                vec![product(1, None, "must not look occupied")],
            ))
            .unwrap();
        let slots = &catalog.folder(SOURCE).unwrap().files;
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0], empty_slot(1));
        assert_eq!(slots[1].name, "Two");
    }

    #[test]
    fn upsert_keeps_files_sorted_by_position_via_insertion_not_a_trailing_resort() {
        // Regression for the O(n) linear-scan-plus-unconditional-resort path:
        // a single message's slot updates can name positions in any order
        // (here: a new position below every existing slot, an in-place
        // replacement of an existing middle slot, and a new position past
        // every existing slot). `upsert_products` must land every slot at
        // its correct sorted index via binary-search-guided insertion, with
        // no separate resort pass, and existing untouched slots must survive
        // exactly where they were.
        let mut catalog = NativePresetCatalog::default();
        seed(&mut catalog); // positions 1 ("One") and 2 ("Two").
        catalog
            .apply(&event(
                pa::message_action::Enum::Update,
                SOURCE,
                vec![
                    product(5, Some("five-key"), "Five"),
                    product(0, Some("zero-key"), "Zero"),
                    product(2, Some("updated-two-key"), "Updated Two"),
                ],
            ))
            .unwrap();
        let files = &catalog.folder(SOURCE).unwrap().files;
        assert_eq!(
            files.iter().map(|file| file.position).collect::<Vec<_>>(),
            [0, 1, 2, 5],
            "slots must stay sorted by position regardless of update order"
        );
        assert_eq!(files[0].name, "Zero");
        assert_eq!(files[1].name, "One", "untouched slot must be preserved");
        assert_eq!(files[2].name, "Updated Two");
        assert_eq!(files[2].product_key.as_deref(), Some("updated-two-key"));
        assert_eq!(files[3].name, "Five");
    }

    #[test]
    fn paired_publication_updates_both_sides_without_inventing_source_deletion() {
        let mut catalog = NativePresetCatalog::default();
        seed(&mut catalog);
        let mut message = event(
            pa::message_action::Enum::Update,
            SOURCE,
            vec![product(1, None, "")],
        );
        message.to_folder = Some(pa::file_message::ToFolder::ToFolder(folder(
            TARGET,
            vec![product(7, Some("target-one"), "One")],
        )));
        let change = catalog.apply(&message).unwrap();
        assert_eq!(change.updated, [SOURCE, TARGET]);
        assert_eq!(catalog.folder(SOURCE).unwrap().files[0], empty_slot(1));
        assert_eq!(catalog.folder(SOURCE).unwrap().files[1].name, "Two");
        assert_eq!(catalog.folder(TARGET).unwrap().files[0].position, 7);
    }

    #[test]
    fn product_delete_matches_key_not_index_and_folder_delete_is_explicit() {
        let mut catalog = NativePresetCatalog::default();
        seed(&mut catalog);
        // A conflicting index must not delete the wrong product. Deletion
        // builds a set of nonempty product keys, then clears matching models.
        catalog
            .apply(&event(
                pa::message_action::Enum::Delete,
                SOURCE,
                vec![product(2, Some("source-one"), "")],
            ))
            .unwrap();
        assert_eq!(catalog.folder(SOURCE).unwrap().files[0], empty_slot(1));
        assert_eq!(catalog.folder(SOURCE).unwrap().files[1].name, "Two");
        let change = catalog
            .apply(&event(pa::message_action::Enum::Delete, SOURCE, vec![]))
            .unwrap();
        assert_eq!(change.removed, [SOURCE]);
        assert!(catalog.folder(SOURCE).is_none());
    }

    #[test]
    fn read_unknown_action_and_header_update_do_not_replace_catalog() {
        let mut catalog = NativePresetCatalog::default();
        seed(&mut catalog);
        for action in [pa::message_action::Enum::Read as i32, 999] {
            let mut message = event(pa::message_action::Enum::Create, SOURCE, vec![]);
            message.action = action;
            assert_eq!(
                catalog.apply(&message).unwrap(),
                NativeCatalogChange::default()
            );
        }
        catalog
            .apply(&event(pa::message_action::Enum::Update, SOURCE, vec![]))
            .unwrap();
        catalog
            .apply(&event(pa::message_action::Enum::Create, SOURCE, vec![]))
            .unwrap();
        assert_eq!(catalog.folder(SOURCE).unwrap().files.len(), 2);
    }

    #[test]
    fn folder_rename_retains_slots_and_rebuilds_saved_product_paths() {
        let mut catalog = NativePresetCatalog::default();
        seed(&mut catalog);
        let mut message = event(pa::message_action::Enum::Update, SOURCE, vec![]);
        message.to_folder = Some(pa::file_message::ToFolder::ToFolder(folder(TARGET, vec![])));
        let change = catalog.apply(&message).unwrap();
        assert_eq!(change.renamed, Some((SOURCE.into(), TARGET.into())));
        assert!(catalog.folder(SOURCE).is_none());
        let renamed = catalog.folder(TARGET).unwrap();
        assert_eq!(renamed.files.len(), 2);
        assert_eq!(
            renamed.files[1].product_key.as_deref(),
            Some("/media/p4/Presets/Target/Two.pb")
        );
    }

    #[test]
    fn native_envelope_preserves_absence_separately_from_retained_state() {
        let mut runtime = DeviceStateRuntime::new();
        let mut message = event(
            pa::message_action::Enum::Create,
            SOURCE,
            vec![product(1, Some("one"), "One")],
        );
        let Some(pa::file_message::Folder::Folder(source)) = &mut message.folder else {
            unreachable!()
        };
        source.files[0].instrument = Some(pa::product_data::Instrument::Instrument(2));
        runtime.ingest(4, &message.encode_to_vec()).unwrap();
        let delta = event(
            pa::message_action::Enum::Update,
            SOURCE,
            vec![product(1, Some("one"), "One renamed")],
        );
        let observation = runtime.ingest(4, &delta.encode_to_vec()).unwrap();
        assert_eq!(observation.file_message.as_deref(), Some(&delta));
        assert_eq!(
            runtime
                .native_preset_catalog()
                .folder(SOURCE)
                .unwrap()
                .files[0]
                .instrument,
            2
        );
        let mut local = runtime
            .native_preset_catalog()
            .folder(SOURCE)
            .unwrap()
            .clone();
        local.files[0].name = "Local presentation".into();
        runtime.ingest_preset_listing(local);
        assert_eq!(
            runtime
                .native_preset_catalog()
                .folder(SOURCE)
                .unwrap()
                .files[0]
                .name,
            "One renamed"
        );
        runtime.reset();
        assert!(runtime.native_preset_catalog().folder(SOURCE).is_none());
    }

    #[test]
    fn malformed_file_invalidates_native_planning_until_session_reset() {
        let mut runtime = DeviceStateRuntime::new();
        let message = event(
            pa::message_action::Enum::Create,
            SOURCE,
            vec![product(1, Some("one"), "One")],
        );
        runtime.ingest(4, &message.encode_to_vec()).unwrap();
        assert!(runtime.ingest(4, &[0x22, 0x02, 0xff]).is_err());
        assert!(runtime.native_preset_catalog().health().is_err());
        // A later partial update cannot prove what the malformed frame changed.
        assert!(runtime.ingest(4, &message.encode_to_vec()).is_err());
        runtime.reset();
        runtime.ingest(4, &message.encode_to_vec()).unwrap();
        assert!(runtime.native_preset_catalog().health().is_ok());
    }

    #[test]
    fn chunked_native_preset_deltas_have_a_transactional_retained_byte_ceiling() {
        // Every individual valid File publication fits the existing wire budget.
        // A sequence, rather than one oversized frame, grows the retained fold.
        // This is the existing Capture/IR SOFTWARE budget, not a firmware limit.
        let ceiling = crate::device::native_library::MAX_LIBRARY_BYTES;
        let mut runtime = DeviceStateRuntime::new();
        let large_name = "x".repeat(1024 * 1024);
        let mut accepted = 0usize;
        let mut refused = None;
        for index in 0..64 {
            let message = event(
                pa::message_action::Enum::Update,
                SOURCE,
                vec![product(index, Some(&format!("slot-{index}")), &large_name)],
            );
            let payload = message.encode_to_vec();
            assert!(payload.len() < crate::profile::MAX_INFLATED_BYTES);
            match runtime.ingest(4, &payload) {
                Ok(_) => accepted += 1,
                Err(error) => {
                    refused = Some((index, error));
                    break;
                }
            }
        }
        let (refused_index, error) =
            refused.expect("chunked valid deltas must not exceed the shared retained-byte ceiling");
        assert!(
            accepted > 1,
            "ordinary successive listings remain supported"
        );
        assert!(error.contains("retained-byte capacity"), "{error}");
        let retained = runtime.native_preset_catalog().folder(SOURCE).unwrap();
        assert_eq!(
            retained.files.len(),
            accepted,
            "the refused delta must not partially commit"
        );
        assert!(retained
            .files
            .iter()
            .all(|slot| slot.position != refused_index as u32));
        assert!(retained.files.iter().all(|slot| slot.name == large_name));
        let retained_bytes = retained.key.len() * 2
            + retained.name.len()
            + retained
                .files
                .iter()
                .map(|slot| {
                    slot.name.len()
                        + slot.product_key.as_ref().map_or(0, String::len)
                        + slot.author.as_ref().map_or(0, String::len)
                        + slot.date.as_ref().map_or(0, String::len)
                })
                .sum::<usize>();
        assert!(retained_bytes <= ceiling);
        assert!(runtime.native_preset_catalog().health().is_err());
        let healthy = event(
            pa::message_action::Enum::Update,
            SOURCE,
            vec![product(0, Some("small"), "Small")],
        );
        assert!(
            runtime.ingest(4, &healthy.encode_to_vec()).is_err(),
            "partial later data cannot repair the refused fold"
        );
        runtime.reset();
        runtime.ingest(4, &healthy.encode_to_vec()).unwrap();
        let recovered = runtime.native_preset_catalog().folder(SOURCE).unwrap();
        assert_eq!(recovered.files.len(), 1);
        assert_eq!(recovered.files[0].name, "Small");
    }

    #[test]
    fn native_preset_rename_refuses_multiplied_paths_before_retained_state_changes() {
        let mut runtime = DeviceStateRuntime::new();
        let seed = event(
            pa::message_action::Enum::Create,
            SOURCE,
            (0..256)
                .map(|index| product(index, Some(&format!("key-{index}")), "Short"))
                .collect(),
        );
        runtime.ingest(4, &seed.encode_to_vec()).unwrap();
        // One ~128KiB key fits a valid File frame, but synthesizing it for every
        // keyed slot would exceed the existing32MiB provider SOFTWARE budget.
        let target = format!("/media/p4/Presets/{}", "r".repeat(128 * 1024));
        let mut rename = event(pa::message_action::Enum::Update, SOURCE, vec![]);
        rename.to_folder = Some(pa::file_message::ToFolder::ToFolder(folder(
            &target,
            vec![],
        )));
        let payload = rename.encode_to_vec();
        assert!(payload.len() < crate::profile::MAX_INFLATED_BYTES);
        let result = runtime.ingest(4, &payload);
        assert!(
            result.is_err(),
            "derived rename paths must fit the retained-byte budget"
        );
        let error = result.unwrap_err();
        assert!(error.contains("retained-byte capacity"), "{error}");
        assert!(runtime.native_preset_catalog().folder(&target).is_none());
        let retained = runtime.native_preset_catalog().folder(SOURCE).unwrap();
        assert_eq!(retained.files.len(), 256);
        assert!(retained
            .files
            .iter()
            .enumerate()
            .all(|(index, slot)| slot.product_key.as_deref()
                == Some(format!("key-{index}").as_str())
                && slot.name == "Short"));
        assert!(runtime.native_preset_catalog().health().is_err());
        runtime.reset();
        runtime.ingest(4, &seed.encode_to_vec()).unwrap();
        rename.to_folder = Some(pa::file_message::ToFolder::ToFolder(folder(TARGET, vec![])));
        runtime.ingest(4, &rename.encode_to_vec()).unwrap();
        assert!(runtime.native_preset_catalog().folder(SOURCE).is_none());
        let healthy = runtime.native_preset_catalog().folder(TARGET).unwrap();
        assert_eq!(healthy.files.len(), 256);
        assert!(healthy
            .files
            .iter()
            .all(|slot| slot.product_key.as_deref() == Some("/media/p4/Presets/Target/Short.pb")));
    }
}
