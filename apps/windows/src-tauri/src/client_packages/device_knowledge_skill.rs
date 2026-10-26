use super::*;

pub(super) const DEVICE_KNOWLEDGE_SKILL_NAME: &str = "qc-device-knowledge";
pub(super) const DEVICE_KNOWLEDGE_SKILL_VERSION: &str = "1.0.0";
pub(super) const DEVICE_KNOWLEDGE_SKILL_MARKER: &str = ".qc-remote-managed.json";
/// Backup sub-directories, one per host that QC Remote installs the skill for.
pub(super) const CODEX_SKILL_HOST: &str = "codex";
pub(super) const ANTIGRAVITY_SKILL_HOST: &str = "antigravity";
/// Named for the identity, not the label, like LM_STUDIO_CLIENT_PREFIX: the
/// backup directory is keyed state and survives the rename to Bionic.
pub(super) const LM_STUDIO_SKILL_HOST: &str = "lm-studio";

pub(super) struct BundledSkillFile {
    pub(super) path: &'static str,
    pub(super) contents: &'static [u8],
}

macro_rules! device_knowledge_skill_file {
    ($path:literal) => {
        BundledSkillFile {
            path: $path,
            contents: include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../skills/qc-device-knowledge/",
                $path
            )),
        }
    };
}

pub(super) const DEVICE_KNOWLEDGE_SKILL_FILES: &[BundledSkillFile] = &[
    device_knowledge_skill_file!("SKILL.md"),
    device_knowledge_skill_file!("references/data/qc-model-lineage.json"),
    // SKILL.md and the lineage catalogue are Schedule A files of the public
    // LICENSE, covered by the reference-material terms. Every copy of the skill
    // carries LICENSE and, transitively, every document a bundled file links
    // to, at the relative path the link names, so each link resolves inside the
    // skill. All are byte copies of the repository's own that
    // scripts/verify-license-carveout.mjs keeps identical, and it also checks
    // that every relative link in every file listed here names a listed file.
    device_knowledge_skill_file!("LICENSE"),
    device_knowledge_skill_file!("LICENSES/LicenseRef-QC-Remote-Reference.txt"),
    device_knowledge_skill_file!("LICENSE-MIT"),
    device_knowledge_skill_file!("LICENSE-APACHE"),
    device_knowledge_skill_file!("legal/LICENSE-MIT"),
    device_knowledge_skill_file!("legal/LICENSE-APACHE"),
    device_knowledge_skill_file!("CONTRIBUTING.md"),
    device_knowledge_skill_file!("TRADEMARKS.md"),
    device_knowledge_skill_file!("THIRD_PARTY-NOTICES.md"),
    device_knowledge_skill_file!("SECURITY.md"),
    device_knowledge_skill_file!("legal/THIRD_PARTY-LICENSE-TEXTS.txt"),
    device_knowledge_skill_file!("legal/THIRD_PARTY-LICENSE-INVENTORY.json"),
    device_knowledge_skill_file!("legal/THIRD_PARTY-SOURCE-OFFER.md"),
];

/// The installed bridge as a manual configuration has to spell it out, with the
/// user's own profile left as a placeholder.
fn installed_bridge_path() -> String {
    #[cfg(target_os = "macos")]
    {
        format!("~/Library/Application Support/QC Remote/integrations/{BRIDGE_FILE_NAME}")
    }
    #[cfg(not(target_os = "macos"))]
    {
        format!(r"C:\Users\<you>\AppData\Local\{BRIDGE_DIRECTORY}\{BRIDGE_FILE_NAME}")
    }
}

/// The same path escaped for a TOML or JSON string.
pub(super) fn escaped_bridge_path() -> String {
    installed_bridge_path().replace('\\', r"\\")
}

fn sha256_hex(contents: &[u8]) -> String {
    digest_hex(Sha256::digest(contents))
}

fn digest_hex(digest: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = digest.as_ref();
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

// Software inspection budgets, not firmware or user-library format maxima.
// Exhaustion refuses replacement/restore before retiring the original tree.
const SKILL_DOCUMENT_LIMIT: u64 = 8 * 1024 * 1024;
const SKILL_FILE_LIMIT: u64 = 64 * 1024 * 1024;
const SKILL_TREE_BYTE_LIMIT: u64 = 256 * 1024 * 1024;
const SKILL_TREE_ENTRY_LIMIT: usize = 16_384;
const SKILL_TREE_PATH_LIMIT: usize = 4 * 1024 * 1024;
const SKILL_TREE_DEPTH_LIMIT: usize = 64;

/// One unpair inspection, including invalid newer backup candidates. These are
/// software headroom ceilings, not firmware limits. Exhaustion is unknown and
/// refuses BEFORE the live managed tree or any historical backup is changed.
struct SkillBackupInspection {
    bytes_remaining: u64,
    entries_remaining: usize,
    paths_remaining: usize,
    exhausted: bool,
}
impl Default for SkillBackupInspection {
    fn default() -> Self {
        Self {
            bytes_remaining: SKILL_TREE_BYTE_LIMIT,
            entries_remaining: SKILL_TREE_ENTRY_LIMIT,
            paths_remaining: SKILL_TREE_PATH_LIMIT,
            exhausted: false,
        }
    }
}
impl SkillBackupInspection {
    fn exhausted(&mut self) -> String {
        self.exhausted = true;
        "The skill backup inspection exceeds its software byte/entry/path/depth budget; the current skill and all backups were left untouched.".into()
    }
    fn entry(&mut self, path_bytes: usize) -> Result<(), String> {
        let Some(entries) = self.entries_remaining.checked_sub(1) else {
            return Err(self.exhausted());
        };
        let Some(paths) = self.paths_remaining.checked_sub(path_bytes) else {
            return Err(self.exhausted());
        };
        self.entries_remaining = entries;
        self.paths_remaining = paths;
        Ok(())
    }
    fn read(
        &mut self,
        reader: &mut impl std::io::Read,
        output: &mut [u8],
    ) -> Result<usize, String> {
        // One witness byte detects growth even with exactly zero budget left.
        let available = self
            .bytes_remaining
            .saturating_add(1)
            .min(output.len() as u64) as usize;
        let count = reader
            .read(&mut output[..available])
            .map_err(|error| error.to_string())?;
        if count as u64 > self.bytes_remaining {
            self.bytes_remaining = 0;
            return Err(self.exhausted());
        }
        self.bytes_remaining -= count as u64;
        Ok(count)
    }
}

fn bounded_skill_document(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut contents = Vec::new();
    file.take(SKILL_DOCUMENT_LIMIT + 1)
        .read_to_end(&mut contents)
        .map_err(|error| error.to_string())?;
    if contents.len() as u64 > SKILL_DOCUMENT_LIMIT {
        return Err("The skill metadata exceeds the software inspection budget.".into());
    }
    Ok(contents)
}

fn streamed_skill_hash(path: &Path, remaining: u64) -> Result<(String, u64), String> {
    streamed_skill_hash_with_inspection(path, remaining, None)
}

fn streamed_skill_hash_with_inspection(
    path: &Path,
    remaining: u64,
    mut inspection: Option<&mut SkillBackupInspection>,
) -> Result<(String, u64), String> {
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let limit = remaining.min(SKILL_FILE_LIMIT);
    if file.metadata().map_err(|error| error.to_string())?.len() > limit {
        if let Some(inspection) = inspection.as_deref_mut() {
            return Err(inspection.exhausted());
        }
        return Err("The skill file exceeds the software inspection budget.".into());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    let mut bytes = 0u64;
    loop {
        let available = (limit - bytes + 1).min(buffer.len() as u64) as usize;
        let read = match inspection.as_deref_mut() {
            Some(inspection) => inspection.read(&mut file, &mut buffer[..available])?,
            None => file
                .read(&mut buffer[..available])
                .map_err(|error| error.to_string())?,
        };
        if read == 0 {
            break;
        }
        if read as u64 > limit - bytes {
            if let Some(inspection) = inspection.as_deref_mut() {
                return Err(inspection.exhausted());
            }
            return Err("The skill file grew beyond the software inspection budget.".into());
        }
        bytes += read as u64;
        hash.update(&buffer[..read]);
    }
    Ok((digest_hex(hash.finalize()), bytes))
}

fn bundled_device_knowledge_skill_hashes() -> serde_json::Map<String, Value> {
    DEVICE_KNOWLEDGE_SKILL_FILES
        .iter()
        .map(|file| {
            (
                file.path.to_string(),
                Value::String(sha256_hex(file.contents)),
            )
        })
        .collect()
}

pub(super) fn bundled_device_knowledge_skill_marker() -> Value {
    json!({
        "schemaVersion": 1,
        "name": DEVICE_KNOWLEDGE_SKILL_NAME,
        "skillVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
        "appVersion": env!("CARGO_PKG_VERSION"),
        "files": bundled_device_knowledge_skill_hashes(),
    })
}

pub(super) fn safe_skill_relative_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Every entry under `source` that `copy_directory` would not carry across.
///
/// Anything that is neither a directory nor a regular file - a symlink, a
/// device node - is skipped by the copy rather than refused. Copying through a
/// link would pull in whatever it points at, and refusing outright used to fail
/// the whole pair over one link in the user's own skill folder.
///
/// A caller that copies and then *deletes* the source needs to know before it
/// starts, not after: the skipped entry is not left behind, it is destroyed,
/// and the backup the user would reach for never had it. Asking first means
/// such a caller can refuse without having written a backup it then has to
/// take back - an abandoned backup is the newest one in the root, and
/// `latest_device_knowledge_skill_backup` restores the newest.
pub(super) fn unbackupable_entries(source: &Path) -> Result<Vec<PathBuf>, String> {
    unbackupable_entries_with(source, 0, &mut 0, &mut 0)
}

fn unbackupable_entries_with(
    source: &Path,
    depth: usize,
    entries: &mut usize,
    paths: &mut usize,
) -> Result<Vec<PathBuf>, String> {
    if depth > SKILL_TREE_DEPTH_LIMIT {
        return Err("The skill directory exceeds the software depth budget.".into());
    }
    let mut skipped = Vec::new();
    for entry in fs::read_dir(source)
        .map_err(|error| format!("Could not read {}: {error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("Could not read a skill entry: {error}"))?;
        *entries = entries
            .checked_add(1)
            .ok_or("Skill entry accounting overflow")?;
        *paths = paths
            .checked_add(entry.path().as_os_str().len())
            .ok_or("Skill path accounting overflow")?;
        if *entries > SKILL_TREE_ENTRY_LIMIT || *paths > SKILL_TREE_PATH_LIMIT {
            return Err("The skill inventory exceeds the software entry/path budget.".into());
        }
        let file_type = entry
            .file_type()
            .map_err(|error| format!("Could not inspect {}: {error}", entry.path().display()))?;
        if file_type.is_dir() {
            skipped.extend(unbackupable_entries_with(
                &entry.path(),
                depth + 1,
                entries,
                paths,
            )?);
        } else if !file_type.is_file() {
            skipped.push(entry.path());
        }
    }
    Ok(skipped)
}

/// The refusal a caller owes its user when a backup cannot carry everything it
/// is about to delete, or `None` when there is nothing to refuse over.
///
/// Shared by the two places that ask - the scan before the copy, and the copy's
/// own report - so both phrase it identically and neither can be tightened
/// without the other. Names the first entry and a count rather than joining
/// them all: this lands in a row detail that ellipses, and an unbounded list
/// clips away whatever follows it.
///
/// Says what to move, not what to press. Every error out of this module is a
/// cause and its context; the panel appends the action, because it is the side
/// that knows whether the button in front of the user says Pair or Update. This
/// message used to end "and pair again", which read as a contradiction once the
/// panel added its own prompt after it.
fn refuse_over_unbackupable(unbacked: &[PathBuf], skill_directory: &Path) -> Option<String> {
    let first = unbacked.first()?;
    let others = match unbacked.len() - 1 {
        0 => String::new(),
        1 => " and 1 other".to_string(),
        more => format!(" and {more} others"),
    };
    Some(format!(
        "Pairing would delete {}{} from {}, and a link or special file cannot be backed up first. Move it aside first.",
        first.display(),
        others,
        skill_directory.display()
    ))
}

/// Copy a directory, returning the entries it could not carry across.
///
/// The return is deliberately not `()`. This skips silently by design (see
/// `unbackupable_entries`), and a caller that deletes the source afterwards
/// turns that skip into permanent loss - so the skipped list is handed back
/// rather than dropped inside, and a caller that genuinely does not care has
/// to say so at its own call site instead of inheriting the decision from a
/// convenience wrapper it never read.
#[must_use = "a caller that deletes the source must check what was not copied"]
#[cfg(test)]
pub(super) fn copy_directory(source: &Path, destination: &Path) -> Result<Vec<PathBuf>, String> {
    copy_skill_directory_with(source, destination, &mut |_, _| Ok(()))
}

fn copy_skill_directory_with(
    source: &Path,
    destination: &Path,
    fault: &mut impl FnMut(&str, &Path) -> Result<(), String>,
) -> Result<Vec<PathBuf>, String> {
    use std::io::Read;
    #[derive(Default)]
    struct CopyBudget {
        entries: usize,
        paths: usize,
        bytes: u64,
    }
    fn walk(
        root: &Path,
        source: &Path,
        destination: &Path,
        depth: usize,
        budget: &mut CopyBudget,
        fault: &mut impl FnMut(&str, &Path) -> Result<(), String>,
    ) -> Result<Vec<PathBuf>, String> {
        if depth > SKILL_TREE_DEPTH_LIMIT {
            return Err("The skill copy exceeds the software depth budget.".into());
        }
        fault("copy-directory", source)?;
        let mut skipped = Vec::new();
        fs::create_dir_all(destination)
            .map_err(|error| format!("Could not create {}: {error}", destination.display()))?;
        for entry in fs::read_dir(source)
            .map_err(|error| format!("Could not read {}: {error}", source.display()))?
        {
            let entry = entry.map_err(|error| format!("Could not read a skill entry: {error}"))?;
            let source_path = entry.path();
            let relative = source_path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?;
            budget.entries = budget
                .entries
                .checked_add(1)
                .ok_or("Skill copy entry accounting overflow")?;
            budget.paths = budget
                .paths
                .checked_add(relative.to_string_lossy().len())
                .ok_or("Skill copy path accounting overflow")?;
            if budget.entries > SKILL_TREE_ENTRY_LIMIT || budget.paths > SKILL_TREE_PATH_LIMIT {
                return Err("The skill copy exceeds the software entry/path budget.".into());
            }
            let target = destination.join(entry.file_name());
            let file_type = entry
                .file_type()
                .map_err(|error| format!("Could not inspect {}: {error}", source_path.display()))?;
            if file_type.is_dir() {
                skipped.extend(walk(root, &source_path, &target, depth + 1, budget, fault)?);
            } else if file_type.is_file() {
                fault("backup-file", &target)?;
                let mut original = fs::File::open(&source_path).map_err(|error| {
                    format!("Could not read {}: {error}", source_path.display())
                })?;
                let metadata = original.metadata().map_err(|error| error.to_string())?;
                let limit =
                    SKILL_FILE_LIMIT.min(SKILL_TREE_BYTE_LIMIT.saturating_sub(budget.bytes));
                if !metadata.is_file() || metadata.len() > limit {
                    return Err(
                        "The skill copy exceeds the software file/total byte budget.".into(),
                    );
                }
                // The enclosing transaction owns an exclusive fresh destination
                // tree. A collision is refused, never overwritten.
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .map_err(|error| {
                        format!(
                            "Could not create skill backup {}: {error}",
                            target.display()
                        )
                    })?;
                let mut used = 0u64;
                let mut buffer = [0u8; 16 * 1024];
                loop {
                    // Read at most the remaining envelope plus one witness byte.
                    let available = (limit - used + 1).min(buffer.len() as u64) as usize;
                    let count = original.read(&mut buffer[..available]).map_err(|error| {
                        format!("Could not read {}: {error}", source_path.display())
                    })?;
                    if count == 0 {
                        break;
                    }
                    if count as u64 > limit - used {
                        return Err("The skill grew beyond the software copy byte budget.".into());
                    }
                    output.write_all(&buffer[..count]).map_err(|error| {
                        format!("Could not back up {}: {error}", source_path.display())
                    })?;
                    used += count as u64;
                }
                output
                    .set_permissions(metadata.permissions())
                    .map_err(|error| {
                        format!("Could not preserve skill file permissions: {error}")
                    })?;
                budget.bytes += used;
            } else {
                skipped.push(source_path);
            }
        }
        Ok(skipped)
    }
    walk(
        source,
        source,
        destination,
        0,
        &mut CopyBudget::default(),
        fault,
    )
}

pub(super) fn device_knowledge_skill_status_at(skill_directory: &Path) -> Value {
    if !skill_directory.exists() {
        return json!({
            "installed": false,
            "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
            "state": "not-installed",
        });
    }
    if !skill_directory.is_dir() {
        return json!({
            "installed": false,
            "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
            "state": "different-configuration",
            "statusError": "The qc-device-knowledge skill path is not a directory.",
        });
    }
    let marker_path = skill_directory.join(DEVICE_KNOWLEDGE_SKILL_MARKER);
    let marker = match bounded_skill_document(&marker_path)
        .map_err(|error| format!("Could not read the managed skill marker: {error}"))
        .and_then(|contents| {
            serde_json::from_slice::<Value>(&contents)
                .map_err(|error| format!("The managed skill marker is invalid: {error}"))
        }) {
        Ok(marker) => marker,
        Err(error) => {
            return json!({
                "installed": true,
                "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
                "state": "different-configuration",
                "statusError": error,
            });
        }
    };
    if marker.get("name").and_then(Value::as_str) != Some(DEVICE_KNOWLEDGE_SKILL_NAME) {
        return json!({
            "installed": true,
            "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
            "state": "different-configuration",
            "statusError": "The managed skill marker names a different skill.",
        });
    }
    let installed_version = marker
        .get("skillVersion")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let Some(installed_hashes) = marker.get("files").and_then(Value::as_object) else {
        return json!({
            "installed": true,
            "installedVersion": installed_version,
            "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
            "state": "different-configuration",
            "statusError": "The managed skill marker has no file inventory.",
        });
    };
    if installed_hashes.is_empty()
        || marker
            .get("schemaVersion")
            .is_some_and(|version| version.as_u64() != Some(1))
    {
        return json!({
            "installed": true, "installedVersion": installed_version,
            "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
            "state": "different-configuration",
            "statusError": "The managed skill marker has an empty inventory or an unsupported schema.",
        });
    }
    if installed_hashes.len() > SKILL_TREE_ENTRY_LIMIT
        || installed_hashes
            .keys()
            .try_fold(0usize, |sum, path| sum.checked_add(path.len()))
            .is_none_or(|bytes| bytes > SKILL_TREE_PATH_LIMIT)
    {
        return json!({"installed": true, "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
            "state": "different-configuration", "statusError": "The skill marker exceeds the software inventory budget."});
    }
    let mut inspected_bytes = 0u64;
    for (relative, expected_hash) in installed_hashes {
        if !safe_skill_relative_path(relative) {
            return json!({
                "installed": true,
                "installedVersion": installed_version,
                "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
                "state": "different-configuration",
                "statusError": "The managed skill marker contains an unsafe path.",
            });
        }
        let Some(expected_hash) = expected_hash.as_str() else {
            return json!({
                "installed": true,
                "installedVersion": installed_version,
                "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
                "state": "different-configuration",
                "statusError": "The managed skill marker contains an invalid hash.",
            });
        };
        let actual = streamed_skill_hash(
            &skill_directory.join(relative),
            SKILL_TREE_BYTE_LIMIT.saturating_sub(inspected_bytes),
        )
        .ok()
        .map(|(hash, bytes)| {
            inspected_bytes += bytes;
            hash
        });
        if actual.as_deref() != Some(expected_hash) {
            return json!({
                "installed": true,
                "installedVersion": installed_version,
                "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
                "state": "modified",
                "statusError": format!("The installed skill file {relative} was modified or removed."),
            });
        }
    }
    let actual = match skill_tree_inventory(skill_directory) {
        Ok(inventory) => inventory,
        Err(error) => {
            return json!({
                "installed": true, "installedVersion": installed_version,
                "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
                "state": "modified", "statusError": error,
            });
        }
    };
    if !managed_skill_inventory_matches(&actual, &marker) {
        return json!({
            "installed": true, "installedVersion": installed_version,
            "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
            "state": "modified",
            "statusError": "The installed skill contains additional user files or directories; unpair will leave it untouched.",
        });
    }
    let available_hashes = bundled_device_knowledge_skill_hashes();
    let state = match installed_version.as_deref() {
        Some(version)
            if version == DEVICE_KNOWLEDGE_SKILL_VERSION
                && installed_hashes == &available_hashes =>
        {
            "current"
        }
        Some(version) => match compare_versions(DEVICE_KNOWLEDGE_SKILL_VERSION, version) {
            Some(Ordering::Less) => "newer-installed",
            _ => "update-available",
        },
        None => "update-available",
    };
    json!({
        "installed": true,
        "installedVersion": installed_version,
        "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
        "state": state,
    })
}

fn managed_skill_inventory_matches(inventory: &Value, marker: &Value) -> bool {
    if marker["name"].as_str() != Some(DEVICE_KNOWLEDGE_SKILL_NAME)
        || marker
            .get("schemaVersion")
            .is_some_and(|version| version.as_u64() != Some(1))
    {
        return false;
    }
    let Some(files) = marker["files"].as_object() else {
        return false;
    };
    if files.is_empty()
        || files.len() > SKILL_TREE_ENTRY_LIMIT
        || files.keys().any(|path| !safe_skill_relative_path(path))
    {
        return false;
    }
    let mut actual = inventory["files"].as_object().cloned().unwrap_or_default();
    actual.remove(DEVICE_KNOWLEDGE_SKILL_MARKER);
    let mut directories = std::collections::BTreeSet::new();
    for file in files.keys() {
        let mut path = file.as_str();
        while let Some((parent, _)) = path.rsplit_once('/') {
            directories.insert(parent.to_owned());
            path = parent;
        }
    }
    actual == *files && inventory["directories"] == json!(directories)
}

/// The outcome of a skill install.  Pairing writes the MCP configuration first
/// and installs the skill second, so a skill failure is a partial success that
/// the caller has to describe rather than collapse into a bare `Err`.  The
/// backup path is reported even when the install then failed: the user's own
/// copy has already been taken aside and they need to be told where it went.
#[derive(Debug, Default)]
pub(super) struct DeviceKnowledgeSkillInstall {
    pub(super) installed: bool,
    pub(super) backup: Option<PathBuf>,
    pub(super) error: Option<String>,
}

impl DeviceKnowledgeSkillInstall {
    pub(super) fn report(self) -> Value {
        json!({
            "skillInstalled": self.installed,
            "skillVersion": self.installed.then(|| DEVICE_KNOWLEDGE_SKILL_VERSION.to_string()),
            "skillBackupPath": self.backup.map(|path| path.to_string_lossy().into_owned()),
            "skillError": self.error,
        })
    }
}

pub(super) fn install_device_knowledge_skill_at(
    skill_directory: &Path,
    backup_root: &Path,
) -> DeviceKnowledgeSkillInstall {
    let mut backup = None;
    match stage_device_knowledge_skill_at(skill_directory, backup_root, &mut backup) {
        Ok(()) => DeviceKnowledgeSkillInstall {
            installed: true,
            backup,
            error: None,
        },
        Err(error) => DeviceKnowledgeSkillInstall {
            installed: false,
            backup,
            error: Some(error),
        },
    }
}

fn stage_device_knowledge_skill_at(
    skill_directory: &Path,
    backup_root: &Path,
    taken_backup: &mut Option<PathBuf>,
) -> Result<(), String> {
    stage_device_knowledge_skill_with(skill_directory, backup_root, taken_backup, &mut |_, _| {
        Ok(())
    })
}

/// Only paths this invocation atomically created are cleanup-owned. In
/// particular, a collision must never delete another install's staging bytes.
struct OwnedSkillPath {
    path: Option<PathBuf>,
    directory: bool,
}

impl OwnedSkillPath {
    fn directory(path: PathBuf) -> Result<Self, String> {
        fs::create_dir(&path)
            .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
        Ok(Self {
            path: Some(path),
            directory: true,
        })
    }
    fn file(path: PathBuf, contents: &[u8]) -> Result<Self, String> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
        let mut owned = Self {
            path: Some(path),
            directory: false,
        };
        let written = file.write_all(contents);
        drop(file);
        if let Err(error) = written {
            let cleanup = owned
                .cleanup()
                .err()
                .map(|reason| format!(". {reason}"))
                .unwrap_or_default();
            return Err(format!(
                "Could not write the backup receipt: {error}{cleanup}"
            ));
        }
        Ok(owned)
    }
    fn keep(&mut self) {
        self.path = None;
    }
    fn cleanup(&mut self) -> Result<(), String> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let result = if self.directory {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match result {
            Ok(()) => {
                self.path = None;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.path = None;
                Ok(())
            }
            Err(error) => Err(format!(
                "Could not clean up owned skill staging {}: {error}",
                path.display()
            )),
        }
    }
}

impl Drop for OwnedSkillPath {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("QC Remote skill cleanup: {error}");
        }
    }
}

fn skill_backup_receipt_path(backup: &Path) -> PathBuf {
    backup.with_file_name(format!(
        "{}.complete.json",
        backup.file_name().unwrap_or_default().to_string_lossy()
    ))
}

/// Include empty directories and exact file bytes; neither skipped links nor a
/// partial copy can be approved as the user's restorable original.
fn skill_tree_inventory(root: &Path) -> Result<Value, String> {
    skill_tree_inventory_with_inspection(root, None)
}

fn skill_tree_inventory_with_inspection(
    root: &Path,
    mut inspection: Option<&mut SkillBackupInspection>,
) -> Result<Value, String> {
    #[derive(Default)]
    struct Budget {
        entries: usize,
        paths: usize,
        bytes: u64,
    }
    fn walk(
        root: &Path,
        path: &Path,
        files: &mut BTreeMap<String, String>,
        dirs: &mut Vec<String>,
        budget: &mut Budget,
        depth: usize,
        inspection: &mut Option<&mut SkillBackupInspection>,
    ) -> Result<(), String> {
        if depth > SKILL_TREE_DEPTH_LIMIT {
            if let Some(inspection) = inspection.as_deref_mut() {
                return Err(inspection.exhausted());
            }
            return Err("The skill directory exceeds the software depth budget.".into());
        }
        for entry in fs::read_dir(path)
            .map_err(|error| format!("Could not inventory {}: {error}", path.display()))?
        {
            let entry =
                entry.map_err(|error| format!("Could not inventory a skill entry: {error}"))?;
            let file_type = entry
                .file_type()
                .map_err(|error| format!("Could not inspect a skill entry: {error}"))?;
            let entry_path = entry.path();
            let relative = entry_path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(inspection) = inspection.as_deref_mut() {
                inspection.entry(relative.len())?;
            }
            budget.entries = budget
                .entries
                .checked_add(1)
                .ok_or("Skill entry accounting overflow")?;
            budget.paths = budget
                .paths
                .checked_add(relative.len())
                .ok_or("Skill path accounting overflow")?;
            if budget.entries > SKILL_TREE_ENTRY_LIMIT || budget.paths > SKILL_TREE_PATH_LIMIT {
                return Err("The skill inventory exceeds the software entry/path budget.".into());
            }
            if file_type.is_dir() {
                dirs.push(relative);
                walk(
                    root,
                    &entry_path,
                    files,
                    dirs,
                    budget,
                    depth + 1,
                    inspection,
                )?;
            } else if file_type.is_file() {
                let (hash, bytes) = streamed_skill_hash_with_inspection(
                    &entry_path,
                    SKILL_TREE_BYTE_LIMIT.saturating_sub(budget.bytes),
                    inspection.as_deref_mut(),
                )
                .map_err(|error| {
                    format!("Could not inventory {}: {error}", entry_path.display())
                })?;
                budget.bytes += bytes;
                files.insert(relative, hash);
            } else {
                return Err(format!(
                    "The skill contains a link or special file: {}",
                    entry_path.display()
                ));
            }
        }
        Ok(())
    }
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("Could not inspect {}: {error}", root.display()))?;
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "The skill source is not a regular directory: {}",
            root.display()
        ));
    }
    let mut files = BTreeMap::new();
    let mut dirs = Vec::new();
    walk(
        root,
        root,
        &mut files,
        &mut dirs,
        &mut Budget::default(),
        0,
        &mut inspection,
    )?;
    dirs.sort();
    Ok(json!({"schemaVersion": 1, "files": files, "directories": dirs}))
}

fn verified_skill_backup(backup: &Path) -> bool {
    bounded_skill_document(&skill_backup_receipt_path(backup))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|expected| skill_tree_inventory(backup).ok().as_ref() == Some(&expected))
}

fn skill_install_suffix() -> Result<String, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    Ok(format!(
        "{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "The system clock is before the Unix epoch.".to_string())?
            .as_millis(),
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

fn stage_device_knowledge_skill_with(
    skill_directory: &Path,
    backup_root: &Path,
    taken_backup: &mut Option<PathBuf>,
    fault: &mut impl FnMut(&str, &Path) -> Result<(), String>,
) -> Result<(), String> {
    if skill_directory.exists() && !skill_directory.is_dir() {
        return Err(format!(
            "The skill destination is not a directory: {}",
            skill_directory.display()
        ));
    }
    let parent = skill_directory
        .parent()
        .ok_or_else(|| "The skill destination has no parent directory.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the AI app's skills directory: {error}"))?;
    let suffix = skill_install_suffix()?;
    let staging = parent.join(format!(
        ".{DEVICE_KNOWLEDGE_SKILL_NAME}.qc-remote-{suffix}.tmp"
    ));
    let mut staged = OwnedSkillPath::directory(staging.clone())?;
    let mut backup_owned: Option<OwnedSkillPath> = None;
    let mut receipt_owned: Option<OwnedSkillPath> = None;
    let mut original: Option<PathBuf> = None;
    let mut activated = false;
    let result = (|| -> Result<(), String> {
        for file in DEVICE_KNOWLEDGE_SKILL_FILES {
            debug_assert!(safe_skill_relative_path(file.path));
            let destination = staging.join(file.path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!("Could not create the staged skill directory: {error}")
                })?;
            }
            fault("stage-file", &destination)?;
            fs::write(&destination, file.contents)
                .map_err(|error| format!("Could not stage {}: {error}", file.path))?;
        }
        fault("stage-marker", &staging)?;
        fs::write(
            staging.join(DEVICE_KNOWLEDGE_SKILL_MARKER),
            serde_json::to_vec_pretty(&bundled_device_knowledge_skill_marker())
                .map_err(|error| format!("Could not encode the managed skill marker: {error}"))?,
        )
        .map_err(|error| format!("Could not stage the managed skill marker: {error}"))?;

        if skill_directory.exists() {
            fs::create_dir_all(backup_root)
                .map_err(|error| format!("Could not create the skill backup directory: {error}"))?;
            let backup = backup_root.join(format!("{DEVICE_KNOWLEDGE_SKILL_NAME}-{suffix}"));
            // Asked before the backup is written, not after. The copy skips links
            // rather than refusing, which is right for copying - but the delete
            // below turns a skipped entry into permanent loss, so this refusal is
            // about the delete, not the copy, and only fires when replacing a skill
            // folder that actually contains one. Refusing costs that user a pair
            // until they move the link aside; proceeding costs them the link,
            // silently and permanently.
            //
            // Checking first also leaves nothing behind. Writing the backup and
            // then refusing left an abandoned copy that was the newest in the
            // backup root, and `latest_device_knowledge_skill_backup` takes the
            // newest - so a later unpair could restore a backup from a pair that
            // never happened, in place of the one taken when the user's own files
            // were last replaced.
            if let Some(refusal) =
                refuse_over_unbackupable(&unbackupable_entries(skill_directory)?, skill_directory)
            {
                return Err(refusal);
            }
            // Checked again, against what the copy actually saw, and in every
            // build. The scan and the copy are two separate walks of the same
            // tree, so a link created between them is invisible to the first and
            // seen by the second - and this was a `debug_assert!`, which is absent
            // from the release build that ships, leaving the delete below free to
            // destroy exactly what the refusal exists to protect. The neighbouring
            // `debug_assert!` on `safe_skill_relative_path` is the contrast: that
            // checks compile-time constants, where a debug assertion is the right
            // tool, and this checks the filesystem, where it is not.
            //
            // Recoverable here, because the source is still on disk: drop the
            // backup that was just written so no abandoned copy is left to be
            // restored later, and refuse the same way the scan does.
            backup_owned = Some(OwnedSkillPath::directory(backup.clone())?);
            fault("backup-copy", &backup)?;
            let before = skill_tree_inventory(skill_directory)?;
            let unbacked = copy_skill_directory_with(skill_directory, &backup, fault)?;
            if let Some(refusal) = refuse_over_unbackupable(&unbacked, skill_directory) {
                // Reported, not discarded. This removal is the whole reason the
                // refusal can be made here at all: an abandoned backup is the
                // newest in the root and `latest_device_knowledge_skill_backup`
                // restores the newest, so a removal that quietly failed would hand
                // a later unpair a backup from a pair that never happened - the
                // exact hazard the pre-copy scan exists to prevent.
                return Err(refusal);
            }
            fault("backup-verify", &backup)?;
            if skill_tree_inventory(&backup)? != before
                || skill_tree_inventory(skill_directory)? != before
            {
                return Err(
                    "The skill changed while it was being backed up; nothing was replaced."
                        .to_string(),
                );
            }
            let receipt_pending = backup_root.join(format!(
                ".{DEVICE_KNOWLEDGE_SKILL_NAME}-{suffix}.receipt.tmp"
            ));
            receipt_owned = Some(OwnedSkillPath::file(
                receipt_pending,
                &serde_json::to_vec(&before).map_err(|error| error.to_string())?,
            )?);
            // The exact original remains intact in its own parent until activation
            // and receipt publication succeed. Never copy-delete a user's tree.
            let retired = parent.join(format!(
                ".{DEVICE_KNOWLEDGE_SKILL_NAME}-{suffix}.original.tmp"
            ));
            fault("retire-original", skill_directory)?;
            if retired.exists() {
                return Err("The original skill staging path is already occupied.".to_string());
            }
            fs::rename(skill_directory, &retired)
                .map_err(|error| format!("Could not preserve the original skill: {error}"))?;
            original = Some(retired.clone());
            *taken_backup = Some(backup.clone());
            // Close the copy/retire gap: the retained original is still recoverable
            // if its bytes changed just before the atomic rename.
            if skill_tree_inventory(&retired)? != before {
                return Err(
                    "The skill changed before replacement; restoring its original directory."
                        .to_string(),
                );
            }
        }
        fault("activate", skill_directory)?;
        fs::rename(&staging, skill_directory)
            .map_err(|error| format!("Could not activate the updated skill: {error}"))?;
        staged.keep();
        activated = true;
        if let Some(backup) = taken_backup.as_deref() {
            fault("publish-receipt", backup)?;
            let receipt = receipt_owned
                .as_mut()
                .expect("verified backup has a receipt");
            let contents = fs::read(receipt.path.as_ref().expect("owned pending receipt"))
                .map_err(|error| format!("Could not read the staged backup receipt: {error}"))?;
            // create_new, not replacing rename: an unrelated receipt collision
            // must never be overwritten on Unix or any other filesystem.
            let mut approved = OwnedSkillPath::file(skill_backup_receipt_path(backup), &contents)?;
            receipt.cleanup()?;
            approved.keep();
            backup_owned
                .as_mut()
                .expect("verified backup is owned")
                .keep();
        }
        Ok(())
    })();
    if let Err(error) = result {
        // The original is already gone by here, so a rollback that also fails
        // has to say so and name where the bytes still are. Reporting only the
        // rename error would leave the user with an empty skill folder, a
        // "could not activate" message, and no idea a backup exists.
        let mut detail = error;
        if let Some(retired) = original.as_deref() {
            let restored = (|| -> Result<(), String> {
                fault("rollback", retired)?;
                if activated {
                    fs::remove_dir_all(skill_directory).map_err(|error| error.to_string())?;
                }
                fs::rename(retired, skill_directory).map_err(|error| error.to_string())
            })();
            match restored {
                Ok(()) => {
                    *taken_backup = None;
                }
                Err(restore) => {
                    // These are declared recovery outputs, never temp garbage:
                    // keep both exact original and verified backup for recovery.
                    if let Some(backup) = backup_owned.as_mut() {
                        backup.keep();
                    }
                    detail = format!(
                        "{detail}. Restoring the previous skill also failed: {restore}. Its original directory is still at {}{}.",
                        retired.display(),
                        taken_backup
                            .as_ref()
                            .map(|path| format!("; its verified backup is at {}", path.display()))
                            .unwrap_or_default()
                    );
                }
            }
        }
        if let Err(error) = staged.cleanup() {
            detail.push_str(&format!(". {error}"));
        }
        for cleanup in [receipt_owned.as_mut(), backup_owned.as_mut()]
            .into_iter()
            .flatten()
        {
            if let Err(error) = cleanup.cleanup() {
                detail.push_str(&format!(". {error}"));
            }
        }
        return Err(detail);
    }
    if let Some(retired) = original {
        fs::remove_dir_all(&retired).map_err(|error| format!("The skill was installed, but the retained original could not be removed at {}: {error}", retired.display()))?;
    }
    Ok(())
}

/// Only a skill whose managed marker is present and whose every file still
/// hashes to what that marker recorded is QC Remote's to delete.  "modified"
/// and "different-configuration" both mean the bytes on disk are the user's,
/// so unpairing leaves them exactly where they are.
pub(super) fn device_knowledge_skill_is_managed(state: &str) -> bool {
    matches!(state, "current" | "update-available" | "newer-installed")
}

/// The newest verified historical backup, ignoring newer incomplete copies
/// only while the bounded inspection can establish the older candidate.
#[cfg(test)]
pub(super) fn latest_device_knowledge_skill_backup(backup_root: &Path) -> Option<PathBuf> {
    let mut inspection = SkillBackupInspection::default();
    let candidates = device_knowledge_skill_backups(backup_root, &mut inspection).ok()?;
    newest_verified_skill_backup(&candidates, &mut inspection, &mut |_, _| Ok(())).ok()?
}

type SkillBackupOrder = (u128, u64, u64);
type SkillBackupEntry = (SkillBackupOrder, PathBuf);

fn device_knowledge_skill_backups(
    backup_root: &Path,
    inspection: &mut SkillBackupInspection,
) -> Result<Vec<SkillBackupEntry>, String> {
    let prefix = format!("{DEVICE_KNOWLEDGE_SKILL_NAME}-");
    let entries = match fs::read_dir(backup_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "Could not inspect skill backups at {}: {error}; the current skill was left untouched.",
                backup_root.display()
            ));
        }
    };
    let mut backups = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("Could not inspect a skill backup: {error}"))?;
        inspection.entry(entry.path().as_os_str().len())?;
        let candidate = (|| {
            let name = entry.file_name().into_string().ok()?;
            let mut parts = name.strip_prefix(&prefix)?.split('-');
            let stamp = parts.next()?.parse::<u128>().ok()?;
            let pid = parts.next()?.parse::<u64>().ok()?;
            let sequence = parts
                .next()
                .map(str::parse::<u64>)
                .transpose()
                .ok()?
                .unwrap_or(0);
            if parts.next().is_some() {
                return None;
            }
            Some((stamp, pid, sequence))
        })();
        if let Some(stamp) = candidate {
            if entry
                .file_type()
                .map_err(|error| format!("Could not inspect {}: {error}", entry.path().display()))?
                .is_dir()
            {
                backups.push((stamp, entry.path()));
            }
        }
    }
    Ok(backups)
}

#[cfg(test)]
fn newest_verified_skill_backup(
    candidates: &[SkillBackupEntry],
    inspection: &mut SkillBackupInspection,
    fault: &mut impl FnMut(&str, &Path) -> Result<(), String>,
) -> Result<Option<PathBuf>, String> {
    Ok(select_verified_user_skill_backup(candidates, inspection, fault)?.restorable)
}

#[derive(Default)]
struct SkillBackupSelection {
    restorable: Option<PathBuf>,
    unverified: Option<PathBuf>,
}

fn inspected_skill_json(
    file: &mut fs::File,
    inspection: &mut SkillBackupInspection,
) -> Result<Value, String> {
    let mut contents = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let remaining = SKILL_DOCUMENT_LIMIT.saturating_sub(contents.len() as u64);
        let available = (remaining + 1).min(buffer.len() as u64) as usize;
        let count = inspection.read(file, &mut buffer[..available])?;
        if count == 0 {
            break;
        }
        if count as u64 > remaining {
            return Err(inspection.exhausted());
        }
        contents.extend_from_slice(&buffer[..count]);
    }
    serde_json::from_slice(&contents).map_err(|error| error.to_string())
}

fn select_verified_user_skill_backup(
    candidates: &[SkillBackupEntry],
    inspection: &mut SkillBackupInspection,
    fault: &mut impl FnMut(&str, &Path) -> Result<(), String>,
) -> Result<SkillBackupSelection, String> {
    let mut selection = SkillBackupSelection::default();
    // max_by_key previously selected the LAST verified entry on an equal
    // parsed key (legacy omitted/zero-padded sequence names can tie).
    let mut ordered: Vec<_> = candidates.iter().rev().collect();
    ordered.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, backup) in ordered {
        let receipt_path = skill_backup_receipt_path(backup);
        let mut receipt = match fs::File::open(&receipt_path) {
            Ok(file) => file,
            Err(_) => {
                selection.unverified.get_or_insert_with(|| backup.clone());
                continue;
            }
        };
        fault("inspect-backup-receipt", &receipt_path)?;
        let expected = inspected_skill_json(&mut receipt, inspection);
        let valid = expected.ok().filter(|expected| {
            skill_tree_inventory_with_inspection(backup, Some(&mut *inspection))
                .ok()
                .as_ref()
                == Some(expected)
        });
        if inspection.exhausted {
            return Err(inspection.exhausted());
        }
        if let Some(inventory) = valid {
            // A verified managed predecessor is rollback history, not the
            // user's displaced original. Modified managed copies remain user
            // data and are eligible for restoration. All extra reads share the
            // same bounded backup inspection, including legacy receipts.
            let managed = inventory["files"]
                .get(DEVICE_KNOWLEDGE_SKILL_MARKER)
                .is_some()
                && fs::File::open(backup.join(DEVICE_KNOWLEDGE_SKILL_MARKER))
                    .ok()
                    .and_then(|mut file| inspected_skill_json(&mut file, inspection).ok())
                    .is_some_and(|marker| managed_skill_inventory_matches(&inventory, &marker));
            if inspection.exhausted {
                return Err(inspection.exhausted());
            }
            if managed {
                continue;
            }
            selection.restorable = Some(backup.clone());
            return Ok(selection);
        }
        selection.unverified.get_or_insert_with(|| backup.clone());
    }
    Ok(selection)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct RemovedDeviceKnowledgeSkill {
    pub(super) removed: bool,
    pub(super) restored_from: Option<PathBuf>,
}

/// Unpairing used to remove routes and permissions and leave the installed
/// skill - and the copy of the user's own skill it had moved aside - behind
/// forever.  Take the managed skill back out and put their copy back.
pub(super) fn remove_device_knowledge_skill_at(
    skill_directory: &Path,
    backup_root: &Path,
) -> Result<RemovedDeviceKnowledgeSkill, String> {
    remove_device_knowledge_skill_with(skill_directory, backup_root, &mut |_, _| Ok(()))
}

fn retire_managed_skill(
    skill: &Path,
    retired: &Path,
    fault: &mut impl FnMut(&str, &Path) -> Result<(), String>,
) -> Result<(), String> {
    if retired.exists() {
        return Err("The managed skill staging path is already occupied.".to_string());
    }
    fault("unpair-retire", skill)?;
    fs::rename(skill, retired)
        .map_err(|error| format!("Could not preserve the current managed skill: {error}"))?;
    if !device_knowledge_skill_status_at(retired)["state"]
        .as_str()
        .is_some_and(device_knowledge_skill_is_managed)
    {
        let reason =
            "The skill changed before unpairing; it is now user-owned and was not removed.";
        return Err(match fs::rename(retired, skill) {
            Ok(()) => reason.to_owned(),
            Err(error) => format!(
                "{reason} Its exact bytes are retained at {}; restoring its location failed: {error}",
                retired.display()
            ),
        });
    }
    Ok(())
}

fn remove_device_knowledge_skill_with(
    skill_directory: &Path,
    backup_root: &Path,
    fault: &mut impl FnMut(&str, &Path) -> Result<(), String>,
) -> Result<RemovedDeviceKnowledgeSkill, String> {
    remove_device_knowledge_skill_with_inspection(
        skill_directory,
        backup_root,
        fault,
        &mut SkillBackupInspection::default(),
    )
}

fn remove_device_knowledge_skill_with_inspection(
    skill_directory: &Path,
    backup_root: &Path,
    fault: &mut impl FnMut(&str, &Path) -> Result<(), String>,
    inspection: &mut SkillBackupInspection,
) -> Result<RemovedDeviceKnowledgeSkill, String> {
    if !skill_directory.exists() {
        return Ok(RemovedDeviceKnowledgeSkill::default());
    }
    let state = device_knowledge_skill_status_at(skill_directory);
    if !state
        .get("state")
        .and_then(Value::as_str)
        .is_some_and(device_knowledge_skill_is_managed)
    {
        return Ok(RemovedDeviceKnowledgeSkill::default());
    }
    let candidates = device_knowledge_skill_backups(backup_root, inspection)?;
    let selection = select_verified_user_skill_backup(&candidates, inspection, fault)?;
    if selection.restorable.is_none() {
        if let Some(ambiguous) = selection.unverified {
            return Err(format!(
                "The existing skill backup at {} has no valid completeness receipt. The current skill and all backups were left untouched. Recover or move that backup aside manually before unpairing.",
                ambiguous.display()
            ));
        }
    }
    let restored_from = match selection.restorable {
        Some(backup) => {
            // Prepare and verify before touching the live managed skill. A
            // damaged backup or failed restore copy cannot leave a partial
            // directory in place of the user's or the current installed copy.
            let parent = skill_directory
                .parent()
                .ok_or("The skill destination has no parent directory.")?;
            let suffix = skill_install_suffix()?;
            let restoring = parent.join(format!(
                ".{DEVICE_KNOWLEDGE_SKILL_NAME}-{suffix}.restore.tmp"
            ));
            let mut restored = OwnedSkillPath::directory(restoring.clone())?;
            let skipped = copy_skill_directory_with(&backup, &restoring, fault)?;
            if !skipped.is_empty()
                || !verified_skill_backup(&backup)
                || skill_tree_inventory(&restoring)? != skill_tree_inventory(&backup)?
            {
                return Err("The skill backup changed during restore; the current skill was left untouched.".to_string());
            }
            let retired = parent.join(format!(
                ".{DEVICE_KNOWLEDGE_SKILL_NAME}-{suffix}.managed.tmp"
            ));
            retire_managed_skill(skill_directory, &retired, fault)?;
            let activate = fault("unpair-activate", skill_directory).and_then(|()| {
                fs::rename(&restoring, skill_directory).map_err(|error| error.to_string())
            });
            if let Err(error) = activate {
                return Err(match fs::rename(&retired, skill_directory) {
                    Ok(()) => format!(
                        "Could not restore the previous skill: {error}; the current managed skill was preserved."
                    ),
                    Err(restore) => format!(
                        "Could not restore the previous skill: {error}. The managed skill is retained at {}; rollback also failed: {restore}",
                        retired.display()
                    ),
                });
            }
            restored.keep();
            fs::remove_dir_all(&retired).map_err(|error| format!("The original skill was restored but managed staging cleanup failed at {}: {error}", retired.display()))?;
            Some(backup)
        }
        None => {
            let parent = skill_directory
                .parent()
                .ok_or("The skill destination has no parent directory.")?;
            let retired = parent.join(format!(
                ".{DEVICE_KNOWLEDGE_SKILL_NAME}-{}.managed.tmp",
                skill_install_suffix()?
            ));
            retire_managed_skill(skill_directory, &retired, fault)?;
            if let Err(error) = fs::remove_dir_all(&retired) {
                return Err(format!(
                    "Could not remove the detached managed skill at {}: {error}; remaining bytes are retained there.",
                    retired.display()
                ));
            }
            None
        }
    };
    Ok(RemovedDeviceKnowledgeSkill {
        removed: true,
        restored_from,
    })
}

pub(super) fn codex_device_knowledge_skill_path() -> Option<PathBuf> {
    codex_config_path()?
        .parent()
        .map(|codex_home| codex_home.join("skills").join(DEVICE_KNOWLEDGE_SKILL_NAME))
}

/// Where Bionic reads a global skill from.
///
/// Its own bundled `skill-management` skill states the layout: a global skill
/// is `~/.lmstudio/skills/<name>/SKILL.md`, the folder name must equal the
/// front matter's `name`, and the folder may carry the skill's other files -
/// which is exactly the shape `DEVICE_KNOWLEDGE_SKILL_FILES` already installs
/// for Codex and Antigravity, so no second layout is needed.
///
/// That document also tells *Bionic's own agent* not to write this directory by
/// hand, because the agent has an install tool that validates the skill and
/// asks the user to approve it. That instruction is addressed to a model acting
/// inside Bionic, not to an installer: QC Remote has no access to that tool, the
/// user's own Pair click is the approval it stands in for, and the same write
/// already serves Codex and Antigravity. It still installs the way those do -
/// backing up whatever it displaces - rather than assuming the directory is
/// ours.
///
/// The legacy `~/.cache/lm-studio/skills` location is honoured only when the
/// current one is absent and it exists, which is the same "rare, very old
/// installation" the document describes. Preferring the current path keeps a
/// machine that has both from being written to the one Bionic no longer reads.
/// Resolved against a named home, so the precedence is testable without
/// touching `USERPROFILE` - the same split as `resolve_desktop_app_in`. The
/// environment read stays in the wrapper: Rust runs tests in parallel, and a
/// test that set `USERPROFILE` would change what every other test resolving a
/// profile path sees.
pub(super) fn bionic_device_knowledge_skill_path_in(home: &Path) -> PathBuf {
    let current_root = home.join(".lmstudio");
    let legacy_root = home.join(".cache").join("lm-studio");
    let current = current_root
        .join("skills")
        .join(DEVICE_KNOWLEDGE_SKILL_NAME);
    let legacy = legacy_root.join("skills").join(DEVICE_KNOWLEDGE_SKILL_NAME);
    // An installed copy decides it, before either root does. Reading the roots
    // alone made the answer depend on when it was asked: status and install are
    // separate commands behind separate clicks, so Bionic creating
    // `~/.lmstudio` between them sent the install to a directory the status
    // call had never looked at. Once the skill exists somewhere, that is the
    // answer for every later call - which is what makes update and unpair reach
    // the copy pair actually wrote.
    //
    // Otherwise the current root wins wherever it exists, and the legacy one is
    // for the "rare, very old installation" Bionic's own documentation
    // describes - a machine that has never had `~/.lmstudio` at all.
    if legacy.is_dir() || !current_root.is_dir() && legacy_root.is_dir() {
        return legacy;
    }
    current
}

pub(super) fn bionic_device_knowledge_skill_path() -> Option<PathBuf> {
    Some(bionic_device_knowledge_skill_path_in(&PathBuf::from(
        std::env::var_os("USERPROFILE")?,
    )))
}

pub(super) fn antigravity_device_knowledge_skill_path() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|home| {
        PathBuf::from(home)
            .join(".gemini")
            .join("config")
            .join("skills")
            .join(DEVICE_KNOWLEDGE_SKILL_NAME)
    })
}

/// Backups are kept per host.  A shared root would let a ChatGPT unpair restore
/// the copy an Antigravity pair had taken - the two skill directories live in
/// different profiles and are not interchangeable - and would let two pairs in
/// the same millisecond of the same process share one backup directory name.
pub(super) fn device_knowledge_skill_backup_root(host: &str) -> Result<PathBuf, String> {
    Ok(canonical_integration_directory()?
        .join("skill-backups")
        .join(host))
}

#[cfg(test)]
mod owned_skill_transaction_tests {
    use super::*;

    fn fixture() -> (OwnedSkillPath, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "qc-owned-skill-{}",
            skill_install_suffix().unwrap()
        ));
        let owned = OwnedSkillPath::directory(root.clone()).unwrap();
        let skill = root.join("skills").join(DEVICE_KNOWLEDGE_SKILL_NAME);
        fs::create_dir_all(skill.join("empty")).unwrap();
        fs::write(skill.join("SKILL.md"), b"owner original").unwrap();
        fs::write(skill.join("NOTES.md"), b"owner notes").unwrap();
        (owned, skill, root.join("backups"))
    }

    fn backup_probe_bytes(backup: &Path) -> u64 {
        fs::metadata(skill_backup_receipt_path(backup))
            .unwrap()
            .len()
            + fs::metadata(backup.join("SKILL.md")).unwrap().len()
            + fs::metadata(backup.join("NOTES.md")).unwrap().len()
    }

    #[test]
    fn unpair_backup_sibling_entry_and_path_overflow_preserve_current_and_all_history() {
        for path_limit in [false, true] {
            let (_root, skill, backups) = fixture();
            assert!(install_device_knowledge_skill_at(&skill, &backups).installed);
            for index in 0..3 {
                fs::create_dir(backups.join(format!(
                    "{DEVICE_KNOWLEDGE_SKILL_NAME}-{}-99-0",
                    u128::MAX - index
                )))
                .unwrap();
            }
            let current = skill_tree_inventory(&skill).unwrap();
            let history = skill_tree_inventory(&backups).unwrap();
            let mut inspection = SkillBackupInspection::default();
            if path_limit {
                inspection.paths_remaining = 1;
            } else {
                inspection.entries_remaining = 2;
            }
            let mut retired = false;
            let error = remove_device_knowledge_skill_with_inspection(
                &skill,
                &backups,
                &mut |step, _| {
                    retired |= step == "unpair-retire";
                    Ok(())
                },
                &mut inspection,
            )
            .unwrap_err();
            assert!(
                error.contains("inspection") && error.contains("budget"),
                "{error}"
            );
            assert!(inspection.exhausted);
            assert!(
                !retired,
                "unknown sibling inventory must precede live-tree retirement"
            );
            assert_eq!(skill_tree_inventory(&skill).unwrap(), current);
            assert_eq!(skill_tree_inventory(&backups).unwrap(), history);
            assert_eq!(fs::read_dir(skill.parent().unwrap()).unwrap().count(), 1);
        }
    }

    #[test]
    fn invalid_newer_backup_receipt_consumes_the_same_held_read_budget_as_the_valid_fallback() {
        let (_root, skill, backups) = fixture();
        let installed = install_device_knowledge_skill_at(&skill, &backups);
        assert!(installed.installed);
        let approved = installed.backup.unwrap();
        let allowance = backup_probe_bytes(&approved);
        let newer = backups.join(format!("{DEVICE_KNOWLEDGE_SKILL_NAME}-{}-99-0", u128::MAX));
        fs::create_dir(&newer).unwrap();
        let invalid = vec![b' '; (allowance / 2) as usize];
        fs::write(skill_backup_receipt_path(&newer), &invalid).unwrap();
        assert!(
            fs::metadata(skill_backup_receipt_path(&approved))
                .unwrap()
                .len()
                <= allowance
        );
        assert!(invalid.len() as u64 <= allowance, "each receipt alone fits");
        let current = skill_tree_inventory(&skill).unwrap();
        let history = skill_tree_inventory(&backups).unwrap();
        let mut inspection = SkillBackupInspection {
            bytes_remaining: allowance,
            ..Default::default()
        };
        let mut retired = false;
        let error = remove_device_knowledge_skill_with_inspection(
            &skill,
            &backups,
            &mut |step, _| {
                retired |= step == "unpair-retire";
                Ok(())
            },
            &mut inspection,
        )
        .unwrap_err();
        assert!(error.contains("budget"), "{error}");
        assert!(inspection.exhausted);
        assert!(!retired);
        assert_eq!(skill_tree_inventory(&skill).unwrap(), current);
        assert_eq!(skill_tree_inventory(&backups).unwrap(), history);

        // The same actual removal transaction still falls back past malformed
        // newer metadata when its one shared inspection budget is sufficient.
        let mut inspection = SkillBackupInspection {
            bytes_remaining: allowance + invalid.len() as u64,
            ..Default::default()
        };
        let removed = remove_device_knowledge_skill_with_inspection(
            &skill,
            &backups,
            &mut |_, _| Ok(()),
            &mut inspection,
        )
        .unwrap();
        assert_eq!(removed.restored_from, Some(approved));
        assert_eq!(
            inspection.bytes_remaining, 0,
            "actual receipt and file bytes share one budget"
        );
        assert!(!inspection.exhausted);
        assert_eq!(fs::read(skill.join("SKILL.md")).unwrap(), b"owner original");
        assert_eq!(fs::read(skill.join("NOTES.md")).unwrap(), b"owner notes");
        assert_eq!(
            fs::read(skill_backup_receipt_path(&newer)).unwrap(),
            invalid
        );
    }

    #[test]
    fn backup_receipt_growth_after_open_is_bounded_before_unpair_retirement() {
        let (_root, skill, backups) = fixture();
        let installed = install_device_knowledge_skill_at(&skill, &backups);
        assert!(installed.installed);
        let approved = installed.backup.unwrap();
        let allowance = backup_probe_bytes(&approved);
        let receipt = skill_backup_receipt_path(&approved);
        let current = skill_tree_inventory(&skill).unwrap();
        let mut grew_while_held = false;
        let mut retired = false;
        let mut inspection = SkillBackupInspection {
            bytes_remaining: allowance,
            ..Default::default()
        };
        let error = remove_device_knowledge_skill_with_inspection(
            &skill,
            &backups,
            &mut |step, path| {
                retired |= step == "unpair-retire";
                if step == "inspect-backup-receipt" {
                    assert_eq!(path, receipt);
                    fs::OpenOptions::new()
                        .write(true)
                        .open(path)
                        .unwrap()
                        .set_len(allowance + 1)
                        .unwrap();
                    grew_while_held = true;
                }
                Ok(())
            },
            &mut inspection,
        )
        .unwrap_err();
        assert!(grew_while_held);
        assert!(error.contains("budget"), "{error}");
        assert!(inspection.exhausted);
        assert!(!retired);
        assert_eq!(fs::metadata(&receipt).unwrap().len(), allowance + 1);
        assert_eq!(skill_tree_inventory(&skill).unwrap(), current);
        assert_eq!(
            fs::read(approved.join("SKILL.md")).unwrap(),
            b"owner original"
        );
        assert_eq!(fs::read_dir(skill.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn newest_valid_backup_does_not_inspect_older_unneeded_history() {
        let (_root, skill, backups) = fixture();
        let installed = install_device_knowledge_skill_at(&skill, &backups);
        assert!(installed.installed);
        let approved = installed.backup.unwrap();
        let allowance = backup_probe_bytes(&approved);
        let older = backups.join(format!("{DEVICE_KNOWLEDGE_SKILL_NAME}-0-1-0"));
        fs::create_dir(&older).unwrap();
        let old_receipt = skill_backup_receipt_path(&older);
        fs::File::create(&old_receipt)
            .unwrap()
            .set_len(SKILL_DOCUMENT_LIMIT + 1)
            .unwrap();
        let mut inspection = SkillBackupInspection {
            bytes_remaining: allowance,
            ..Default::default()
        };
        let mut probed = Vec::new();
        let removed = remove_device_knowledge_skill_with_inspection(
            &skill,
            &backups,
            &mut |step, path| {
                if step == "inspect-backup-receipt" {
                    probed.push(path.to_path_buf());
                }
                Ok(())
            },
            &mut inspection,
        )
        .unwrap();
        assert_eq!(removed.restored_from, Some(approved.clone()));
        assert_eq!(probed, vec![skill_backup_receipt_path(&approved)]);
        assert_eq!(inspection.bytes_remaining, 0);
        assert!(!inspection.exhausted);
        assert_eq!(
            fs::metadata(&old_receipt).unwrap().len(),
            SKILL_DOCUMENT_LIMIT + 1
        );
        assert_eq!(fs::read(skill.join("NOTES.md")).unwrap(), b"owner notes");
    }

    #[test]
    fn equal_order_verified_backups_preserve_the_previous_last_valid_tie_policy() {
        let (_root, skill, backups) = fixture();
        let installed = install_device_knowledge_skill_at(&skill, &backups);
        assert!(installed.installed);
        let approved = installed.backup.unwrap();
        for suffix in ["1-1", "01-1-0"] {
            let alias = backups.join(format!("{DEVICE_KNOWLEDGE_SKILL_NAME}-{suffix}"));
            fs::create_dir(&alias).unwrap();
            assert!(
                copy_skill_directory_with(&approved, &alias, &mut |_, _| Ok(()))
                    .unwrap()
                    .is_empty()
            );
            fs::copy(
                skill_backup_receipt_path(&approved),
                skill_backup_receipt_path(&alias),
            )
            .unwrap();
            assert!(verified_skill_backup(&alias));
        }
        let mut parsed =
            device_knowledge_skill_backups(&backups, &mut SkillBackupInspection::default())
                .unwrap();
        parsed.retain(|(order, _)| *order == (1, 1, 0));
        assert_eq!(
            parsed.len(),
            2,
            "actual legacy names must parse to the same order"
        );
        let mut reversed = parsed.clone();
        reversed.reverse();
        for candidates in [parsed, reversed] {
            let previous = candidates
                .iter()
                .filter(|(_, path)| verified_skill_backup(path))
                .max_by_key(|(order, _)| *order)
                .map(|(_, path)| path.clone());
            let selected = newest_verified_skill_backup(
                &candidates,
                &mut SkillBackupInspection::default(),
                &mut |_, _| Ok(()),
            )
            .unwrap();
            assert_eq!(selected, previous);
            assert_eq!(selected, Some(candidates[1].1.clone()));
        }
    }

    #[test]
    fn streamed_skill_hash_matches_complete_bytes_and_refuses_a_growing_budget() {
        let (_root, skill, _) = fixture();
        let path = skill.join("hash.txt");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(
            streamed_skill_hash(&path, 3).unwrap(),
            (
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
                3
            )
        );
        assert!(streamed_skill_hash(&path, 2).is_err());
    }

    #[test]
    fn post_inventory_growth_is_refused_before_an_oversized_backup_is_completed() {
        let (_root, skill, backups) = fixture();
        let mut taken = None;
        let mut grew_after_inventory = false;
        let mut oversized_copy_completed = false;
        let result =
            stage_device_knowledge_skill_with(&skill, &backups, &mut taken, &mut |step, target| {
                if step == "backup-file"
                    && target.file_name().is_some_and(|name| name == "NOTES.md")
                {
                    fs::OpenOptions::new()
                        .write(true)
                        .open(skill.join("NOTES.md"))
                        .unwrap()
                        .set_len(SKILL_FILE_LIMIT + 1)
                        .unwrap();
                    grew_after_inventory = true;
                }
                if step == "backup-verify" {
                    oversized_copy_completed = fs::metadata(target.join("NOTES.md"))
                        .is_ok_and(|metadata| metadata.len() > SKILL_FILE_LIMIT);
                }
                Ok(())
            });
        assert!(
            grew_after_inventory,
            "the actual bounded preflight must finish before source growth"
        );
        assert!(result.unwrap_err().contains("budget"));
        assert!(
            !oversized_copy_completed,
            "copy must enforce the byte ceiling before completing oversized staging"
        );
        assert_eq!(
            fs::metadata(skill.join("NOTES.md")).unwrap().len(),
            SKILL_FILE_LIMIT + 1
        );
        assert_eq!(fs::read(skill.join("SKILL.md")).unwrap(), b"owner original");
        assert!(taken.is_none());
        assert_eq!(fs::read_dir(&backups).unwrap().count(), 0);
        assert_eq!(fs::read_dir(skill.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn post_inventory_added_depth_is_refused_before_the_unbounded_tree_is_copied() {
        let (_root, skill, backups) = fixture();
        let mut taken = None;
        let mut added_after_inventory = false;
        let mut too_deep_copy_completed = false;
        let mut relative = PathBuf::from("late");
        for _ in 0..SKILL_TREE_DEPTH_LIMIT {
            relative.push("d");
        }
        let result =
            stage_device_knowledge_skill_with(&skill, &backups, &mut taken, &mut |step, path| {
                if step == "copy-directory" && path == skill.as_path() {
                    fs::create_dir_all(skill.join(&relative)).unwrap();
                    added_after_inventory = true;
                }
                if step == "backup-verify" {
                    too_deep_copy_completed = path.join(&relative).is_dir();
                }
                Ok(())
            });
        assert!(
            added_after_inventory,
            "inert copy seam must run after the actual preflight inventory"
        );
        assert!(result.unwrap_err().contains("budget"));
        assert!(
            !too_deep_copy_completed,
            "the depth ceiling must apply during copy, not only after it"
        );
        assert!(
            skill.join(relative).is_dir(),
            "the concurrent user's source change is preserved"
        );
        assert_eq!(fs::read(skill.join("NOTES.md")).unwrap(), b"owner notes");
        assert!(taken.is_none());
        assert_eq!(fs::read_dir(&backups).unwrap().count(), 0);
        assert_eq!(fs::read_dir(skill.parent().unwrap()).unwrap().count(), 1);
    }
    #[test]
    fn oversized_skill_data_and_metadata_refuse_before_original_retirement() {
        let (_root, skill, backups) = fixture();
        let original = skill.join("NOTES.md");
        fs::OpenOptions::new()
            .write(true)
            .open(&original)
            .unwrap()
            .set_len(SKILL_FILE_LIMIT + 1)
            .unwrap();
        let installed = install_device_knowledge_skill_at(&skill, &backups);
        assert!(!installed.installed);
        assert!(installed.error.unwrap().contains("inspection budget"));
        assert_eq!(fs::metadata(&original).unwrap().len(), SKILL_FILE_LIMIT + 1);
        assert!(fs::read_dir(&backups).unwrap().next().is_none());
        let marker = skill.join(DEVICE_KNOWLEDGE_SKILL_MARKER);
        fs::File::create(&marker)
            .unwrap()
            .set_len(SKILL_DOCUMENT_LIMIT + 1)
            .unwrap();
        assert!(bounded_skill_document(&marker).is_err());
        assert_eq!(
            device_knowledge_skill_status_at(&skill)["state"],
            "different-configuration"
        );
    }

    #[test]
    fn deep_skill_inventory_fails_closed_before_copy_or_retirement() {
        let (_root, skill, backups) = fixture();
        let mut deepest = skill.clone();
        for _ in 0..SKILL_TREE_DEPTH_LIMIT + 1 {
            deepest = deepest.join("d");
            fs::create_dir(&deepest).unwrap();
        }
        let installed = install_device_knowledge_skill_at(&skill, &backups);
        assert!(!installed.installed);
        assert!(installed.error.unwrap().contains("depth budget"));
        assert!(deepest.is_dir());
        assert_eq!(fs::read(skill.join("SKILL.md")).unwrap(), b"owner original");
        assert!(fs::read_dir(&backups).unwrap().next().is_none());
    }

    #[test]
    fn injected_skill_install_failures_clean_owned_paths_and_preserve_the_exact_original() {
        for failed_step in [
            "stage-file",
            "stage-marker",
            "backup-file",
            "backup-verify",
            "retire-original",
            "activate",
            "publish-receipt",
        ] {
            let (_root, skill, backups) = fixture();
            let before = skill_tree_inventory(&skill).unwrap();
            let mut taken = None;
            let mut files = 0;
            let outcome = stage_device_knowledge_skill_with(
                &skill,
                &backups,
                &mut taken,
                &mut |step, path| {
                    if step != failed_step {
                        return Ok(());
                    }
                    if step.ends_with("file") {
                        files += 1;
                        if files < 2 {
                            return Ok(());
                        }
                        fs::write(path, b"partially written bytes").unwrap();
                    }
                    Err(format!("injected {step}"))
                },
            );
            assert!(outcome.unwrap_err().contains("injected"), "{failed_step}");
            assert_eq!(
                skill_tree_inventory(&skill).unwrap(),
                before,
                "{failed_step}"
            );
            assert!(
                taken.is_none(),
                "a recovered failure publishes no abandoned backup: {failed_step}"
            );
            assert_eq!(
                fs::read_dir(skill.parent().unwrap()).unwrap().count(),
                1,
                "staging and retained original removed: {failed_step}"
            );
            assert_eq!(
                fs::read_dir(&backups)
                    .map(|entries| entries.count())
                    .unwrap_or(0),
                0,
                "partial copy and pending receipt removed: {failed_step}"
            );
            assert!(latest_device_knowledge_skill_backup(&backups).is_none());
        }
    }

    #[test]
    fn skill_changed_at_retirement_is_restored_from_its_exact_original_not_the_stale_copy() {
        let (_root, skill, backups) = fixture();
        let mut taken = None;
        let result =
            stage_device_knowledge_skill_with(&skill, &backups, &mut taken, &mut |step, _| {
                if step == "retire-original" {
                    fs::write(skill.join("NOTES.md"), b"new owner edit").unwrap();
                }
                Ok(())
            });
        assert!(result.unwrap_err().contains("changed before replacement"));
        assert_eq!(fs::read(skill.join("NOTES.md")).unwrap(), b"new owner edit");
        assert!(taken.is_none());
        assert_eq!(fs::read_dir(&backups).unwrap().count(), 0);
    }

    #[test]
    fn failed_activation_and_rollback_retain_the_original_and_report_verified_recovery_bytes() {
        let (_root, skill, backups) = fixture();
        let before = skill_tree_inventory(&skill).unwrap();
        let mut taken = None;
        let result =
            stage_device_knowledge_skill_with(&skill, &backups, &mut taken, &mut |step, _| {
                if matches!(step, "activate" | "rollback") {
                    Err(format!("injected {step}"))
                } else {
                    Ok(())
                }
            });
        let detail = result.unwrap_err();
        assert!(detail.contains("original directory is still at"));
        let backup = taken.unwrap();
        assert_eq!(skill_tree_inventory(&backup).unwrap(), before);
        assert!(
            !verified_skill_backup(&backup),
            "recovery output is not an approved successful-install backup"
        );
        assert!(latest_device_knowledge_skill_backup(&backups).is_none());
        let retained = fs::read_dir(skill.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.to_string_lossy().ends_with(".original.tmp"))
            .unwrap();
        assert_eq!(skill_tree_inventory(&retained).unwrap(), before);
    }

    #[test]
    fn unpair_restores_only_a_verified_complete_backup_and_never_its_sidecar() {
        let (_root, skill, backups) = fixture();
        let before = skill_tree_inventory(&skill).unwrap();
        let installed = install_device_knowledge_skill_at(&skill, &backups);
        assert!(installed.installed, "{:?}", installed.error);
        let approved = installed.backup.unwrap();
        let partial = backups.join(format!("{DEVICE_KNOWLEDGE_SKILL_NAME}-{}-999-0", u128::MAX));
        fs::create_dir(&partial).unwrap();
        fs::write(partial.join("SKILL.md"), b"incomplete newer copy").unwrap();
        assert_eq!(
            latest_device_knowledge_skill_backup(&backups),
            Some(approved.clone())
        );
        let removed = remove_device_knowledge_skill_at(&skill, &backups).unwrap();
        assert_eq!(removed.restored_from, Some(approved));
        assert_eq!(
            skill_tree_inventory(&skill).unwrap(),
            before,
            "receipt is beside backup, never added to the owner's tree"
        );
    }

    #[test]
    fn repeated_managed_skill_updates_unpair_to_the_user_original_or_absence() {
        for had_original in [false, true] {
            let (_root, skill, backups) = fixture();
            let before = skill_tree_inventory(&skill).unwrap();
            if !had_original {
                fs::remove_dir_all(&skill).unwrap();
            }
            for _ in 0..3 {
                let installed = install_device_knowledge_skill_at(&skill, &backups);
                assert!(installed.installed, "{:?}", installed.error);
                assert!(device_knowledge_skill_is_managed(
                    device_knowledge_skill_status_at(&skill)["state"]
                        .as_str()
                        .unwrap()
                ));
            }
            let removed = remove_device_knowledge_skill_at(&skill, &backups).unwrap();
            if had_original {
                assert!(removed.restored_from.is_some());
                assert_eq!(skill_tree_inventory(&skill).unwrap(), before);
            } else {
                assert!(removed.removed);
                assert!(removed.restored_from.is_none());
                assert!(!skill.exists());
            }
        }
    }

    #[test]
    fn modified_managed_skill_backup_is_user_data_and_survives_later_updates() {
        let (_root, skill, backups) = fixture();
        let installed = install_device_knowledge_skill_at(&skill, &backups);
        assert!(installed.installed, "{:?}", installed.error);
        fs::write(skill.join("personal-notes.md"), b"user additions").unwrap();
        let modified = skill_tree_inventory(&skill).unwrap();
        for _ in 0..2 {
            let installed = install_device_knowledge_skill_at(&skill, &backups);
            assert!(installed.installed, "{:?}", installed.error);
        }
        let removed = remove_device_knowledge_skill_at(&skill, &backups).unwrap();
        assert!(removed.restored_from.is_some());
        assert_eq!(skill_tree_inventory(&skill).unwrap(), modified);
    }

    #[test]
    fn ambiguous_legacy_or_damaged_backup_refuses_unpair_before_current_skill_deletion() {
        for damaged in [false, true] {
            let (_root, skill, backups) = fixture();
            let installed = install_device_knowledge_skill_at(&skill, &backups);
            assert!(installed.installed);
            let backup = installed.backup.unwrap();
            if damaged {
                fs::write(backup.join("NOTES.md"), b"damaged").unwrap();
            } else {
                fs::remove_file(skill_backup_receipt_path(&backup)).unwrap();
            }
            let before = skill_tree_inventory(&skill).unwrap();
            let error = remove_device_knowledge_skill_at(&skill, &backups).unwrap_err();
            assert!(
                error.contains("no valid completeness receipt") && error.contains("left untouched")
            );
            assert_eq!(skill_tree_inventory(&skill).unwrap(), before);
            assert!(backup.exists());
        }
    }

    #[test]
    fn a_collision_never_gives_cleanup_ownership_of_an_existing_directory() {
        let (_root, skill, _) = fixture();
        assert!(OwnedSkillPath::directory(skill.clone()).is_err());
        assert_eq!(fs::read(skill.join("SKILL.md")).unwrap(), b"owner original");
    }

    #[test]
    fn user_added_files_and_empty_directories_are_not_managed_or_deleted_by_unpair() {
        for directory in [false, true] {
            let (_root, skill, backups) = fixture();
            assert!(install_device_knowledge_skill_at(&skill, &backups).installed);
            let addition = skill.join("owner-addition");
            if directory {
                fs::create_dir(&addition).unwrap();
            } else {
                fs::write(&addition, b"owner added bytes").unwrap();
            }
            let before = skill_tree_inventory(&skill).unwrap();
            assert_eq!(
                device_knowledge_skill_status_at(&skill)["state"],
                "modified"
            );
            assert_eq!(
                remove_device_knowledge_skill_at(&skill, &backups).unwrap(),
                RemovedDeviceKnowledgeSkill::default()
            );
            assert_eq!(skill_tree_inventory(&skill).unwrap(), before);
        }
    }

    #[test]
    fn an_empty_forged_managed_marker_never_authorizes_removing_a_user_tree() {
        let (_root, skill, backups) = fixture();
        fs::write(
            skill.join(DEVICE_KNOWLEDGE_SKILL_MARKER),
            serde_json::to_vec(&json!({
                "schemaVersion": 1, "name": DEVICE_KNOWLEDGE_SKILL_NAME,
                "skillVersion": DEVICE_KNOWLEDGE_SKILL_VERSION, "files": {}
            }))
            .unwrap(),
        )
        .unwrap();
        let before = skill_tree_inventory(&skill).unwrap();
        assert_eq!(
            device_knowledge_skill_status_at(&skill)["state"],
            "different-configuration"
        );
        assert_eq!(
            remove_device_knowledge_skill_at(&skill, &backups).unwrap(),
            RemovedDeviceKnowledgeSkill::default()
        );
        assert_eq!(skill_tree_inventory(&skill).unwrap(), before);
    }

    #[test]
    fn unpair_rechecks_exact_ownership_after_retirement_with_and_without_a_backup() {
        for with_backup in [false, true] {
            let (_root, skill, backups) = fixture();
            if !with_backup {
                fs::remove_dir_all(&skill).unwrap();
            }
            assert!(install_device_knowledge_skill_at(&skill, &backups).installed);
            let error = remove_device_knowledge_skill_with(&skill, &backups, &mut |step, path| {
                if step == "unpair-retire" {
                    fs::write(path.join("late-owner-edit.txt"), b"preserve this").unwrap();
                }
                Ok(())
            })
            .unwrap_err();
            assert!(error.contains("changed before unpairing"));
            assert_eq!(
                fs::read(skill.join("late-owner-edit.txt")).unwrap(),
                b"preserve this"
            );
            assert_eq!(
                device_knowledge_skill_status_at(&skill)["state"],
                "modified"
            );
            assert_eq!(
                fs::read_dir(skill.parent().unwrap()).unwrap().count(),
                1,
                "restore staging and retained tree cleaned after safe rollback"
            );
        }
    }

    #[test]
    fn failed_unpair_restore_copy_or_activation_preserves_current_managed_bytes() {
        for failed_step in ["backup-file", "unpair-activate"] {
            let (_root, skill, backups) = fixture();
            assert!(install_device_knowledge_skill_at(&skill, &backups).installed);
            let before = skill_tree_inventory(&skill).unwrap();
            let result = remove_device_knowledge_skill_with(&skill, &backups, &mut |step, path| {
                if step == failed_step {
                    if step == "backup-file" {
                        fs::write(path, b"partial restore").unwrap();
                    }
                    return Err(format!("injected {step}"));
                }
                Ok(())
            });
            assert!(result.unwrap_err().contains("injected"));
            assert_eq!(skill_tree_inventory(&skill).unwrap(), before);
            assert_eq!(fs::read_dir(skill.parent().unwrap()).unwrap().count(), 1);
        }
    }

    #[test]
    fn unreadable_backup_root_is_not_treated_as_no_original_to_restore() {
        let (_root, skill, backups) = fixture();
        fs::remove_dir_all(&skill).unwrap();
        assert!(install_device_knowledge_skill_at(&skill, &backups).installed);
        fs::write(&backups, b"not a readable directory").unwrap();
        let before = skill_tree_inventory(&skill).unwrap();
        assert!(remove_device_knowledge_skill_at(&skill, &backups)
            .unwrap_err()
            .contains("Could not inspect skill backups"));
        assert_eq!(skill_tree_inventory(&skill).unwrap(), before);
    }
}
