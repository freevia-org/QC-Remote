//! Catalog operands retained from preparation, updated only by native File ingress.
use crate::state::{PresetFileListing, PresetFolderListing};
use std::collections::BTreeMap;

#[derive(PartialEq)]
struct FolderIdentity {
    name: String,
    file_type: Option<i32>,
    is_factory: bool,
    is_plugin: bool,
}
impl From<&PresetFolderListing> for FolderIdentity {
    fn from(folder: &PresetFolderListing) -> Self {
        Self {
            name: folder.name.clone(),
            file_type: folder.file_type,
            is_factory: folder.is_factory,
            is_plugin: folder.is_plugin,
        }
    }
}

pub(super) struct CatalogGuard {
    headers: BTreeMap<String, FolderIdentity>,
    inventories: BTreeMap<String, Vec<PresetFileListing>>,
    full_directory: bool,
    lost: bool,
}
impl CatalogGuard {
    pub(super) fn new(
        folders: &BTreeMap<String, PresetFolderListing>,
        required_keys: &[String],
        full_directory: bool,
    ) -> Self {
        Self {
            headers: folders
                .iter()
                .filter(|(key, folder)| !folder.is_factory || required_keys.contains(key))
                .map(|(key, folder)| (key.clone(), FolderIdentity::from(folder)))
                .collect(),
            inventories: folders
                .iter()
                .filter(|(key, _)| required_keys.contains(key))
                .map(|(key, folder)| (key.clone(), inventory(folder)))
                .collect(),
            full_directory,
            lost: false,
        }
    }
    pub(super) fn observe(&mut self, folder: &PresetFolderListing) {
        let key = folder.key.trim_end_matches('/');
        if let Some(expected) = self.headers.get(key) {
            if *expected != FolderIdentity::from(folder) {
                self.lost = true;
            }
        } else if self.full_directory && !folder.is_factory {
            self.lost = true;
        }
        if let Some(expected) = self.inventories.get(key) {
            if *expected != inventory(folder) {
                self.lost = true;
            }
        }
    }
    pub(super) fn matches(&self) -> bool {
        !self.lost
    }
    /// Name from the exact source inventory retained by native preparation.
    /// Capture before Save changes the catalog; never reconstruct it afterward.
    pub(super) fn source_name(&self, setlist_key: &str, position: u32) -> Option<String> {
        if self.lost {
            return None;
        }
        let mut rows = self
            .inventories
            .get(setlist_key.trim_end_matches('/'))?
            .iter()
            .filter(|row| row.position == position);
        let row = rows.next()?;
        if rows.next().is_some()
            || row.name.is_empty()
            || !row.product_key.as_ref().is_some_and(|key| !key.is_empty())
        {
            return None;
        }
        Some(row.name.clone())
    }
    pub(super) fn invalidate(&mut self) {
        self.lost = true;
    }
    pub(super) fn observe_removal(&mut self, key: &str) {
        if self.headers.contains_key(key.trim_end_matches('/')) {
            self.lost = true;
        }
    }
}
fn inventory(folder: &PresetFolderListing) -> Vec<PresetFileListing> {
    let mut files = folder.files.clone();
    files.sort_by(|left, right| {
        (
            left.position,
            &left.product_key,
            &left.name,
            left.instrument_present,
            left.instrument,
        )
            .cmp(&(
                right.position,
                &right.product_key,
                &right.name,
                right.instrument_present,
                right.instrument,
            ))
    });
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    fn folder() -> PresetFolderListing {
        PresetFolderListing {
            key: "/media/p4/Presets/Test".into(),
            name: "Test".into(),
            file_type: Some(0),
            is_factory: false,
            is_plugin: false,
            files: (0..2)
                .map(|position| PresetFileListing {
                    position,
                    name: format!("Preset {position}"),
                    instrument: 0,
                    instrument_present: true,
                    product_key: Some(format!("key-{position}")),
                    ..Default::default()
                })
                .collect(),
        }
    }
    #[test]
    fn source_name_requires_one_occupied_identity_in_an_unmodified_inventory() {
        for case in 0..6 {
            let mut original = folder();
            match case {
                1 => original.files[1].product_key = None,
                2 => original.files[1].name.clear(),
                3 => original.files.push(original.files[1].clone()),
                _ => {}
            }
            let mut guard = CatalogGuard::new(
                &BTreeMap::from([(original.key.clone(), original.clone())]),
                std::slice::from_ref(&original.key),
                false,
            );
            if case == 4 {
                guard.invalidate();
            }
            let key = if case == 5 {
                "Other"
            } else {
                original.key.as_str()
            };
            assert_eq!(
                guard.source_name(key, 1),
                (case == 0).then(|| "Preset 1".into()),
                "case {case}"
            );
        }
    }

    #[test]
    fn inventory_identity_ignores_wire_order_but_retains_keys_names_and_instrument_presence() {
        for change in 0..5 {
            let original = folder();
            let mut guard = CatalogGuard::new(
                &BTreeMap::from([(original.key.clone(), original.clone())]),
                std::slice::from_ref(&original.key),
                false,
            );
            let mut reordered = original.clone();
            reordered.files.reverse();
            guard.observe(&reordered);
            assert!(guard.matches());
            match change {
                0 => reordered.files[0].product_key = Some("replacement".into()),
                1 => reordered.files[0].name = "Renamed".into(),
                2 => reordered.files[0].instrument_present = false,
                3 => reordered.files[0].instrument = 1,
                _ => reordered.files[0].position = 9,
            }
            guard.observe(&reordered);
            assert!(!guard.matches());
            guard.observe(&original);
            assert!(!guard.matches());
        }
    }
    #[test]
    fn directory_identity_protects_custom_names_and_capacity_without_freezing_unrelated_inventory()
    {
        let original = folder();
        let folders = BTreeMap::from([(original.key.clone(), original.clone())]);
        let mut guard = CatalogGuard::new(&folders, &[], true);
        let mut observed = original.clone();
        observed.files.clear();
        guard.observe(&observed);
        assert!(guard.matches());
        observed.key = "/opt/neural/Factory".into();
        observed.is_factory = true;
        guard.observe(&observed);
        assert!(guard.matches());
        observed.key = "/media/p4/Presets/Added".into();
        observed.is_factory = false;
        guard.observe(&observed);
        assert!(!guard.matches());
        let mut guard = CatalogGuard::new(&folders, &[], true);
        let mut renamed = original;
        renamed.name = "Renamed folder".into();
        guard.observe(&renamed);
        assert!(!guard.matches());
    }
}
