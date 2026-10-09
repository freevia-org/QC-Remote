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
    format!(r"C:\Users\<you>\AppData\Local\{BRIDGE_DIRECTORY}\{BRIDGE_FILE_NAME}")
}

/// The same path escaped for a TOML or JSON string.
pub(super) fn escaped_bridge_path() -> String {
    installed_bridge_path().replace('\\', r"\\")
}

fn sha256_hex(contents: &[u8]) -> String {
    format!("{:x}", Sha256::digest(contents))
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
    let mut skipped = Vec::new();
    for entry in fs::read_dir(source)
        .map_err(|error| format!("Could not read {}: {error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("Could not read a skill entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("Could not inspect {}: {error}", entry.path().display()))?;
        if file_type.is_dir() {
            skipped.extend(unbackupable_entries(&entry.path())?);
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
pub(super) fn copy_directory(source: &Path, destination: &Path) -> Result<Vec<PathBuf>, String> {
    let mut skipped = Vec::new();
    fs::create_dir_all(destination)
        .map_err(|error| format!("Could not create {}: {error}", destination.display()))?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("Could not read {}: {error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("Could not read a skill entry: {error}"))?;
        let target = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("Could not inspect {}: {error}", entry.path().display()))?;
        if file_type.is_dir() {
            skipped.extend(copy_directory(&entry.path(), &target)?);
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target).map_err(|error| {
                format!("Could not back up {}: {error}", entry.path().display())
            })?;
        } else {
            skipped.push(entry.path());
        }
    }
    Ok(skipped)
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
    let marker = match fs::read_to_string(&marker_path)
        .map_err(|error| format!("Could not read the managed skill marker: {error}"))
        .and_then(|contents| {
            serde_json::from_str::<Value>(&contents)
                .map_err(|error| format!("The managed skill marker is invalid: {error}"))
        }) {
        Ok(marker) => marker,
        Err(error) => {
            return json!({
                "installed": true,
                "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
                "state": "different-configuration",
                "statusError": error,
            })
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
        let actual = fs::read(skill_directory.join(relative))
            .ok()
            .map(|contents| sha256_hex(&contents));
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
    let suffix = format!(
        "{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "The system clock is before the Unix epoch.".to_string())?
            .as_millis(),
        std::process::id()
    );
    let staging = parent.join(format!(
        ".{DEVICE_KNOWLEDGE_SKILL_NAME}.qc-remote-{suffix}.tmp"
    ));
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .map_err(|error| format!("Could not clear the staged skill directory: {error}"))?;
    }
    for file in DEVICE_KNOWLEDGE_SKILL_FILES {
        debug_assert!(safe_skill_relative_path(file.path));
        let destination = staging.join(file.path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create the staged skill directory: {error}"))?;
        }
        fs::write(&destination, file.contents)
            .map_err(|error| format!("Could not stage {}: {error}", file.path))?;
    }
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
        let unbacked = copy_directory(skill_directory, &backup)?;
        if let Some(refusal) = refuse_over_unbackupable(&unbacked, skill_directory) {
            // Reported, not discarded. This removal is the whole reason the
            // refusal can be made here at all: an abandoned backup is the
            // newest in the root and `latest_device_knowledge_skill_backup`
            // restores the newest, so a removal that quietly failed would hand
            // a later unpair a backup from a pair that never happened - the
            // exact hazard the pre-copy scan exists to prevent.
            return Err(match fs::remove_dir_all(&backup) {
                Ok(()) => refusal,
                Err(error) => format!(
                    "{refusal} A partial backup could not be cleaned up either ({error}); remove {} before unpairing.",
                    backup.display()
                ),
            });
        }
        *taken_backup = Some(backup.clone());
        fs::remove_dir_all(skill_directory)
            .map_err(|error| format!("Could not replace the existing skill: {error}"))?;
    }
    if let Err(error) = fs::rename(&staging, skill_directory) {
        // The original is already gone by here, so a rollback that also fails
        // has to say so and name where the bytes still are. Reporting only the
        // rename error would leave the user with an empty skill folder, a
        // "could not activate" message, and no idea a backup exists.
        if let Some(backup) = taken_backup.as_deref() {
            // The skipped list is knowably empty here: a backup only exists
            // because `unbackupable_entries` found nothing to refuse over, so
            // this restore has nothing it can silently drop.
            match copy_directory(backup, skill_directory) {
                Ok(_nothing_skippable) => {}
                Err(restore) => {
                    return Err(format!(
                        "Could not activate the updated skill: {error}. Restoring the previous skill also failed: {restore}. It is still at {}.",
                        backup.display()
                    ));
                }
            }
        }
        return Err(format!("Could not activate the updated skill: {error}"));
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

/// The newest `qc-device-knowledge-<unix-ms>-<pid>` directory `install_device_knowledge_skill_at`
/// left behind, which is whatever the user had before the most recent pair.
pub(super) fn latest_device_knowledge_skill_backup(backup_root: &Path) -> Option<PathBuf> {
    let prefix = format!("{DEVICE_KNOWLEDGE_SKILL_NAME}-");
    fs::read_dir(backup_root)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stamp = name
                .strip_prefix(&prefix)?
                .split('-')
                .next()?
                .parse::<u128>()
                .ok()?;
            entry
                .file_type()
                .ok()?
                .is_dir()
                .then(|| (stamp, entry.path()))
        })
        .max_by_key(|(stamp, _)| *stamp)
        .map(|(_, path)| path)
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
    fs::remove_dir_all(skill_directory)
        .map_err(|error| format!("Could not remove the managed device knowledge skill: {error}"))?;
    let restored_from = match latest_device_knowledge_skill_backup(backup_root) {
        Some(backup) => {
            // Same as the rollback: every backup in this root was written
            // after `unbackupable_entries` cleared its source, so there is
            // nothing here for the copy to skip.
            let _nothing_skippable = copy_directory(&backup, skill_directory)?;
            Some(backup)
        }
        None => None,
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
