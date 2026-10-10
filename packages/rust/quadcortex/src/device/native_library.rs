//! Capture and IR publication state. Unlike preset slots, these records have
//! opaque keys and no fixed-size inventory or keyless empty-slot encoding.
//!
//! Projections retain wire metadata: the display default version "1" is
//! not evidence that the device supplied a version field.
use crate::proto::cortex_protobuf_v2 as pa;
use crate::responses::{LibraryEntries, LibraryEntry, LibraryFolder};
use crate::sync::allocate_next_id;
use prost::Message as _;
use std::collections::{BTreeMap, VecDeque};

// Hard bounds on what one provider (Capture or IR) may retain from
// device-supplied listings. Every listing is opaque device data, so without
// these a misbehaving or hostile peer could grow the catalog without limit
// across messages even though each single frame is size-capped
// (`profile::MAX_INFLATED_BYTES`). The values sit far above any real Quad
// Cortex library: the embedded factory capture tree is 179 folders, and a
// unit's factory plus user captures/IRs number in the low thousands with
// names of a few dozen characters. Exceeding a bound is an error that latches
// on that provider exactly like any other reduction failure (the retained
// tree and staged startup data are dropped, reads report the error) until
// the catalog is cleared for a new session.
/// Folder nodes per provider, including the embedded seed topology.
pub const MAX_LIBRARY_FOLDERS: usize = 4_096;
/// Records (captures or IRs) per provider across all folders.
pub const MAX_LIBRARY_RECORDS: usize = 65_536;
/// Bytes in any single retained string (key, name, author, description...).
pub const MAX_LIBRARY_STRING_BYTES: usize = 16 * 1024;
/// Retained string bytes per provider, counted separately for the published
/// tree and for listings staged during startup.
pub const MAX_LIBRARY_BYTES: usize = 32 * 1024 * 1024;

fn bounded_text(value: &str, what: &str) -> Result<(), String> {
    if value.len() > MAX_LIBRARY_STRING_BYTES {
        return Err(format!(
            "Native library {what} exceeds {MAX_LIBRARY_STRING_BYTES} bytes"
        ));
    }
    Ok(())
}

fn bounded_folder_text(source: &pa::FolderInfo) -> Result<(), String> {
    bounded_text(folder_key(source).unwrap_or_default(), "folder key")?;
    bounded_text(folder_name(source), "folder name")
}

fn entry_bytes(entry: &LibraryEntry) -> usize {
    [
        Some(&entry.name),
        Some(&entry.key),
        entry.author.as_ref(),
        entry.description.as_ref(),
        entry.date.as_ref(),
        entry.product_key.as_ref(),
        entry.version.as_ref(),
        entry.folder_key.as_ref(),
        entry.folder_name.as_ref(),
    ]
    .into_iter()
    .flatten()
    .map(String::len)
    .sum()
}

/// The two library providers this catalog tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LibraryKind {
    /// Impulse responses.
    Ir,
    /// Neural captures.
    Capture,
}

impl LibraryKind {
    /// Key of this provider's root folder.
    pub const fn root_key(self) -> &'static str {
        match self {
            Self::Capture => "local_nc_root",
            Self::Ir => "local_ir_root",
        }
    }
    pub(crate) fn from_message(message: &pa::FileMessage) -> Option<Self> {
        match message.r#type {
            Some(pa::file_message::Type::Type(1)) => Some(Self::Ir),
            Some(pa::file_message::Type::Type(2)) => Some(Self::Capture),
            // The receiver dispatches absent type as protobuf's default 0,
            // which selects the preset provider, regardless of the folder key.
            _ => None,
        }
    }

    /// Whether folder construction for this kind must fully complete --
    /// `ensure()` for every staged folder, then known-folder registration --
    /// before any folder's records are applied via `update_records()`. Only
    /// `Capture` needs this today: the augmented known-folders map is
    /// installed only after all folder construction, then
    /// items are applied without constructing again. Naming the requirement here
    /// (rather than `publish_folders` special-casing `LibraryKind::Capture`
    /// by literal comparison) gives it one discoverable, single-point-of-truth
    /// definition; a new `LibraryKind` variant still has to be added to this
    /// `matches!` by hand, same as any other closed-enum match in this file.
    pub(crate) fn requires_deferred_known_folders(self) -> bool {
        matches!(self, Self::Capture)
    }

    fn canonical_key(self, key: &str) -> &str {
        if key.trim_end_matches('/') == self.root_key() {
            self.root_key()
        } else {
            key
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FolderType {
    LocalFactory,
    Downloads,
    LocalUser,
    Library,
}

impl FolderType {
    fn from_source(source: &pa::FolderInfo) -> Self {
        let key = folder_key(source).unwrap_or_default();
        // Existing embedded models retain their type in ensure(). A newly
        // created model derives its type from this descriptor's flags.
        if is_factory(source) {
            Self::LocalFactory
        } else if matches!(
            source.is_downloads,
            Some(pa::folder_info::IsDownloads::IsDownloads(true))
        ) {
            Self::Downloads
        } else if has_library_root_pattern(key) {
            Self::Library
        } else {
            Self::LocalUser
        }
    }
}

// Equivalent to the narrow-string regex search `local_.+_root`.
// It classifies a model; it must not rewrite the public opaque folder key.
fn has_library_root_pattern(key: &str) -> bool {
    key.split('\0')
        .next()
        .unwrap_or_default()
        .split(['\r', '\n'])
        .any(|line| {
            line.find("local_")
                .is_some_and(|start| line[start + 6..].rfind("_root").is_some_and(|end| end > 0))
        })
}

struct FolderSeed {
    parent: Option<u64>,
    key: &'static str,
    name: &'static str,
    folder_type: FolderType,
}

include!("native_library_seeds.rs");

#[derive(Clone)]
struct NativeRecord {
    entry: LibraryEntry,
    // Both native item handlers recompute this on every supplied item update.
    // ProductData.is_readonly contributes even when FolderInfo.is_factory=false.
    is_factory: bool,
}

#[derive(Clone)]
struct Folder {
    key: String,
    parent: Option<u64>,
    children: Vec<u64>,
    observed: bool,
    name: String,
    model_name: String,
    folder_type: FolderType,
    records: Vec<NativeRecord>,
}

impl Folder {
    fn new(
        source: &pa::FolderInfo,
        key: String,
        parent: Option<u64>,
        model_name: String,
        observed: bool,
    ) -> Self {
        Self {
            key,
            parent,
            children: Vec::new(),
            observed,
            name: folder_name(source).to_owned(),
            model_name,
            folder_type: FolderType::from_source(source),
            records: Vec::new(),
        }
    }
}

// Native model identities are separate from folder keys. Two parents can own
// children with the same key; neither sorted-key iteration nor string prefixes
// expresses the required traversal or deletion behavior.
struct FolderTree {
    nodes: BTreeMap<u64, Folder>,
    roots: Vec<u64>,
    next_id: u64,
    lookup_cache: BTreeMap<String, u64>,
    known_folders: BTreeMap<String, String>,
}

impl FolderTree {
    fn new(kind: LibraryKind) -> Self {
        // Static folder topology supplies no observed rows. IDs preserve the
        // embedded tree's order; its outer provider container is represented by
        // roots, not exposed as a folder with an empty public key.
        let seeds = match kind {
            LibraryKind::Capture => CAPTURE_FOLDER_SEEDS,
            LibraryKind::Ir => IR_FOLDER_SEEDS,
        };
        let mut nodes: BTreeMap<u64, Folder> = BTreeMap::new();
        let mut roots = Vec::new();
        for (index, seed) in seeds.iter().enumerate() {
            let id = index as u64 + 1;
            nodes.insert(
                id,
                Folder {
                    key: seed.key.into(),
                    parent: seed.parent,
                    children: Vec::new(),
                    observed: false,
                    name: String::new(),
                    model_name: seed.name.into(),
                    folder_type: seed.folder_type,
                    records: Vec::new(),
                },
            );
            match seed.parent {
                Some(parent) => nodes
                    .get_mut(&parent)
                    .expect("generated parent precedes child")
                    .children
                    .push(id),
                None => roots.push(id),
            }
        }
        Self {
            nodes,
            roots,
            next_id: seeds.len() as u64 + 1,
            lookup_cache: BTreeMap::new(),
            known_folders: match kind {
                LibraryKind::Capture => CAPTURE_KNOWN_FOLDERS,
                LibraryKind::Ir => IR_KNOWN_FOLDERS,
            }
            .iter()
            .map(|(key, name)| ((*key).into(), (*name).into()))
            .collect(),
        }
    }

    // Creation resolves parent_key breadth first, independently of the normal
    // resolver cache. Missing parents mean the provider root now;
    // a later publication of that parent must not silently move this node.
    fn parent(&self, key: &str) -> Option<u64> {
        if key.is_empty() {
            return None;
        }
        let mut pending: VecDeque<_> = self.roots.iter().copied().collect();
        while let Some(id) = pending.pop_front() {
            let node = &self.nodes[&id];
            if node.key == key {
                return Some(id);
            }
            pending.extend(node.children.iter().copied());
        }
        None
    }

    // The ordinary resolver walks ordered descendants depth first after its
    // weak-cache check. Read projections do not
    // install additional protocol lookup cache entries.
    fn resolve(&self, key: &str) -> Option<u64> {
        if let Some(id) = self.lookup_cache.get(key) {
            if self.nodes.contains_key(id) {
                return Some(*id);
            }
        }
        let mut pending: Vec<_> = self.roots.iter().rev().copied().collect();
        while let Some(id) = pending.pop() {
            let node = &self.nodes[&id];
            if node.key == key {
                return Some(id);
            }
            pending.extend(node.children.iter().rev().copied());
        }
        None
    }

    fn resolve_operation(&mut self, key: &str) -> Option<u64> {
        let id = self.resolve(key)?;
        self.lookup_cache.insert(key.into(), id);
        Some(id)
    }

    fn ensure(&mut self, source: &pa::FolderInfo, key: &str) -> Result<bool, String> {
        let mut parent = match &source.parent_key {
            Some(pa::folder_info::ParentKey::ParentKey(key)) => self.parent(key),
            None => None,
        };
        // Provider-known keys are atomic, including the factory IR key with
        // slashes. The generic JUCE tokenizer preserves empty segments.
        let segments: Vec<_> = if self.known_folders.contains_key(key) {
            vec![key]
        } else {
            key.split('/').collect()
        };
        let mut changed = false;
        for (index, segment) in segments.iter().enumerate() {
            let model_name = self
                .known_folders
                .get(*segment)
                .map(String::as_str)
                .unwrap_or_else(|| folder_name(source));
            let siblings = parent.map_or(&self.roots, |id| &self.nodes[&id].children);
            let existing = siblings
                .iter()
                .find(|id| self.nodes[id].key == *segment)
                .copied();
            // A constructed prefix is not a device publication for that key.
            let observed = segments.len() == 1;
            if let Some(id) = existing {
                let node = self.nodes.get_mut(&id).unwrap();
                if observed && !node.observed {
                    node.name = folder_name(source).into();
                    node.observed = true;
                    changed = true;
                }
            } else {
                // Checked per node so one key with many '/' segments cannot
                // build an unbounded chain inside a single message.
                if self.nodes.len() >= MAX_LIBRARY_FOLDERS {
                    return Err(format!(
                        "Native library exceeded its capacity of {MAX_LIBRARY_FOLDERS} folders"
                    ));
                }
                let id = allocate_next_id(&mut self.next_id, "Native library folder")?;
                self.nodes.insert(
                    id,
                    Folder::new(
                        source,
                        (*segment).into(),
                        parent,
                        model_name.into(),
                        observed,
                    ),
                );
                match parent {
                    Some(parent) => self.nodes.get_mut(&parent).unwrap().children.push(id),
                    None => self.roots.push(id),
                }
                changed = true;
            }
            if index + 1 < segments.len() {
                // Native recursion reselects by folderName, not the key match
                // above. A missing name produces a null native parent; reject
                // that malformed model operation instead of fabricating one.
                let siblings = parent.map_or(&self.roots, |id| &self.nodes[&id].children);
                parent = Some(
                    *siblings
                        .iter()
                        .find(|id| self.nodes[id].model_name == model_name)
                        .ok_or(
                            "Native library construction could not resolve the next parent by name",
                        )?,
                );
            }
        }
        Ok(changed)
    }

    // Folder construction and item application are separate native operations.
    // Startup NC builds all folders before visiting any records.
    fn update_records(
        &mut self,
        source: &pa::FolderInfo,
        incoming: Vec<Option<LibraryEntry>>,
    ) -> bool {
        // Empty-folder CREATE shares the startup buffer, but its empty item
        // loop must not bind an operation lookup to a duplicate-key model.
        if incoming.is_empty() {
            return false;
        }
        let Some(key) = folder_key(source) else {
            return false;
        };
        let Some(id) = self.resolve_operation(key) else {
            return false;
        };
        let folder = self.nodes.get_mut(&id).unwrap();
        if !folder.observed {
            folder.name = folder_name(source).into();
            folder.observed = true;
        }
        for (wire, entry) in source.files.iter().zip(incoming) {
            let Some(entry) = entry else { continue };
            let lookup = match &wire.old_key {
                Some(pa::product_data::OldKey::OldKey(key)) => key,
                None => &entry.key,
            };
            let index = folder
                .records
                .iter()
                .position(|item| &item.entry.key == lookup);
            let record = NativeRecord {
                entry,
                is_factory: is_factory(source)
                    || matches!(
                        wire.is_readonly,
                        Some(pa::product_data::IsReadonly::IsReadonly(true))
                    ),
            };
            if let Some(index) = index {
                folder.records[index] = record;
            } else {
                folder.records.push(record);
            }
        }
        true
    }

    fn remove(&mut self, id: u64) {
        let parent = self.nodes[&id].parent;
        match parent {
            Some(parent) => self
                .nodes
                .get_mut(&parent)
                .unwrap()
                .children
                .retain(|child| *child != id),
            None => self.roots.retain(|child| *child != id),
        }
        let mut pending = vec![id];
        while let Some(id) = pending.pop() {
            if let Some(node) = self.nodes.remove(&id) {
                pending.extend(node.children);
            }
        }
        self.lookup_cache
            .retain(|_, id| self.nodes.contains_key(id));
    }

    fn check_capacity(&self) -> Result<(), String> {
        if self.nodes.len() > MAX_LIBRARY_FOLDERS {
            return Err(format!(
                "Native library exceeded its capacity of {MAX_LIBRARY_FOLDERS} folders"
            ));
        }
        let mut records = 0usize;
        let mut bytes = 0usize;
        for folder in self.nodes.values() {
            records += folder.records.len();
            bytes += folder.key.len() + folder.name.len() + folder.model_name.len();
            bytes += folder
                .records
                .iter()
                .map(|record| entry_bytes(&record.entry))
                .sum::<usize>();
        }
        if records > MAX_LIBRARY_RECORDS {
            return Err(format!(
                "Native library exceeded its capacity of {MAX_LIBRARY_RECORDS} records"
            ));
        }
        if bytes > MAX_LIBRARY_BYTES {
            return Err(format!(
                "Native library exceeded its capacity of {MAX_LIBRARY_BYTES} bytes"
            ));
        }
        Ok(())
    }
}

struct StagedFolder {
    source: pa::FolderInfo,
    entries: Vec<Option<LibraryEntry>>,
}

/// Running totals for one provider's staged startup listings, which are
/// append-only until published or cleared.
#[derive(Default, Clone, Copy)]
struct StagedUsage {
    folders: usize,
    records: usize,
    bytes: usize,
}

/// Capture and IR folder trees reduced from native File messages.
///
/// Each provider keeps its own tree. A failure, including exceeding the
/// `MAX_LIBRARY_*` bounds, latches on that provider and makes reads and updates
/// for it return the error until [`NativeLibraryCatalog::clear`].
#[derive(Default)]
pub struct NativeLibraryCatalog {
    trees: BTreeMap<LibraryKind, FolderTree>,
    errors: BTreeMap<LibraryKind, String>,
    staged: BTreeMap<LibraryKind, Vec<StagedFolder>>,
    staged_usage: BTreeMap<LibraryKind, StagedUsage>,
}

impl NativeLibraryCatalog {
    /// Defer folder creation and item updates until native lazy-load publication.
    /// Folder renames and all deletions retain their normal dispatch.
    pub(crate) fn stage_or_apply(&mut self, message: &pa::FileMessage) -> Result<bool, String> {
        let Some(kind) = LibraryKind::from_message(message) else {
            return Ok(false);
        };
        let Some(pa::file_message::Folder::Folder(source)) = &message.folder else {
            return Ok(false);
        };
        if !matches!(message.action, 0 | 1) || (message.action == 1 && source.files.is_empty()) {
            return self.apply(message);
        }
        if let Some(error) = self.errors.get(&kind) {
            return Err(error.clone());
        }
        if folder_key(source).is_none() {
            return Ok(false);
        }
        let entries = match bounded_folder_text(source).and_then(|()| {
            source
                .files
                .iter()
                .map(|file| project(source, file))
                .collect::<Result<Vec<_>, _>>()
        }) {
            Ok(entries) => entries,
            Err(error) => {
                self.errors.insert(kind, error.clone());
                return Err(error);
            }
        };
        if let Err(error) = self.reserve_staged(kind, source, &entries) {
            self.fail_capacity(kind, &error);
            return Err(error);
        }
        self.staged.entry(kind).or_default().push(StagedFolder {
            source: source.clone(),
            entries,
        });
        Ok(true)
    }

    /// Account for one more staged listing, refusing it once the provider's
    /// staged startup data would exceed the retained-state bounds.
    fn reserve_staged(
        &mut self,
        kind: LibraryKind,
        source: &pa::FolderInfo,
        entries: &[Option<LibraryEntry>],
    ) -> Result<(), String> {
        let usage = self.staged_usage.get(&kind).copied().unwrap_or_default();
        let next = StagedUsage {
            folders: usage.folders + 1,
            records: usage.records + entries.iter().flatten().count(),
            // The staged copy keeps both the wire folder and its projection.
            bytes: usage.bytes
                + source.encoded_len()
                + entries.iter().flatten().map(entry_bytes).sum::<usize>(),
        };
        if next.folders > MAX_LIBRARY_FOLDERS + MAX_LIBRARY_RECORDS {
            return Err(format!(
                "Native library staged more than {} startup listings",
                MAX_LIBRARY_FOLDERS + MAX_LIBRARY_RECORDS
            ));
        }
        if next.records > MAX_LIBRARY_RECORDS {
            return Err(format!(
                "Native library exceeded its capacity of {MAX_LIBRARY_RECORDS} records"
            ));
        }
        if next.bytes > MAX_LIBRARY_BYTES {
            return Err(format!(
                "Native library exceeded its capacity of {MAX_LIBRARY_BYTES} bytes"
            ));
        }
        self.staged_usage.insert(kind, next);
        Ok(())
    }

    /// Drop everything retained for a provider that overflowed its bounds and
    /// latch the failure, so the excess is released rather than kept behind
    /// an error.
    fn fail_capacity(&mut self, kind: LibraryKind, error: &str) {
        self.trees.remove(&kind);
        self.staged.remove(&kind);
        self.staged_usage.remove(&kind);
        self.errors.insert(kind, error.to_owned());
    }

    fn enforce_capacity(&mut self, kind: LibraryKind) -> Result<(), String> {
        let Some(tree) = self.trees.get(&kind) else {
            return Ok(());
        };
        if let Err(error) = tree.check_capacity() {
            self.fail_capacity(kind, &error);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn publish_staged(&mut self, kind: LibraryKind) -> Result<bool, String> {
        if let Some(error) = self.errors.get(&kind) {
            return Err(error.clone());
        }
        self.staged_usage.remove(&kind);
        let Some(staged) = self.staged.remove(&kind) else {
            return Ok(false);
        };
        let result = self
            .publish_folders(kind, staged)
            .and_then(|changed| self.enforce_capacity(kind).map(|()| changed));
        if let Err(error) = &result {
            self.errors.insert(kind, error.clone());
        }
        result
    }

    fn publish_folders(
        &mut self,
        kind: LibraryKind,
        staged: Vec<StagedFolder>,
    ) -> Result<bool, String> {
        let tree = self
            .trees
            .entry(kind)
            .or_insert_with(|| FolderTree::new(kind));
        if kind.requires_deferred_known_folders() {
            // The augmented known-folders map is installed only after all
            // folder construction, then items are applied without constructing again.
            for folder in &staged {
                tree.ensure(
                    &folder.source,
                    folder_key(&folder.source).expect("validated staged key"),
                )?;
            }
            for folder in &staged {
                if is_factory(&folder.source) {
                    let key = folder_key(&folder.source).expect("validated staged key");
                    tree.known_folders
                        .entry(key.into())
                        .or_insert_with(|| key.into());
                }
            }
            for folder in staged {
                tree.update_records(&folder.source, folder.entries);
            }
        } else {
            for folder in staged {
                tree.ensure(
                    &folder.source,
                    folder_key(&folder.source).expect("validated staged key"),
                )?;
                tree.update_records(&folder.source, folder.entries);
            }
        }
        Ok(true)
    }

    /// Drops every tree, staged listing and latched error, for a new session.
    pub fn clear(&mut self) {
        self.trees.clear();
        self.errors.clear();
        self.staged.clear();
        self.staged_usage.clear();
    }

    /// Latches `error` on both providers. A provider that already has an error
    /// keeps its first one.
    pub fn invalidate(&mut self, error: String) {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            self.errors.entry(kind).or_insert_with(|| error.clone());
        }
    }

    /// Projects the folder `key` of provider `kind` into its folders and
    /// records.
    ///
    /// Returns the latched error if the provider has failed, and `Ok(None)`
    /// when there is no tree, the key is unknown, or the folder has not been
    /// observed yet. A read of the Capture root includes every observed
    /// descendant folder; any other read covers just the one folder.
    pub fn folder(&self, kind: LibraryKind, key: &str) -> Result<Option<LibraryEntries>, String> {
        if let Some(error) = self.errors.get(&kind) {
            return Err(error.clone());
        }
        // Preserve the public root aliases without normalizing arbitrary
        // opaque folder identities as filesystem paths.
        let key = kind.canonical_key(key);
        let Some(tree) = self.trees.get(&kind) else {
            return Ok(None);
        };
        let Some(root_id) = tree.resolve(key) else {
            return Ok(None);
        };
        if !tree.nodes[&root_id].observed {
            return Ok(None);
        }

        // Capture startup publication is a folder tree. Projecting only the
        // root silently discarded every child folder and all captures in it.
        // Root capture reads are inventory reads; targeted IR reads keep their
        // existing one-folder semantics.
        let recursive = kind == LibraryKind::Capture && key == kind.root_key();
        let mut pending = vec![root_id];
        let mut entries = Vec::new();
        let mut folders = Vec::new();
        while let Some(id) = pending.pop() {
            let folder = &tree.nodes[&id];
            if !folder.observed {
                continue;
            }
            let parent_key = folder
                .parent
                .and_then(|parent| tree.nodes.get(&parent))
                .map(|parent| parent.key.clone());
            folders.push(LibraryFolder {
                key: folder.key.clone(),
                name: folder.name.clone(),
                parent_key,
                is_factory: folder.folder_type == FolderType::LocalFactory,
                is_downloads: folder.folder_type == FolderType::Downloads,
                is_library: folder.folder_type == FolderType::Library,
            });
            entries.extend(folder.records.iter().map(|record| {
                let mut entry = record.entry.clone();
                entry.folder_key = Some(folder.key.clone());
                entry.folder_name = Some(folder.name.clone());
                entry
            }));
            if recursive {
                pending.extend(folder.children.iter().rev().copied());
            }
        }
        Ok(Some(LibraryEntries { entries, folders }))
    }

    /// Whether a supplied operation contributed to retained state. Ignored
    /// provider actions must not acquire or invalidate publication provenance.
    pub fn apply(&mut self, message: &pa::FileMessage) -> Result<bool, String> {
        let Some(kind) = LibraryKind::from_message(message) else {
            return Ok(false);
        };
        if !matches!(message.action, 0..=2) {
            return Ok(false);
        }
        if let Some(error) = self.errors.get(&kind) {
            return Err(error.clone());
        }
        // A newly bound resolver entry can change which duplicate-key node a
        // later operation visits, even if the current deletion is refused.
        // Cache entries are only added by lookup and removed with a subtree;
        // IDs are never reused, so cardinality captures otherwise-no-op binds.
        let cache_size = self
            .trees
            .get(&kind)
            .map_or(0, |tree| tree.lookup_cache.len());
        let result = self
            .apply_inner(kind, message)
            .and_then(|changed| self.enforce_capacity(kind).map(|()| changed));
        if let Err(error) = &result {
            self.errors.insert(kind, error.clone());
        }
        result.map(|changed| {
            changed
                || self
                    .trees
                    .get(&kind)
                    .map_or(0, |tree| tree.lookup_cache.len())
                    != cache_size
        })
    }

    fn apply_inner(
        &mut self,
        kind: LibraryKind,
        message: &pa::FileMessage,
    ) -> Result<bool, String> {
        let Some(pa::file_message::Folder::Folder(source)) = &message.folder else {
            return Ok(false);
        };
        let Some(source_key) = folder_key(source) else {
            return Ok(false);
        };
        let key = source_key;
        if matches!(message.action, 0 | 1) {
            bounded_folder_text(source)?;
        }
        match (message.action, source.files.is_empty()) {
            (0, true) => {
                return self
                    .trees
                    .entry(kind)
                    .or_insert_with(|| FolderTree::new(kind))
                    .ensure(source, key);
            }
            (1, true) => {
                // Renaming a folder looks up the selected FolderData key and
                // sets its name. It does not re-key a tree or derive new
                // product keys from source/destination names.
                let selected = match &message.to_folder {
                    Some(pa::file_message::ToFolder::ToFolder(folder)) => folder,
                    None => source,
                };
                if let Some(key) = folder_key(selected) {
                    bounded_folder_text(selected)?;
                    if let Some(tree) = self.trees.get_mut(&kind) {
                        if let Some(id) = tree.resolve_operation(key) {
                            let folder = tree.nodes.get_mut(&id).unwrap();
                            folder.name = folder_name(selected).to_owned();
                            folder.model_name = folder_name(selected).to_owned();
                            folder.observed = true;
                            return Ok(true);
                        }
                    }
                }
            }
            (2, true) => {
                // Folder-delete handling exits on the incoming FolderData
                // factory flag.
                if is_factory(source) {
                    return Ok(false);
                }
                let Some(tree) = self.trees.get_mut(&kind) else {
                    return Ok(false);
                };
                let Some(id) = tree.resolve_operation(key) else {
                    return Ok(false);
                };
                let folder = tree.nodes.get_mut(&id).unwrap();
                match folder.folder_type {
                    FolderType::LocalFactory => return Ok(false),
                    FolderType::Library => {
                        // The library container survives; its native
                        // isFactory items survive with it. Child folders
                        // belong to a separate subfolders model; this branch
                        // does not delete or recurse into those models.
                        let before = folder.records.len();
                        folder.records.retain(|record| record.is_factory);
                        return Ok(before != folder.records.len());
                    }
                    FolderType::Downloads | FolderType::LocalUser => {}
                }
                tree.remove(id);
                return Ok(true);
            }
            (0 | 1, false) => {
                // Validate/project all supplied records before changing state.
                // Omitted fields describe this record update, not a patch to
                // its previous metadata. Omitted *records* remain untouched.
                let incoming = source
                    .files
                    .iter()
                    .map(|file| project(source, file))
                    .collect::<Result<Vec<_>, _>>()?;
                let tree = self
                    .trees
                    .entry(kind)
                    .or_insert_with(|| FolderTree::new(kind));
                let created = tree.ensure(source, key)?;
                return Ok(created | tree.update_records(source, incoming));
            }

            (2, false) => {
                if let Some(tree) = self.trees.get_mut(&kind) {
                    let Some(id) = tree.resolve_operation(key) else {
                        return Ok(false);
                    };
                    let folder = tree.nodes.get_mut(&id).unwrap();
                    let before = folder.records.len();
                    folder.records.retain(|entry| !source.files.iter().any(|file| {
                        matches!(&file.key, Some(pa::product_data::Key::Key(key)) if !key.is_empty() && key == &entry.entry.key)
                    }));
                    return Ok(before != folder.records.len());
                }
            }
            _ => {}
        }
        Ok(false)
    }
}

fn is_factory(folder: &pa::FolderInfo) -> bool {
    matches!(
        folder.is_factory,
        Some(pa::folder_info::IsFactory::IsFactory(true))
    )
}

fn folder_key(folder: &pa::FolderInfo) -> Option<&str> {
    match &folder.key {
        Some(pa::folder_info::Key::Key(key)) if !key.is_empty() => Some(key),
        _ => None,
    }
}

fn folder_name(folder: &pa::FolderInfo) -> &str {
    match &folder.name {
        Some(pa::folder_info::Name::Name(name)) => name,
        None => "",
    }
}

fn project(
    folder: &pa::FolderInfo,
    file: &pa::ProductData,
) -> Result<Option<LibraryEntry>, String> {
    let Some(pa::product_data::Key::Key(key)) = &file.key else {
        return Ok(None);
    };
    // A keyless library row provides no addressable product identity. It must
    // never clear another record sharing an index, as preset empty slots do.
    if key.is_empty() {
        return Ok(None);
    }
    bounded_text(key, "record key")?;
    for (value, what) in [
        (
            file.name
                .as_ref()
                .map(|pa::product_data::Name::Name(value)| value),
            "record name",
        ),
        (
            file.author
                .as_ref()
                .map(|pa::product_data::Author::Author(value)| value),
            "record author",
        ),
        (
            file.description
                .as_ref()
                .map(|pa::product_data::Description::Description(value)| value),
            "record description",
        ),
        (
            file.date
                .as_ref()
                .map(|pa::product_data::Date::Date(value)| value),
            "record date",
        ),
        (
            file.version
                .as_ref()
                .map(|pa::product_data::Version::Version(value)| value),
            "record version",
        ),
    ] {
        bounded_text(value.map_or("", String::as_str), what)?;
    }
    let position = match file.index {
        Some(pa::product_data::Index::Index(index)) => Some(
            u32::try_from(index)
                .map_err(|_| "Native library record has a negative position".to_owned())?,
        ),
        None => None,
    };
    Ok(Some(LibraryEntry {
        key: key.clone(),
        product_key: Some(key.clone()),
        name: match &file.name {
            Some(pa::product_data::Name::Name(name)) => name.clone(),
            None => String::new(),
        },
        author: file
            .author
            .as_ref()
            .map(|pa::product_data::Author::Author(value)| value.clone())
            .filter(|value| !value.is_empty()),
        description: file
            .description
            .as_ref()
            .map(|pa::product_data::Description::Description(value)| value.clone())
            .filter(|value| !value.is_empty()),
        date: file
            .date
            .as_ref()
            .map(|pa::product_data::Date::Date(value)| value.clone())
            .filter(|value| !value.is_empty()),
        version: file
            .version
            .as_ref()
            .map(|pa::product_data::Version::Version(value)| value.clone()),
        folder_key: folder_key(folder).map(str::to_owned),
        folder_name: Some(folder_name(folder).to_owned()),
        position,
        instrument: file
            .instrument
            .map(|pa::product_data::Instrument::Instrument(value)| value),
        is_factory: matches!(
            folder.is_factory,
            Some(pa::folder_info::IsFactory::IsFactory(true))
        ),
        is_plugin: false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(key: &str) -> pa::ProductData {
        pa::ProductData {
            key: Some(pa::product_data::Key::Key(key.into())),
            name: Some(pa::product_data::Name::Name(format!("Name {key}"))),
            ..Default::default()
        }
    }
    fn message(
        kind: LibraryKind,
        action: i32,
        key: &str,
        files: Vec<pa::ProductData>,
    ) -> pa::FileMessage {
        pa::FileMessage {
            action,
            r#type: Some(pa::file_message::Type::Type(match kind {
                LibraryKind::Ir => 1,
                LibraryKind::Capture => 2,
            })),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(key.into())),
                name: Some(pa::folder_info::Name::Name("Library".into())),
                files,
                ..Default::default()
            })),
            ..Default::default()
        }
    }
    fn rows(catalog: &NativeLibraryCatalog, kind: LibraryKind, key: &str) -> Vec<LibraryEntry> {
        catalog.folder(kind, key).unwrap().unwrap().entries
    }

    fn under(mut message: pa::FileMessage, parent: &str) -> pa::FileMessage {
        let Some(pa::file_message::Folder::Folder(folder)) = &mut message.folder else {
            unreachable!()
        };
        folder.parent_key = Some(pa::folder_info::ParentKey::ParentKey(parent.into()));
        message
    }

    #[test]
    fn startup_capture_map_changes_after_folder_creation_and_never_replays_drained_rows() {
        let kind = LibraryKind::Capture;
        let mut catalog = NativeLibraryCatalog::default();
        let mut first = message(kind, 0, "factory/path", vec![record("staged")]);
        let Some(pa::file_message::Folder::Folder(folder)) = &mut first.folder else {
            unreachable!()
        };
        folder.is_factory = Some(pa::folder_info::IsFactory::IsFactory(true));
        catalog.stage_or_apply(&first).unwrap();
        assert!(catalog.folder(kind, "factory/path").unwrap().is_none());
        catalog.publish_staged(kind).unwrap();
        // Construction used the old map; applying items after augmenting it
        // must not construct the now-atomic complete key a second time.
        assert!(catalog.folder(kind, "factory/path").unwrap().is_none());
        assert!(catalog.trees[&kind]
            .known_folders
            .contains_key("factory/path"));
        catalog
            .apply(&message(kind, 1, "factory/path", vec![record("later")]))
            .unwrap();
        assert_eq!(
            rows(&catalog, kind, "factory/path")
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>(),
            ["later"]
        );
        assert!(!catalog.publish_staged(kind).unwrap());
        catalog.clear();
        catalog.apply(&first).unwrap();
        assert!(catalog.folder(kind, "factory/path").unwrap().is_none());
        assert!(!catalog.trees[&kind]
            .known_folders
            .contains_key("factory/path"));
    }

    #[test]
    fn startup_capture_builds_all_folders_before_items_while_ir_binds_each_in_order() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            catalog
                .stage_or_apply(&message(kind, 0, "duplicate", vec![record("first")]))
                .unwrap();
            catalog
                .stage_or_apply(&under(
                    message(kind, 1, "duplicate", vec![record("second")]),
                    kind.root_key(),
                ))
                .unwrap();
            catalog.publish_staged(kind).unwrap();
            let tree = &catalog.trees[&kind];
            let selected = &tree.nodes[&tree.resolve("duplicate").unwrap()];
            assert_eq!(
                selected.parent,
                if kind == LibraryKind::Capture {
                    tree.resolve(kind.root_key())
                } else {
                    None
                }
            );
            assert_eq!(
                rows(&catalog, kind, "duplicate")
                    .iter()
                    .map(|row| row.key.as_str())
                    .collect::<Vec<_>>(),
                ["first", "second"]
            );
        }
    }

    #[test]
    fn startup_deletions_bypass_staging_and_ir_does_not_gain_capture_factory_key_rules() {
        let kind = LibraryKind::Ir;
        let mut catalog = NativeLibraryCatalog::default();
        catalog
            .stage_or_apply(&message(kind, 0, "ordinary", vec![record("pending")]))
            .unwrap();
        catalog
            .stage_or_apply(&message(kind, 2, "ordinary", vec![record("pending")]))
            .unwrap();
        let mut read = message(kind, 3, "ignored", vec![record("not-a-publication")]);
        assert!(!catalog.stage_or_apply(&read).unwrap());
        read.action = 0;
        let Some(pa::file_message::Folder::Folder(folder)) = &mut read.folder else {
            unreachable!()
        };
        folder.key = Some(pa::folder_info::Key::Key("factory/path".into()));
        folder.is_factory = Some(pa::folder_info::IsFactory::IsFactory(true));
        catalog.stage_or_apply(&read).unwrap();
        catalog.publish_staged(kind).unwrap();
        assert_eq!(rows(&catalog, kind, "ordinary")[0].key, "pending");
        assert!(catalog.folder(kind, "factory/path").unwrap().is_none());
        assert!(catalog.folder(kind, "ignored").unwrap().is_none());
        catalog.apply(&read).unwrap();
        assert!(catalog.folder(kind, "factory/path").unwrap().is_none());
    }

    #[test]
    fn startup_stages_empty_folder_creation_but_not_folder_rename_or_delete() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            let creation = message(kind, 0, "pending", vec![]);
            assert!(catalog.stage_or_apply(&creation).unwrap());
            assert!(catalog.folder(kind, "pending").unwrap().is_none());
            let mut rename = message(kind, 1, "pending", vec![]);
            let Some(pa::file_message::Folder::Folder(folder)) = &mut rename.folder else {
                unreachable!()
            };
            folder.name = Some(pa::folder_info::Name::Name("Premature rename".into()));
            assert!(!catalog.stage_or_apply(&rename).unwrap());
            assert!(!catalog
                .stage_or_apply(&message(kind, 2, "pending", vec![]))
                .unwrap());
            assert!(catalog.publish_staged(kind).unwrap());
            assert!(rows(&catalog, kind, "pending").is_empty());
            let tree = &catalog.trees[&kind];
            assert_eq!(
                tree.nodes[&tree.resolve("pending").unwrap()].name,
                "Library"
            );
            assert!(!tree.lookup_cache.contains_key("pending"));
            catalog.stage_or_apply(&rename).unwrap();
            let tree = &catalog.trees[&kind];
            assert_eq!(
                tree.nodes[&tree.resolve("pending").unwrap()].name,
                "Premature rename"
            );
            catalog
                .stage_or_apply(&message(kind, 2, "pending", vec![]))
                .unwrap();
            assert!(catalog.folder(kind, "pending").unwrap().is_none());
            assert!(!catalog.publish_staged(kind).unwrap());
        }
    }

    #[test]
    fn segmented_creation_preserves_empty_tokens_and_does_not_publish_a_full_key_listing() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            assert!(catalog
                .apply(&message(kind, 0, "/parent//leaf/", vec![record("skipped")]))
                .unwrap());
            for key in ["/parent//leaf/", "parent", "leaf"] {
                assert!(catalog.folder(kind, key).unwrap().is_none());
            }
            let tree = &catalog.trees[&kind];
            let mut siblings = &tree.roots;
            for key in ["", "parent", "", "leaf", ""] {
                let id = siblings
                    .iter()
                    .find(|id| tree.nodes[id].key == key)
                    .unwrap();
                let node = &tree.nodes[id];
                assert!(!node.observed);
                assert!(node.records.is_empty());
                siblings = &node.children;
            }
            assert!(siblings.is_empty());
        }
    }

    #[test]
    fn segmented_creation_reselects_parent_by_name_and_rejects_missing_name() {
        let kind = LibraryKind::Ir;
        let mut catalog = NativeLibraryCatalog::default();
        catalog.apply(&message(kind, 0, "other", vec![])).unwrap();
        let mut first = message(kind, 0, "first", vec![]);
        let Some(pa::file_message::Folder::Folder(folder)) = &mut first.folder else {
            unreachable!()
        };
        folder.name = Some(pa::folder_info::Name::Name("Different".into()));
        catalog.apply(&first).unwrap();
        catalog
            .apply(&message(kind, 0, "first/child", vec![record("skip")]))
            .unwrap();
        let tree = &catalog.trees[&kind];
        let child = tree.resolve("child").unwrap();
        assert_eq!(tree.nodes[&child].parent, tree.resolve("other"));
        assert!(tree.nodes[&tree.resolve("first").unwrap()]
            .children
            .is_empty());
        assert!(catalog.folder(kind, "first/child").unwrap().is_none());
        // With no same-name sibling, the native recursive parent is null.
        catalog.apply(&message(kind, 2, "other", vec![])).unwrap();
        assert!(catalog
            .apply(&message(kind, 0, "first/missing", vec![]))
            .unwrap_err()
            .contains("next parent by name"));
        assert!(catalog.folder(kind, "first").is_err());
        assert!(catalog
            .folder(LibraryKind::Capture, "first")
            .unwrap()
            .is_none());
    }

    #[test]
    fn wire_root_keys_are_exact_even_when_public_read_accepts_a_root_alias() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            let alias = format!("{}/", kind.root_key());
            catalog
                .apply(&message(kind, 0, &alias, vec![record("skip")]))
                .unwrap();
            assert!(catalog.folder(kind, kind.root_key()).unwrap().is_none());
            catalog
                .apply(&message(kind, 0, kind.root_key(), vec![record("keep")]))
                .unwrap();
            catalog
                .apply(&message(kind, 2, &alias, vec![record("keep")]))
                .unwrap();
            assert_eq!(rows(&catalog, kind, &alias)[0].key, "keep");
        }
    }

    #[test]
    fn embedded_folders_supply_parent_identity_without_publishing_device_rows() {
        let kind = LibraryKind::Capture;
        let mut catalog = NativeLibraryCatalog::default();
        catalog
            .apply(&under(
                message(kind, 0, "user-child", vec![record("live")]),
                "3_f",
            ))
            .unwrap();
        let tree = &catalog.trees[&kind];
        let child = tree.resolve("user-child").unwrap();
        let amp = tree.resolve("3_f").unwrap();
        assert_eq!(tree.nodes[&child].parent, Some(amp));
        let factory = tree.nodes[&amp].parent.unwrap();
        assert_eq!(tree.nodes[&factory].key, "1_f");
        assert_eq!(tree.nodes[&amp].model_name, "Amp");
        for key in ["3_f", "1_f", "local_nc_root", "104_f"] {
            assert!(catalog.folder(kind, key).unwrap().is_none(), "{key}");
        }
        assert_eq!(rows(&catalog, kind, "user-child")[0].key, "live");
        catalog.clear();
        assert!(catalog.folder(kind, "user-child").unwrap().is_none());
        catalog
            .apply(&under(
                message(kind, 0, "new-child", vec![record("new")]),
                "3_f",
            ))
            .unwrap();
        assert!(catalog.folder(kind, "user-child").unwrap().is_none());
        assert!(catalog.folder(kind, "3_f").unwrap().is_none());
        let tree = &catalog.trees[&kind];
        assert_eq!(
            tree.nodes[&tree.resolve("new-child").unwrap()].parent,
            tree.resolve("3_f")
        );
    }

    #[test]
    fn embedded_factory_ir_retains_type_without_incoming_factory_flag() {
        let kind = LibraryKind::Ir;
        let key = "/opt/neuraldsp/impulse_responses";
        let mut catalog = NativeLibraryCatalog::default();
        catalog
            .apply(&message(kind, 0, key, vec![record("device-ir")]))
            .unwrap();
        let tree = &catalog.trees[&kind];
        let id = tree.resolve(key).unwrap();
        assert_eq!(tree.nodes[&id].parent, None);
        assert_eq!(tree.nodes[&id].model_name, "Factory IRs");
        assert!(!catalog.apply(&message(kind, 2, key, vec![])).unwrap());
        let entry = &rows(&catalog, kind, key)[0];
        assert_eq!(entry.key, "device-ir");
        assert_eq!(entry.folder_name.as_deref(), Some("Library"));
        assert!(!entry.is_factory);
        assert!(catalog.folder(kind, kind.root_key()).unwrap().is_none());
        // Model protection is independent of explicit per-record deletion.
        catalog
            .apply(&message(kind, 2, key, vec![record("device-ir")]))
            .unwrap();
        assert!(rows(&catalog, kind, key).is_empty());
    }

    #[test]
    fn ancestor_delete_removes_attached_descendants_without_key_prefix_inference() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            catalog
                .apply(&message(kind, 0, "ancestor", vec![record("top")]))
                .unwrap();
            catalog
                .apply(&under(
                    message(kind, 0, "unrelated-key", vec![record("child")]),
                    "ancestor",
                ))
                .unwrap();
            let mut protected = under(
                message(kind, 0, "protected-descendant", vec![record("nested")]),
                "unrelated-key",
            );
            let Some(pa::file_message::Folder::Folder(folder)) = &mut protected.folder else {
                unreachable!()
            };
            folder.is_factory = Some(pa::folder_info::IsFactory::IsFactory(true));
            catalog.apply(&protected).unwrap();
            catalog
                .apply(&message(
                    kind,
                    0,
                    "ancestor-lookalike",
                    vec![record("sibling")],
                ))
                .unwrap();
            assert!(catalog
                .apply(&message(kind, 2, "ancestor", vec![]))
                .unwrap());
            for key in ["ancestor", "unrelated-key", "protected-descendant"] {
                assert!(catalog.folder(kind, key).unwrap().is_none(), "{key}");
            }
            assert_eq!(rows(&catalog, kind, "ancestor-lookalike")[0].key, "sibling");
        }
    }

    #[test]
    fn library_clear_keeps_subfolders_and_unpublished_root_is_not_a_listing() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            catalog
                .apply(&under(
                    message(kind, 0, "child", vec![record("keep")]),
                    kind.root_key(),
                ))
                .unwrap();
            assert!(catalog.folder(kind, kind.root_key()).unwrap().is_none());
            catalog
                .apply(&message(kind, 0, kind.root_key(), vec![record("remove")]))
                .unwrap();
            assert!(catalog
                .apply(&message(kind, 2, kind.root_key(), vec![]))
                .unwrap());
            if kind == LibraryKind::Capture {
                assert_eq!(rows(&catalog, kind, kind.root_key())[0].key, "keep");
            } else {
                assert!(rows(&catalog, kind, kind.root_key()).is_empty());
            }
            assert_eq!(rows(&catalog, kind, "child")[0].key, "keep");
            catalog.clear();
            assert!(catalog.folder(kind, "child").unwrap().is_none());
            assert!(catalog.folder(kind, kind.root_key()).unwrap().is_none());
        }
    }

    #[test]
    fn capture_root_projects_observed_folder_tree_and_child_records() {
        let kind = LibraryKind::Capture;
        let mut catalog = NativeLibraryCatalog::default();
        catalog
            .apply(&message(kind, 0, kind.root_key(), vec![record("root")]))
            .unwrap();
        catalog
            .apply(&under(
                message(kind, 0, "user-folder", vec![record("child")]),
                kind.root_key(),
            ))
            .unwrap();

        let listing = catalog.folder(kind, kind.root_key()).unwrap().unwrap();
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            ["root", "child"]
        );
        assert_eq!(
            listing.entries[1].folder_key.as_deref(),
            Some("user-folder")
        );
        assert_eq!(listing.folders.len(), 2);
        let child = listing
            .folders
            .iter()
            .find(|folder| folder.key == "user-folder")
            .unwrap();
        assert_eq!(child.parent_key.as_deref(), Some("local_nc_root"));
    }

    #[test]
    fn items_publish_the_resolved_embedded_root_even_when_creation_selected_another_parent() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            catalog
                .apply(&message(kind, 0, "container", vec![]))
                .unwrap();
            assert!(catalog.folder(kind, kind.root_key()).unwrap().is_none());
            catalog
                .apply(&under(
                    message(kind, 0, kind.root_key(), vec![record("published")]),
                    "container",
                ))
                .unwrap();
            assert_eq!(rows(&catalog, kind, kind.root_key())[0].key, "published");
            catalog
                .apply(&message(kind, 2, "container", vec![]))
                .unwrap();
            assert_eq!(rows(&catalog, kind, kind.root_key())[0].key, "published");
        }
    }

    #[test]
    fn missing_parent_fallback_and_folder_name_updates_do_not_reparent_nodes() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            catalog
                .apply(&under(
                    message(kind, 0, "child", vec![record("keep")]),
                    "late-parent",
                ))
                .unwrap();
            catalog
                .apply(&message(kind, 0, "late-parent", vec![]))
                .unwrap();
            let mut rename = under(message(kind, 1, "child", vec![]), "late-parent");
            let Some(pa::file_message::Folder::Folder(folder)) = &mut rename.folder else {
                unreachable!()
            };
            folder.name = Some(pa::folder_info::Name::Name("Renamed".into()));
            catalog.apply(&rename).unwrap();
            catalog
                .apply(&message(kind, 2, "late-parent", vec![]))
                .unwrap();
            assert_eq!(
                rows(&catalog, kind, "child")[0].folder_name.as_deref(),
                Some("Renamed")
            );
        }
    }

    #[test]
    fn parent_selection_is_breadth_first_and_item_resolution_is_depth_first() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            catalog
                .apply(&message(kind, 0, "container", vec![]))
                .unwrap();
            catalog
                .apply(&under(message(kind, 0, "shared", vec![]), "container"))
                .unwrap();
            catalog.apply(&message(kind, 0, "shared", vec![])).unwrap();
            catalog
                .apply(&under(
                    message(kind, 0, "leaf", vec![record("keep")]),
                    "shared",
                ))
                .unwrap();
            catalog
                .apply(&message(kind, 1, "shared", vec![record("deep")]))
                .unwrap();
            assert_eq!(rows(&catalog, kind, "shared")[0].key, "deep");
            catalog
                .apply(&message(kind, 2, "container", vec![]))
                .unwrap();
            // The item reached the deep first match. The leaf's parent was the
            // shallow first match, so it survives removal of the deep branch.
            assert!(rows(&catalog, kind, "shared").is_empty());
            assert_eq!(rows(&catalog, kind, "leaf")[0].key, "keep");
        }
    }

    #[test]
    fn live_lookup_cache_keeps_node_identity_until_that_subtree_is_removed() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            catalog
                .apply(&message(kind, 0, "container", vec![]))
                .unwrap();
            catalog
                .apply(&message(kind, 0, "shared", vec![record("first")]))
                .unwrap();
            catalog
                .apply(&under(message(kind, 0, "shared", vec![]), "container"))
                .unwrap();
            catalog
                .apply(&under(
                    message(kind, 1, "shared", vec![record("second")]),
                    "container",
                ))
                .unwrap();
            catalog
                .apply(&message(kind, 2, "container", vec![]))
                .unwrap();
            assert_eq!(
                rows(&catalog, kind, "shared")
                    .iter()
                    .map(|row| row.key.as_str())
                    .collect::<Vec<_>>(),
                ["first", "second"]
            );
            // Removing the cached node permits a later fresh node with its key.
            catalog.apply(&message(kind, 2, "shared", vec![])).unwrap();
            catalog
                .apply(&message(kind, 0, "shared", vec![record("fresh")]))
                .unwrap();
            assert_eq!(rows(&catalog, kind, "shared")[0].key, "fresh");
        }
    }

    #[test]
    fn library_container_delete_preserves_factory_and_readonly_records() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            let root = kind.root_key();
            let mut factory = message(kind, 0, root, vec![record("factory")]);
            let Some(pa::file_message::Folder::Folder(folder)) = &mut factory.folder else {
                unreachable!()
            };
            folder.is_factory = Some(pa::folder_info::IsFactory::IsFactory(true));
            catalog.apply(&factory).unwrap();
            let mut readonly = record("readonly");
            readonly.is_readonly = Some(pa::product_data::IsReadonly::IsReadonly(true));
            catalog
                .apply(&message(kind, 0, root, vec![readonly, record("user")]))
                .unwrap();
            assert!(catalog.apply(&message(kind, 2, root, vec![])).unwrap());
            assert_eq!(
                rows(&catalog, kind, root)
                    .iter()
                    .map(|r| r.key.as_str())
                    .collect::<Vec<_>>(),
                ["factory", "readonly"]
            );
            // Omitted protection is not inherited by a later supplied update.
            catalog
                .apply(&message(
                    kind,
                    1,
                    root,
                    vec![record("factory"), record("readonly")],
                ))
                .unwrap();
            assert!(catalog.apply(&message(kind, 2, root, vec![])).unwrap());
            assert!(rows(&catalog, kind, root).is_empty());
            assert!(!catalog.apply(&message(kind, 2, root, vec![])).unwrap());
            // Explicit item deletion has its own native action semantics.
            catalog.apply(&factory).unwrap();
            catalog
                .apply(&message(kind, 2, root, vec![record("factory")]))
                .unwrap();
            assert!(rows(&catalog, kind, root).is_empty());
        }
    }

    #[test]
    fn retained_factory_folder_type_cannot_be_removed_by_an_unflagged_delete() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            let mut create = message(kind, 0, "factory-folder", vec![]);
            let Some(pa::file_message::Folder::Folder(folder)) = &mut create.folder else {
                unreachable!()
            };
            folder.is_factory = Some(pa::folder_info::IsFactory::IsFactory(true));
            catalog.apply(&create).unwrap();
            catalog
                .apply(&message(kind, 1, "factory-folder", vec![record("a")]))
                .unwrap();
            assert!(!catalog
                .apply(&message(kind, 2, "factory-folder", vec![]))
                .unwrap());
            assert_eq!(rows(&catalog, kind, "factory-folder")[0].key, "a");
        }
    }

    #[test]
    fn native_folder_classification_uses_flags_before_the_embedded_root_pattern() {
        let kind = LibraryKind::Ir;
        for (key, protected) in [
            ("local_custom_root", true),
            ("prefix-local_custom_root-tail", true),
            ("local__root", false),
            ("local_local__root", true),
            ("local_x\n_root", false),
            ("local_x\r_root", false),
            ("other\0local_x_root", false),
            ("ordinary", false),
        ] {
            let mut catalog = NativeLibraryCatalog::default();
            catalog
                .apply(&message(kind, 0, key, vec![record("user")]))
                .unwrap();
            catalog.apply(&message(kind, 2, key, vec![])).unwrap();
            assert_eq!(
                catalog.folder(kind, key).unwrap().is_some(),
                protected,
                "{key:?}"
            );
        }
        let mut catalog = NativeLibraryCatalog::default();
        let mut download = message(kind, 0, "local_download_root", vec![record("user")]);
        let Some(pa::file_message::Folder::Folder(folder)) = &mut download.folder else {
            unreachable!()
        };
        folder.is_downloads = Some(pa::folder_info::IsDownloads::IsDownloads(true));
        catalog.apply(&download).unwrap();
        catalog
            .apply(&message(kind, 2, "local_download_root", vec![]))
            .unwrap();
        assert!(catalog
            .folder(kind, "local_download_root")
            .unwrap()
            .is_none());
    }

    #[test]
    fn updates_are_keyed_and_deletes_are_explicit_in_both_libraries() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            catalog
                .apply(&message(kind, 0, "opaque", vec![record("a"), record("b")]))
                .unwrap();
            let mut renamed = record("new-a");
            renamed.old_key = Some(pa::product_data::OldKey::OldKey("a".into()));
            catalog
                .apply(&message(kind, 1, "opaque", vec![renamed]))
                .unwrap();
            assert_eq!(
                rows(&catalog, kind, "opaque")
                    .iter()
                    .map(|v| v.key.as_str())
                    .collect::<Vec<_>>(),
                ["new-a", "b"]
            );
            assert!(catalog.folder(kind, "opaque/").unwrap().is_none());
            let keyless = pa::ProductData {
                index: Some(pa::product_data::Index::Index(0)),
                ..Default::default()
            };
            catalog
                .apply(&message(kind, 2, "opaque", vec![keyless]))
                .unwrap();
            assert_eq!(rows(&catalog, kind, "opaque").len(), 2);
            catalog
                .apply(&message(kind, 2, "opaque", vec![record("b")]))
                .unwrap();
            assert_eq!(rows(&catalog, kind, "opaque")[0].key, "new-a");
            assert_eq!(rows(&catalog, kind, "opaque").len(), 1);
        }
    }

    #[test]
    fn incoming_metadata_does_not_inherit_omitted_fields_or_invent_defaults() {
        let mut catalog = NativeLibraryCatalog::default();
        let mut original = record("a");
        original.version = Some(pa::product_data::Version::Version("2".into()));
        original.instrument = Some(pa::product_data::Instrument::Instrument(3));
        original.author = Some(pa::product_data::Author::Author("NeuralDSP".into()));
        original.description = Some(pa::product_data::Description::Description(
            "Official capture".into(),
        ));
        original.date = Some(pa::product_data::Date::Date("2026-09-30".into()));
        original.index = Some(pa::product_data::Index::Index(22));
        catalog
            .apply(&message(
                LibraryKind::Capture,
                0,
                "local_nc_root",
                vec![original, record("b")],
            ))
            .unwrap();
        let sparse = pa::ProductData {
            key: Some(pa::product_data::Key::Key("a".into())),
            ..Default::default()
        };
        catalog
            .apply(&message(
                LibraryKind::Capture,
                1,
                "local_nc_root",
                vec![sparse],
            ))
            .unwrap();
        let entries = rows(&catalog, LibraryKind::Capture, "local_nc_root/");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "");
        assert_eq!(entries[0].version, None);
        assert_eq!(entries[0].instrument, None);
        assert_eq!(entries[0].position, None);
        assert_eq!(entries[0].author, None);
        assert_eq!(entries[0].description, None);
        assert_eq!(entries[0].date, None);
        assert_eq!(entries[1].name, "Name b");
    }

    #[test]
    fn read_unknown_and_wrong_provider_frames_cannot_publish_a_library() {
        let mut catalog = NativeLibraryCatalog::default();
        for action in [3, 4, -1] {
            catalog
                .apply(&message(
                    LibraryKind::Capture,
                    action,
                    "local_nc_root",
                    vec![record("a")],
                ))
                .unwrap();
        }
        for file_type in [
            None,
            Some(pa::file_message::Type::Type(0)),
            Some(pa::file_message::Type::Type(4)),
        ] {
            let mut frame = message(LibraryKind::Capture, 0, "local_nc_root", vec![record("a")]);
            frame.r#type = file_type;
            catalog.apply(&frame).unwrap();
        }
        assert!(catalog
            .folder(LibraryKind::Capture, "local_nc_root")
            .unwrap()
            .is_none());
        catalog
            .apply(&message(
                LibraryKind::Ir,
                0,
                "local_nc_root",
                vec![record("ir")],
            ))
            .unwrap();
        assert!(catalog
            .folder(LibraryKind::Capture, "local_nc_root")
            .unwrap()
            .is_none());
        assert_eq!(
            rows(&catalog, LibraryKind::Ir, "local_nc_root")[0].key,
            "ir"
        );
    }

    #[test]
    fn folder_update_names_selected_identity_without_rekeying_products() {
        let kind = LibraryKind::Capture;
        let mut catalog = NativeLibraryCatalog::default();
        catalog
            .apply(&message(kind, 0, "source", vec![record("opaque-product")]))
            .unwrap();
        catalog
            .apply(&message(kind, 0, "target", vec![record("another")]))
            .unwrap();
        let mut frame = message(kind, 1, "source", vec![]);
        frame.to_folder = Some(pa::file_message::ToFolder::ToFolder(pa::FolderInfo {
            key: Some(pa::folder_info::Key::Key("target".into())),
            name: Some(pa::folder_info::Name::Name("Renamed".into())),
            ..Default::default()
        }));
        catalog.apply(&frame).unwrap();
        assert_eq!(rows(&catalog, kind, "source")[0].key, "opaque-product");
        assert_eq!(
            rows(&catalog, kind, "source")[0].folder_name.as_deref(),
            Some("Library")
        );
        assert_eq!(
            rows(&catalog, kind, "target")[0].folder_name.as_deref(),
            Some("Renamed")
        );
        frame.to_folder = None;
        let Some(pa::file_message::Folder::Folder(source)) = &mut frame.folder else {
            unreachable!()
        };
        source.name = Some(pa::folder_info::Name::Name("Source renamed".into()));
        catalog.apply(&frame).unwrap();
        assert_eq!(
            rows(&catalog, kind, "source")[0].folder_name.as_deref(),
            Some("Source renamed")
        );
        catalog.apply(&message(kind, 2, "source", vec![])).unwrap();
        assert!(catalog.folder(kind, "source").unwrap().is_none());
        assert_eq!(rows(&catalog, kind, "target").len(), 1);
    }

    #[test]
    fn failed_projection_is_atomic_and_cannot_expose_stale_rows_until_reset() {
        let mut catalog = NativeLibraryCatalog::default();
        let kind = LibraryKind::Capture;
        catalog
            .apply(&message(kind, 0, "folder", vec![record("a")]))
            .unwrap();
        let mut bad = record("bad");
        bad.index = Some(pa::product_data::Index::Index(-1));
        assert!(catalog
            .apply(&message(kind, 1, "folder", vec![record("b"), bad]))
            .is_err());
        let tree = &catalog.trees[&kind];
        assert_eq!(
            tree.nodes[&tree.resolve("folder").unwrap()].records.len(),
            1
        );
        assert!(catalog.folder(kind, "folder").is_err());
        assert!(catalog.folder(LibraryKind::Ir, "folder").unwrap().is_none());
        catalog.clear();
        assert!(catalog.folder(kind, "folder").unwrap().is_none());
    }

    #[test]
    fn file_updates_ignore_destination_and_empty_create_preserves_records() {
        for kind in [LibraryKind::Capture, LibraryKind::Ir] {
            let mut catalog = NativeLibraryCatalog::default();
            catalog
                .apply(&message(kind, 0, "source", vec![record("a")]))
                .unwrap();
            catalog.apply(&message(kind, 0, "source", vec![])).unwrap();
            let mut update = message(kind, 1, "source", vec![record("b")]);
            update.to_folder = Some(pa::file_message::ToFolder::ToFolder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key("destination".into())),
                files: vec![record("phantom")],
                ..Default::default()
            }));
            catalog.apply(&update).unwrap();
            assert_eq!(
                rows(&catalog, kind, "source")
                    .iter()
                    .map(|v| v.key.as_str())
                    .collect::<Vec<_>>(),
                ["a", "b"]
            );
            assert!(catalog.folder(kind, "destination").unwrap().is_none());
        }
    }

    fn records(prefix: &str, count: usize) -> Vec<pa::ProductData> {
        (0..count)
            .map(|index| record(&format!("{prefix}-{index}")))
            .collect()
    }

    fn assert_released(catalog: &NativeLibraryCatalog, kind: LibraryKind, needle: &str) {
        let error = catalog.folder(kind, kind.root_key()).unwrap_err();
        assert!(error.contains(needle), "{error}");
        assert!(!catalog.trees.contains_key(&kind));
        assert!(!catalog.staged.contains_key(&kind));
        assert!(!catalog.staged_usage.contains_key(&kind));
    }

    #[test]
    fn a_live_record_flood_is_refused_at_the_record_cap_and_released() {
        let kind = LibraryKind::Ir;
        let mut catalog = NativeLibraryCatalog::default();
        let per_folder = 1_024;
        let mut accepted = 0;
        let mut folder = 0;
        let error = loop {
            let key = format!("flood-{folder}");
            match catalog.apply(&message(kind, 0, &key, records(&key, per_folder))) {
                Ok(_) => accepted += per_folder,
                Err(error) => break error,
            }
            folder += 1;
            assert!(folder < 1_000, "the record flood was never refused");
        };
        assert!(error.contains("records"), "{error}");
        assert_eq!(accepted, MAX_LIBRARY_RECORDS);
        assert_released(&catalog, kind, "records");
        // Further device input cannot regrow the released provider.
        assert!(catalog
            .apply(&message(kind, 0, "after", records("after", 1)))
            .is_err());
        assert!(!catalog.trees.contains_key(&kind));
        // The other provider is unaffected, and a new session recovers.
        assert!(catalog
            .apply(&message(LibraryKind::Capture, 0, "ok", records("ok", 1)))
            .unwrap());
        catalog.clear();
        assert!(catalog
            .apply(&message(kind, 0, "fresh", records("fresh", 1)))
            .unwrap());
        assert_eq!(rows(&catalog, kind, "fresh").len(), 1);
    }

    #[test]
    fn a_folder_flood_is_refused_at_the_folder_cap() {
        let kind = LibraryKind::Ir;
        let mut catalog = NativeLibraryCatalog::default();
        let mut created = 0;
        let error = loop {
            match catalog.apply(&message(kind, 0, &format!("folder-{created}"), vec![])) {
                Ok(true) => created += 1,
                Ok(false) => panic!("folder {created} was not created"),
                Err(error) => break error,
            }
            assert!(
                created <= MAX_LIBRARY_FOLDERS,
                "the folder flood was never refused"
            );
        };
        assert!(error.contains("folders"), "{error}");
        let seeds = FolderTree::new(kind).nodes.len();
        assert_eq!(created, MAX_LIBRARY_FOLDERS - seeds);
        assert!(catalog.folder(kind, "folder-0").is_err());
    }

    #[test]
    fn one_deeply_segmented_key_cannot_build_an_unbounded_chain() {
        let kind = LibraryKind::Capture;
        let mut catalog = NativeLibraryCatalog::default();
        // Within the per-string bound, but with more segments than folders.
        let key = vec!["a"; MAX_LIBRARY_FOLDERS + 2_000].join("/");
        assert!(key.len() <= MAX_LIBRARY_STRING_BYTES);
        let error = catalog.apply(&message(kind, 0, &key, vec![])).unwrap_err();
        assert!(error.contains("folders"), "{error}");
        assert!(catalog
            .trees
            .get(&kind)
            .is_none_or(|tree| tree.nodes.len() <= MAX_LIBRARY_FOLDERS));
    }

    #[test]
    fn a_staged_startup_flood_is_refused_before_publication() {
        let kind = LibraryKind::Capture;
        let mut catalog = NativeLibraryCatalog::default();
        let per_folder = 4_096;
        let mut staged = 0;
        let error = loop {
            let key = format!("staged-{staged}");
            match catalog.stage_or_apply(&message(kind, 0, &key, records(&key, per_folder))) {
                Ok(true) => staged += 1,
                Ok(false) => panic!("listing {staged} was not staged"),
                Err(error) => break error,
            }
            assert!(staged < 1_000, "the staged flood was never refused");
        };
        assert!(error.contains("records"), "{error}");
        assert_eq!(staged * per_folder, MAX_LIBRARY_RECORDS);
        assert_released(&catalog, kind, "records");
        assert!(catalog.publish_staged(kind).is_err());
    }

    #[test]
    fn oversized_strings_are_refused_and_the_byte_budget_bounds_large_rows() {
        let kind = LibraryKind::Ir;
        let at_limit = "d".repeat(MAX_LIBRARY_STRING_BYTES);
        let with_description = |key: String, description: &str| pa::ProductData {
            description: Some(pa::product_data::Description::Description(
                description.into(),
            )),
            ..record(&key)
        };

        let mut catalog = NativeLibraryCatalog::default();
        assert!(catalog
            .apply(&message(
                kind,
                0,
                "f",
                vec![with_description("ok".into(), &at_limit)]
            ))
            .unwrap());
        let oversized = format!("{at_limit}d");
        let error = catalog
            .apply(&message(
                kind,
                0,
                "f",
                vec![with_description("big".into(), &oversized)],
            ))
            .unwrap_err();
        assert!(error.contains("record description"), "{error}");

        let mut catalog = NativeLibraryCatalog::default();
        let error = catalog
            .stage_or_apply(&message(kind, 0, &oversized, vec![]))
            .unwrap_err();
        assert!(error.contains("folder key"), "{error}");

        // Rows each within the per-string bound still cannot exceed the
        // provider's retained-byte budget in aggregate.
        let mut catalog = NativeLibraryCatalog::default();
        let mut folder = 0;
        let error = loop {
            let key = format!("large-{folder}");
            let files = (0..256)
                .map(|index| with_description(format!("{key}-{index}"), &at_limit))
                .collect();
            if let Err(error) = catalog.apply(&message(kind, 0, &key, files)) {
                break error;
            }
            folder += 1;
            assert!(folder < 64, "the byte flood was never refused");
        };
        assert!(error.contains("bytes"), "{error}");
        assert_released(&catalog, kind, "bytes");
    }
}
