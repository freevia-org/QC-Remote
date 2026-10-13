//! Session-fenced fold of the model preset list the device pushes.
//!
//! The device announces its model presets as a series of `ModelPresetMessage`
//! pushes: a large boot update, then single creates, renames and deletes. This
//! module folds them into one retained list per session so a host can offer,
//! and a write planner can check, the identities the device announced.
//!
//! What the fold claims is deliberately small. It never claims the list is
//! complete, because no message marks the end of the boot update, and it never
//! treats absence as proof that a preset does not exist. It claims only that,
//! within one state session and since nothing was noticed to be lost, the
//! entries it lists were announced and not since deleted. Anything that could
//! break that claim invalidates the fold until the session ends:
//!
//! - a message whose identity is incomplete (no value, hash or factory flag);
//! - a message that did not decode;
//! - any loss on the inbound transport (a coarse policy, see
//!   [`ModelPresetCatalog::observe_inbound_loss`]);
//! - more entries than the fold's inclusive maxima (see [`PUBLISHED_ENTRY_LIMIT`] and
//!   [`RETAINED_ENTRY_LIMIT`]).
//!
//! The limits are operational, not facts about the device: the number of user
//! presets is not bounded by anything this crate knows. Each limit is an
//! inclusive maximum: a list of exactly that size is accepted and the fold
//! invalidates only when a list is strictly larger. A list that outgrows a
//! limit is never truncated. The fold is invalidated, publishes no entries and
//! admits nothing for the rest of the session.
//!
//! What the fold sees is what the session owner has accepted. It cannot see
//! reports still queued below the owner, a message only half received, or
//! anything that happens between the owner's admission of a write and the
//! write reaching the USB link. Nothing here states what the device did or in
//! what order; it states what the owner observed.
//!
//! A session change replaces the whole decoder, so the fold starts again as
//! unavailable. No message here is ever sent to the device.

use crate::generated_payloads::{
    ModelPresetCatalogCounters, ModelPresetCatalogMarker, ModelPresetCatalogState,
    ModelPresetSummary,
};
use crate::proto::cortex_protobuf_v2 as pa;
use pa::message_action::Enum as Action;
use std::collections::{HashMap, HashSet};

/// Model hash of the preset family the Global EQ screen offers for recall.
///
/// This is the only hash whose entries are published and the only hash a
/// recall is admitted for. Hosts and clients read it from the published view
/// rather than repeating the number.
pub const GLOBAL_EQ_MODEL_PRESET_HASH: u32 = 4004;

/// Entries retained across every hash. An inclusive maximum: exactly this many
/// are accepted, and the fold invalidates (`retention-limit`) only when strictly
/// more would be retained, rather than grow without bound.
pub const RETAINED_ENTRY_LIMIT: usize = 20_000;

/// Entries of the scope hash the fold may hold, and so a publication may
/// carry. The projection is republished whole on every message and travels in
/// snapshots and write results, so it is bounded. This is an operational
/// limit and an inclusive maximum: exactly this many entries are accepted, and
/// the fold invalidates (`publication-limit`) only when strictly more would be
/// held.
pub const PUBLISHED_ENTRY_LIMIT: usize = 256;

/// Per retained identity, name or marker string. This generous operational cap matches
/// the capture/IR catalog and is not a device name-length claim.
pub const MAX_RETAINED_STRING_BYTES: usize = 16 * 1024;

/// Total retained string bytes, including the identity-key index and loaded marker.
/// Matching the capture/IR provider budget prevents count-bounded entries from
/// accumulating many GiB across individually valid frames. Overflow invalidates
/// the entire fold as `retention-limit`; it never truncates identities.
pub const MAX_RETAINED_BYTES: usize = 32 * 1024 * 1024;

type Tuple = (u32, bool, String);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    hash: u32,
    is_factory: bool,
    value: String,
    name: String,
    is_default: bool,
}

impl Entry {
    fn tuple(&self) -> Tuple {
        (self.hash, self.is_factory, self.value.clone())
    }

    fn matches(&self, key: &Tuple) -> bool {
        self.hash == key.0 && self.is_factory == key.1 && self.value == key.2
    }
}

/// Why a fold stopped being usable. The names are the published reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Invalidation {
    DecodeFailure,
    MalformedIdentity,
    InboundLoss,
    RetentionLimit,
    PublicationLimit,
}

impl Invalidation {
    fn name(self) -> &'static str {
        match self {
            Self::DecodeFailure => "decode-failure",
            Self::MalformedIdentity => "malformed-identity",
            Self::InboundLoss => "inbound-loss",
            Self::RetentionLimit => "retention-limit",
            Self::PublicationLimit => "publication-limit",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    /// No message has yet added an entry in this session.
    #[default]
    Unavailable,
    Folding,
    /// Sticky: nothing short of a new session revives it, because the fold has
    /// no completeness marker with which to prove the list whole again.
    Invalidated(Invalidation),
}

/// A complete identity read from a message, with no defaulting of absent parts.
struct Identity {
    value: String,
    is_factory: bool,
    hash: u32,
}

/// Why a recall was not admitted. Each variant has its own code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No list was announced in this session. Code `state_unavailable`.
    Unavailable,
    /// The fold may have lost a message or outgrew a limit, and stays unusable
    /// until the session ends. Code `state_unavailable`.
    Invalidated(&'static str),
    /// The hash is not the Global EQ family. Code `wrong_family`.
    WrongFamily(u32),
    /// The list does not contain the identity: never announced, or deleted
    /// since. It does not follow that the preset does not exist. Code
    /// `not_announced`.
    NotAnnounced,
    /// The identity was announced more than once. Code `ambiguous`.
    Ambiguous,
}

impl Refusal {
    /// Stable code for tests and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable | Self::Invalidated(_) => "state_unavailable",
            Self::WrongFamily(_) => "wrong_family",
            Self::NotAnnounced => "not_announced",
            Self::Ambiguous => "ambiguous",
        }
    }

    /// The refusal text. Only the two "the list cannot answer" refusals carry
    /// the `STATE_UNAVAILABLE: ` prefix that hosts map to a typed error; the
    /// others are plain sentences, as the other planner refusals are.
    pub fn message(&self) -> String {
        match self {
            Self::Unavailable => {
                "STATE_UNAVAILABLE: No Global EQ preset list has been observed in this session"
                    .into()
            }
            Self::Invalidated(reason) => format!(
                "STATE_UNAVAILABLE: The Global EQ preset list is unusable for the rest of this \
                 session ({reason})"
            ),
            Self::WrongFamily(hash) => format!(
                "Hash {hash} is not the Global EQ preset family \
                 ({GLOBAL_EQ_MODEL_PRESET_HASH})"
            ),
            Self::NotAnnounced => "The Global EQ preset was not announced in this session, or \
                                   has since been deleted"
                .into(),
            Self::Ambiguous => "The Global EQ preset identity was announced more than once and \
                                is not recalled"
                .into(),
        }
    }
}

/// The retained fold. Held by the state decoder, so a decoder reset (every
/// session change) returns it to unavailable.
#[derive(Debug, Default)]
pub struct ModelPresetCatalog {
    phase: Phase,
    entries: Vec<Entry>,
    /// How many retained entries carry each identity. More than one makes the
    /// identity ambiguous, and an ambiguous identity is neither offered nor
    /// admitted.
    tuple_counts: HashMap<Tuple, u32>,
    marker: Option<ModelPresetCatalogMarker>,
    counters: ModelPresetCatalogCounters,
    generation: u32,
    /// Inbound loss total when this fold was created; see `seed_inbound_loss`.
    loss_baseline: Option<u64>,
}

impl ModelPresetCatalog {
    /// Folds one model preset message into the retained lists.
    ///
    /// The action decides the branch, and only the update branch looks at the
    /// loaded-preset fields:
    ///
    /// - Create appends each preset, whatever else the message carries.
    /// - Update with `loaded_preset_id` present is a loaded-preset
    ///   notification and nothing else: the active marker is recorded and
    ///   every list is left alone, even when the message also lists presets.
    ///   Presence is decoded schema presence, never a default value.
    /// - Update with another loaded field (hash, row, column, capture version)
    ///   but no `loaded_preset_id` is an unproven combination. It is counted
    ///   as ambiguous and leaves every list alone, so it can create no
    ///   admission.
    /// - Any other update appends when it lists two or more presets and, with
    ///   exactly one, is a keyed rename of an existing entry. A rename never
    ///   inserts: an absent entry is a counted no-op.
    /// - Delete with entries removes by `(hash, is_factory, value)`; a miss is
    ///   a counted no-op. Loaded fields on it change nothing.
    /// - Delete with no entries clears every list. It also clears the active
    ///   marker, which is a conservative project policy: the device is only
    ///   known to wipe the lists, and a marker that outlived them could name an
    ///   entry that no longer exists.
    /// - Read and every other action are counted and ignored.
    ///
    /// Create, delete, read and other actions that carry a loaded field still
    /// follow their own branch; the field is counted in `loadedIgnored`.
    ///
    /// An entry without a value, a hash or a factory flag invalidates the fold
    /// before any list changes; no part is defaulted into an identity. The fold
    /// becomes usable only when a message actually adds an entry.
    pub fn fold(&mut self, message: &pa::ModelPresetMessage) {
        // Counted even when invalidated, so the view still shows the traffic.
        self.counters.messages = self.counters.messages.saturating_add(1);
        self.generation = self.generation.saturating_add(1);
        if matches!(self.phase, Phase::Invalidated(_)) {
            return;
        }

        let loaded = carries_loaded_field(message);
        match Action::try_from(message.action).ok() {
            Some(Action::Create) => {
                self.note_loaded_ignored(loaded);
                self.counters.creates = self.counters.creates.saturating_add(1);
                self.append(&message.presets);
            }
            Some(Action::Update) => self.fold_update(message),
            Some(Action::Delete) => {
                self.note_loaded_ignored(loaded);
                self.delete(&message.presets);
            }
            Some(Action::Read) => {
                self.note_loaded_ignored(loaded);
                self.counters.reads_ignored = self.counters.reads_ignored.saturating_add(1);
            }
            _ => {
                self.note_loaded_ignored(loaded);
                self.counters.other_actions_ignored =
                    self.counters.other_actions_ignored.saturating_add(1);
            }
        }
        self.enforce_limits();
    }

    fn note_loaded_ignored(&mut self, loaded: bool) {
        if loaded {
            self.counters.loaded_ignored = self.counters.loaded_ignored.saturating_add(1);
        }
    }

    fn fold_update(&mut self, message: &pa::ModelPresetMessage) {
        self.counters.updates = self.counters.updates.saturating_add(1);
        if let Some(pa::model_preset_message::LoadedPresetId::LoadedPresetId(id)) =
            message.loaded_preset_id.as_ref()
        {
            self.record_marker(id);
            return;
        }
        if carries_loaded_field(message) {
            // A loaded hash, row, column or capture version without the
            // loaded preset id names no preset, and the receiver is not known
            // to treat the list part of such a message as a list.
            self.counters.ambiguous_markers = self.counters.ambiguous_markers.saturating_add(1);
            return;
        }
        match message.presets.as_slice() {
            [] => {}
            [only] => self.rename(only),
            many => self.append(many),
        }
    }

    fn record_marker(&mut self, id: &pa::ModelPresetId) {
        let incoming_bytes = id.value.as_ref().map_or(0, |value| match value {
            pa::model_preset_id::Value::Value(value) => value.len(),
        });
        let previous_bytes = self
            .marker
            .as_ref()
            .and_then(|marker| marker.id.as_ref())
            .map_or(0, String::len);
        if incoming_bytes > MAX_RETAINED_STRING_BYTES
            || !self.retained_budget_allows(previous_bytes, incoming_bytes)
        {
            self.invalidate(Invalidation::RetentionLimit);
            return;
        }
        let marker = ModelPresetCatalogMarker {
            id: id.value.as_ref().map(|value| match value {
                pa::model_preset_id::Value::Value(value) => value.clone(),
            }),
            is_factory: id.is_factory.as_ref().map(|value| match value {
                pa::model_preset_id::IsFactory::IsFactory(value) => *value,
            }),
            model_hash: id.hash.as_ref().map(|value| match value {
                pa::model_preset_id::Hash::Hash(value) => *value,
            }),
        };
        if marker.id.is_none() || marker.is_factory.is_none() || marker.model_hash.is_none() {
            // Recorded as reported, but flagged: an incomplete marker is
            // information about what is loaded, never an identity.
            self.counters.ambiguous_markers = self.counters.ambiguous_markers.saturating_add(1);
        }
        self.counters.marker_updates = self.counters.marker_updates.saturating_add(1);
        self.marker = Some(marker);
    }

    fn append(&mut self, presets: &[pa::ModelPreset]) {
        let mut additional = 0usize;
        let mut tuple_keys: HashSet<_> = self
            .tuple_counts
            .keys()
            .map(|key| (key.0, key.1, key.2.as_str()))
            .collect();
        for preset in presets {
            let Some((value, is_factory, hash)) = preset_identity_parts(preset) else {
                self.invalidate_malformed();
                return;
            };
            let name = preset_name(preset).unwrap_or_default();
            if value.len() > MAX_RETAINED_STRING_BYTES || name.len() > MAX_RETAINED_STRING_BYTES {
                self.invalidate(Invalidation::RetentionLimit);
                return;
            }
            additional = additional
                .saturating_add(value.len())
                .saturating_add(name.len());
            if tuple_keys.insert((hash, is_factory, value)) {
                additional = additional.saturating_add(value.len());
            }
        }
        if !self.retained_budget_allows(0, additional) {
            self.invalidate(Invalidation::RetentionLimit);
            return;
        }
        // Identity first, so a malformed entry leaves the lists as they were.
        let mut batch = Vec::with_capacity(presets.len());
        for preset in presets {
            let Some(identity) = preset_identity(preset) else {
                self.invalidate_malformed();
                return;
            };
            batch.push(Entry {
                hash: identity.hash,
                is_factory: identity.is_factory,
                value: identity.value,
                name: preset_name(preset).unwrap_or_default().to_owned(),
                is_default: preset_is_default(preset).unwrap_or(false),
            });
        }
        if batch.is_empty() {
            return;
        }
        // Only a message that adds an entry makes the list usable.
        if matches!(self.phase, Phase::Unavailable) {
            self.phase = Phase::Folding;
        }
        for entry in batch {
            let count = self.tuple_counts.entry(entry.tuple()).or_insert(0);
            *count += 1;
            if *count > 1 {
                self.counters.duplicate_tuples = self.counters.duplicate_tuples.saturating_add(1);
            }
            self.entries.push(entry);
        }
    }

    fn rename(&mut self, preset: &pa::ModelPreset) {
        let Some((value, is_factory, hash)) = preset_identity_parts(preset) else {
            self.invalidate_malformed();
            return;
        };
        let name = preset_name(preset);
        if value.len() > MAX_RETAINED_STRING_BYTES
            || name.is_some_and(|name| name.len() > MAX_RETAINED_STRING_BYTES)
            || !self.rename_budget_allows(value, is_factory, hash, name)
        {
            self.invalidate(Invalidation::RetentionLimit);
            return;
        }
        let Some(identity) = preset_identity(preset) else {
            self.invalidate_malformed();
            return;
        };
        let key = (identity.hash, identity.is_factory, identity.value);
        let is_default = preset_is_default(preset);
        let mut matched = false;
        for entry in self.entries.iter_mut().filter(|entry| entry.matches(&key)) {
            matched = true;
            if let Some(name) = name {
                entry.name = name.to_owned();
            }
            if let Some(is_default) = is_default {
                entry.is_default = is_default;
            }
        }
        if matched {
            self.counters.renames = self.counters.renames.saturating_add(1);
        } else {
            self.counters.rename_misses = self.counters.rename_misses.saturating_add(1);
        }
    }

    fn delete(&mut self, presets: &[pa::ModelPreset]) {
        if presets.is_empty() {
            self.entries.clear();
            self.tuple_counts.clear();
            self.marker = None;
            self.counters.wipes = self.counters.wipes.saturating_add(1);
            return;
        }
        let mut keys = Vec::with_capacity(presets.len());
        for preset in presets {
            let Some(identity) = preset_identity(preset) else {
                self.invalidate_malformed();
                return;
            };
            keys.push((identity.hash, identity.is_factory, identity.value));
        }
        let mut removed_any = false;
        for key in keys {
            if self.tuple_counts.remove(&key).is_some() {
                self.entries.retain(|entry| !entry.matches(&key));
                removed_any = true;
            } else {
                self.counters.delete_misses = self.counters.delete_misses.saturating_add(1);
            }
        }
        if removed_any {
            self.counters.deletes = self.counters.deletes.saturating_add(1);
        }
    }

    /// Invalidates instead of truncating: a list cut at a limit would look
    /// like a whole one.
    fn retained_bytes(&self) -> usize {
        self.entries
            .iter()
            .fold(0usize, |bytes, entry| {
                bytes
                    .saturating_add(entry.value.len())
                    .saturating_add(entry.name.len())
            })
            .saturating_add(
                self.tuple_counts
                    .keys()
                    .fold(0usize, |bytes, key| bytes.saturating_add(key.2.len())),
            )
            .saturating_add(
                self.marker
                    .as_ref()
                    .and_then(|marker| marker.id.as_ref())
                    .map_or(0, String::len),
            )
    }

    fn retained_budget_allows(&self, removed: usize, added: usize) -> bool {
        self.retained_bytes()
            .saturating_sub(removed)
            .saturating_add(added)
            <= MAX_RETAINED_BYTES
    }

    fn rename_budget_allows(
        &self,
        value: &str,
        is_factory: bool,
        hash: u32,
        name: Option<&str>,
    ) -> bool {
        let Some(name) = name else { return true };
        let (removed, added) = self
            .entries
            .iter()
            .filter(|entry| {
                entry.hash == hash && entry.is_factory == is_factory && entry.value == value
            })
            .fold((0usize, 0usize), |(removed, added), entry| {
                (
                    removed.saturating_add(entry.name.len()),
                    added.saturating_add(name.len()),
                )
            });
        self.retained_budget_allows(removed, added)
    }

    fn enforce_limits(&mut self) {
        let oversized = self.entries.iter().any(|entry| {
            entry.value.len() > MAX_RETAINED_STRING_BYTES
                || entry.name.len() > MAX_RETAINED_STRING_BYTES
        }) || self
            .marker
            .as_ref()
            .and_then(|marker| marker.id.as_ref())
            .is_some_and(|id| id.len() > MAX_RETAINED_STRING_BYTES);
        if oversized || self.retained_bytes() > MAX_RETAINED_BYTES {
            self.invalidate(Invalidation::RetentionLimit);
            return;
        }
        if !matches!(self.phase, Phase::Folding) {
            return;
        }
        let scoped = self
            .entries
            .iter()
            .filter(|entry| entry.hash == GLOBAL_EQ_MODEL_PRESET_HASH)
            .count();
        if self.entries.len() > RETAINED_ENTRY_LIMIT {
            self.invalidate(Invalidation::RetentionLimit);
        } else if scoped > PUBLISHED_ENTRY_LIMIT {
            self.invalidate(Invalidation::PublicationLimit);
        }
    }

    fn invalidate_malformed(&mut self) {
        self.counters.malformed_identities = self.counters.malformed_identities.saturating_add(1);
        self.invalidate(Invalidation::MalformedIdentity);
    }

    fn invalidate(&mut self, reason: Invalidation) {
        if matches!(self.phase, Phase::Invalidated(_)) {
            return;
        }
        self.phase = Phase::Invalidated(reason);
        // An invalidated projection outranks the folding one it replaces.
        self.generation = self.generation.saturating_add(1);
        self.entries = Vec::new();
        self.tuple_counts = HashMap::new();
        self.marker = None;
    }

    /// Records that a model preset message could not be decoded, or never
    /// reached the fold (size limit, inflate failure).
    pub fn note_decode_failure(&mut self) {
        self.counters.decode_failures = self.counters.decode_failures.saturating_add(1);
        self.invalidate(Invalidation::DecodeFailure);
    }

    /// Records the transport's loss total at the moment this fold is created, before any
    /// ingress can reach it. A healthy session starts from this known baseline, so losses
    /// counted earlier, on a previous connection or attempt, are not this session's.
    pub fn seed_inbound_loss(&mut self, total: u64) {
        self.loss_baseline = Some(total);
    }

    /// Feeds the transport's running loss total and invalidates the fold when it differs from
    /// the baseline. Returns `true` when this call changed the state.
    ///
    /// `total` counts every report or frame the transport dropped that could have been a model
    /// preset message: reports whose message never completed (an abandoned partial, an orphaned
    /// continuation, a framing error), whose type is unknown and is therefore not harmless, plus
    /// the failures of frames typed as model presets. Failures of frames typed as another
    /// message are not counted: the fold has no dependency on them.
    ///
    /// Any such loss invalidates the fold for the rest of the session, whatever the phase and
    /// whatever message follows. A model preset message carries no marker of completeness, so a
    /// later append can neither prove the list whole nor heal a lost message, and a lost delete
    /// would leave a stale admission for as long as the loss goes unnoticed. The cost is that an
    /// unrelated loss disables the picker until the session ends. Only losses the transport has
    /// counted are seen: a report still queued below the owner, or a message that is half
    /// received, is not a counted loss yet.
    ///
    /// There is no exemption for attach noise. If the fold was never seeded the baseline is
    /// zero, so any loss counted at all invalidates. A total that fell below the baseline means
    /// the transport restarted its tally and dropped whatever it had open, which is also a loss.
    pub fn observe_inbound_loss(&mut self, total: u64) -> bool {
        let baseline = self.loss_baseline.unwrap_or(0);
        if total == baseline {
            return false;
        }
        self.loss_baseline = Some(total);
        if matches!(self.phase, Phase::Invalidated(_)) {
            return false;
        }
        self.counters.inbound_loss = self.counters.inbound_loss.saturating_add(1);
        self.invalidate(Invalidation::InboundLoss);
        true
    }

    /// Whether the fold is invalidated.
    pub fn is_invalidated(&self) -> bool {
        matches!(self.phase, Phase::Invalidated(_))
    }

    /// Checks a recall request against the retained fold.
    ///
    /// The check reads the fold itself, never the published view, so it cannot
    /// admit from a list that was cut for publication. Admits only an identity
    /// that the fold lists exactly once, for the scope hash, while the fold is
    /// usable.
    pub fn admits(&self, value: &str, is_factory: bool, hash: u32) -> Result<(), Refusal> {
        match self.phase {
            Phase::Unavailable => return Err(Refusal::Unavailable),
            Phase::Invalidated(reason) => return Err(Refusal::Invalidated(reason.name())),
            Phase::Folding => {}
        }
        if hash != GLOBAL_EQ_MODEL_PRESET_HASH {
            return Err(Refusal::WrongFamily(hash));
        }
        let key = (hash, is_factory, value.to_owned());
        match self.tuple_counts.get(&key).copied() {
            Some(1) => Ok(()),
            Some(_) => Err(Refusal::Ambiguous),
            None => Err(Refusal::NotAnnounced),
        }
    }

    /// The published projection. `session` is left empty: only the session
    /// runtime knows the state session and stamps it. The list is never cut:
    /// while folding it holds every unique scope-hash entry, and the fold
    /// invalidates before that can exceed [`PUBLISHED_ENTRY_LIMIT`].
    pub fn view(&self) -> ModelPresetCatalogState {
        let mut counters = self.counters.clone();
        counters.retained_entries = clamp_u32(self.entries.len());
        let scoped = self
            .entries
            .iter()
            .filter(|entry| entry.hash == GLOBAL_EQ_MODEL_PRESET_HASH)
            .count();
        counters.retained_other_hash_entries = clamp_u32(self.entries.len() - scoped);
        counters.ambiguous_tuples = clamp_u32(
            self.tuple_counts
                .values()
                .filter(|count| **count > 1)
                .count(),
        );
        let (state, entries, invalid_reason) = match self.phase {
            Phase::Unavailable => ("unavailable", None, None),
            Phase::Folding => (
                "folding",
                Some(
                    self.entries
                        .iter()
                        .filter(|entry| {
                            entry.hash == GLOBAL_EQ_MODEL_PRESET_HASH
                                && self.tuple_counts.get(&entry.tuple()) == Some(&1)
                        })
                        .map(|entry| ModelPresetSummary {
                            id: entry.value.clone(),
                            name: entry.name.clone(),
                            is_factory: entry.is_factory,
                            is_default: entry.is_default,
                            model_hash: Some(entry.hash),
                        })
                        .collect(),
                ),
                None,
            ),
            Phase::Invalidated(reason) => ("invalidated", None, Some(reason.name().to_owned())),
        };
        ModelPresetCatalogState {
            state: state.into(),
            session: String::new(),
            generation: self.generation,
            scope_hash: GLOBAL_EQ_MODEL_PRESET_HASH,
            entries,
            active_marker: self.marker.clone(),
            invalid_reason,
            counters,
        }
    }
}

fn clamp_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Any `loaded_*` field is present. Decoded field presence is used, never a
/// default value.
fn carries_loaded_field(message: &pa::ModelPresetMessage) -> bool {
    message.loaded_preset_id.is_some()
        || message.loaded_hash.is_some()
        || message.loaded_row.is_some()
        || message.loaded_column.is_some()
        || message.loaded_capture_version.is_some()
}

fn preset_identity(preset: &pa::ModelPreset) -> Option<Identity> {
    let (value, is_factory, hash) = preset_identity_parts(preset)?;
    Some(Identity {
        value: value.to_owned(),
        is_factory,
        hash,
    })
}

fn preset_identity_parts(preset: &pa::ModelPreset) -> Option<(&str, bool, u32)> {
    let pa::model_preset::Id::Id(id) = preset.id.as_ref()?;
    let pa::model_preset_id::Value::Value(value) = id.value.as_ref()?;
    let pa::model_preset_id::IsFactory::IsFactory(is_factory) = id.is_factory.as_ref()?;
    let pa::model_preset_id::Hash::Hash(hash) = id.hash.as_ref()?;
    Some((value, *is_factory, *hash))
}

fn preset_name(preset: &pa::ModelPreset) -> Option<&str> {
    preset.name.as_ref().map(|name| match name {
        pa::model_preset::Name::Name(name) => name.as_str(),
    })
}

fn preset_is_default(preset: &pa::ModelPreset) -> Option<bool> {
    preset.is_default.as_ref().map(|value| match value {
        pa::model_preset::IsDefault::IsDefault(value) => *value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCOPE: u32 = GLOBAL_EQ_MODEL_PRESET_HASH;

    fn id(value: &str, is_factory: bool, hash: u32) -> pa::ModelPresetId {
        pa::ModelPresetId {
            value: Some(pa::model_preset_id::Value::Value(value.into())),
            is_factory: Some(pa::model_preset_id::IsFactory::IsFactory(is_factory)),
            hash: Some(pa::model_preset_id::Hash::Hash(hash)),
        }
    }

    fn preset(
        value: &str,
        is_factory: bool,
        hash: u32,
        name: &str,
        is_default: bool,
    ) -> pa::ModelPreset {
        pa::ModelPreset {
            id: Some(pa::model_preset::Id::Id(id(value, is_factory, hash))),
            name: Some(pa::model_preset::Name::Name(name.into())),
            is_default: Some(pa::model_preset::IsDefault::IsDefault(is_default)),
            ..Default::default()
        }
    }

    /// A message that names its target and nothing else, as a delete does.
    fn key_only(value: &str, is_factory: bool, hash: u32) -> pa::ModelPreset {
        pa::ModelPreset {
            id: Some(pa::model_preset::Id::Id(id(value, is_factory, hash))),
            ..Default::default()
        }
    }

    fn message(action: Action, presets: Vec<pa::ModelPreset>) -> pa::ModelPresetMessage {
        pa::ModelPresetMessage {
            action: action as i32,
            presets,
            ..Default::default()
        }
    }

    fn loaded(id: pa::ModelPresetId) -> pa::ModelPresetMessage {
        pa::ModelPresetMessage {
            action: Action::Update as i32,
            loaded_preset_id: Some(pa::model_preset_message::LoadedPresetId::LoadedPresetId(id)),
            ..Default::default()
        }
    }

    /// Every loaded field except the preset id, one message each.
    fn other_loaded_fields() -> Vec<pa::ModelPresetMessage> {
        use pa::model_preset_message as m;
        let base = || pa::ModelPresetMessage {
            action: Action::Update as i32,
            ..Default::default()
        };
        vec![
            pa::ModelPresetMessage {
                loaded_hash: Some(m::LoadedHash::LoadedHash(SCOPE as i32)),
                ..base()
            },
            pa::ModelPresetMessage {
                loaded_row: Some(m::LoadedRow::LoadedRow(1)),
                ..base()
            },
            pa::ModelPresetMessage {
                loaded_column: Some(m::LoadedColumn::LoadedColumn(2)),
                ..base()
            },
            pa::ModelPresetMessage {
                loaded_capture_version: Some(m::LoadedCaptureVersion::LoadedCaptureVersion(3)),
                ..base()
            },
        ]
    }

    fn names(catalog: &ModelPresetCatalog) -> Vec<(String, String, bool)> {
        catalog
            .view()
            .entries
            .unwrap_or_default()
            .into_iter()
            .map(|entry| (entry.id, entry.name, entry.is_default))
            .collect()
    }

    fn ids(catalog: &ModelPresetCatalog) -> Vec<String> {
        names(catalog).into_iter().map(|entry| entry.0).collect()
    }

    fn seeded() -> ModelPresetCatalog {
        let mut catalog = ModelPresetCatalog::default();
        catalog.fold(&message(
            Action::Update,
            vec![
                preset("a", true, SCOPE, "Flat", true),
                preset("b", false, SCOPE, "Mine", false),
                preset("x", false, 77, "Other family", false),
            ],
        ));
        catalog
    }

    fn many(count: usize, hash: u32) -> Vec<pa::ModelPreset> {
        (0..count)
            .map(|index| preset(&format!("p{index}"), false, hash, "P", false))
            .collect()
    }

    #[test]
    fn boot_update_then_create_appends_in_order() {
        let mut catalog = ModelPresetCatalog::default();
        assert_eq!(catalog.view().state, "unavailable");
        catalog.fold(&message(
            Action::Update,
            vec![
                preset("a", true, SCOPE, "Flat", true),
                preset("b", false, SCOPE, "Mine", false),
            ],
        ));
        catalog.fold(&message(
            Action::Create,
            vec![preset("c", false, SCOPE, "New", false)],
        ));
        let view = catalog.view();
        assert_eq!(view.state, "folding");
        assert_eq!(view.generation, 2);
        assert_eq!(view.scope_hash, SCOPE);
        assert_eq!(ids(&catalog), ["a", "b", "c"]);
        assert_eq!(view.counters.updates, 1);
        assert_eq!(view.counters.creates, 1);
        assert!(catalog.admits("c", false, SCOPE).is_ok());
    }

    #[test]
    fn single_update_renames_existing_entry_and_changes_default_only() {
        let mut catalog = seeded();
        // Name only: is_default is absent and must stay as it was.
        let mut rename = key_only("a", true, SCOPE);
        rename.name = Some(pa::model_preset::Name::Name("Flat v2".into()));
        catalog.fold(&message(Action::Update, vec![rename]));
        assert_eq!(names(&catalog)[0], ("a".into(), "Flat v2".into(), true));
        // Default only: the name is absent and must stay as it was.
        let mut default = key_only("a", true, SCOPE);
        default.is_default = Some(pa::model_preset::IsDefault::IsDefault(false));
        catalog.fold(&message(Action::Update, vec![default]));
        assert_eq!(names(&catalog)[0], ("a".into(), "Flat v2".into(), false));
        assert_eq!(catalog.view().counters.renames, 2);
        assert_eq!(catalog.view().counters.rename_misses, 0);
        assert_eq!(names(&catalog)[1].1, "Mine");
    }

    #[test]
    fn single_update_for_absent_tuple_is_a_counted_noop_and_never_inserts() {
        let mut catalog = seeded();
        catalog.fold(&message(
            Action::Update,
            vec![preset("ghost", false, SCOPE, "Ghost", true)],
        ));
        // Same value and hash, other factory flag: still absent.
        catalog.fold(&message(
            Action::Update,
            vec![preset("a", false, SCOPE, "Wrong flag", true)],
        ));
        let view = catalog.view();
        assert_eq!(view.state, "folding");
        assert_eq!(view.counters.rename_misses, 2);
        assert_eq!(view.entries.unwrap().len(), 2);
        assert!(catalog.admits("ghost", false, SCOPE).is_err());
    }

    #[test]
    fn loaded_marker_update_records_marker_and_leaves_lists_untouched() {
        let mut catalog = seeded();
        catalog.fold(&loaded(id("b", false, SCOPE)));
        let view = catalog.view();
        assert_eq!(view.counters.marker_updates, 1);
        assert_eq!(view.counters.ambiguous_markers, 0);
        assert_eq!(
            view.active_marker,
            Some(ModelPresetCatalogMarker {
                id: Some("b".into()),
                is_factory: Some(false),
                model_hash: Some(SCOPE),
            })
        );
        assert_eq!(ids(&catalog), ["a", "b"]);
        // Presence decides, not the length of the list: a marker update that
        // also lists presets must not append, rename or delete anything.
        let mut with_list = loaded(id("b", false, SCOPE));
        with_list.presets = vec![
            preset("new1", false, SCOPE, "N1", false),
            preset("new2", false, SCOPE, "N2", false),
        ];
        catalog.fold(&with_list);
        let mut with_one = loaded(id("b", false, SCOPE));
        with_one.presets = vec![preset("a", true, SCOPE, "Renamed", false)];
        catalog.fold(&with_one);
        assert_eq!(
            names(&catalog),
            [
                ("a".into(), "Flat".into(), true),
                ("b".into(), "Mine".into(), false)
            ]
        );
        assert_eq!(catalog.view().counters.marker_updates, 3);
        assert_eq!(catalog.view().counters.renames, 0);
        assert!(!catalog.is_invalidated());
    }

    #[test]
    fn update_with_another_loaded_field_only_is_counted_ambiguous_and_touches_no_list() {
        let fields = other_loaded_fields();
        // Hash, row, column and capture-version cases.
        assert_eq!(fields.len(), 4);
        for field_only in fields {
            let mut catalog = seeded();
            let mut with_list = field_only.clone();
            with_list.presets = vec![
                preset("n1", false, SCOPE, "N1", false),
                preset("n2", false, SCOPE, "N2", false),
            ];
            for candidate in [&field_only, &with_list] {
                catalog.fold(candidate);
            }
            let view = catalog.view();
            assert_eq!(view.counters.ambiguous_markers, 2, "{field_only:?}");
            assert_eq!(view.counters.marker_updates, 0);
            assert_eq!(view.active_marker, None);
            assert_eq!(ids(&catalog), ["a", "b"]);
            assert!(catalog.admits("n1", false, SCOPE).is_err());
            assert!(!catalog.is_invalidated());
        }
    }

    #[test]
    fn loaded_id_without_hash_is_recorded_as_reported_and_counted_ambiguous() {
        let mut catalog = seeded();
        let mut partial = id("b", false, SCOPE);
        partial.hash = None;
        catalog.fold(&loaded(partial));
        let view = catalog.view();
        assert_eq!(view.counters.ambiguous_markers, 1);
        assert_eq!(view.counters.marker_updates, 1);
        assert_eq!(
            view.active_marker,
            Some(ModelPresetCatalogMarker {
                id: Some("b".into()),
                is_factory: Some(false),
                model_hash: None,
            })
        );
        assert_eq!(view.entries.unwrap().len(), 2);
        assert!(!catalog.is_invalidated());
    }

    #[test]
    fn create_carrying_a_loaded_field_still_appends() {
        let fields = other_loaded_fields();
        // Hash, row, column and capture-version cases.
        assert_eq!(fields.len(), 4);
        for field_only in fields {
            let mut catalog = seeded();
            let mut create = field_only;
            create.action = Action::Create as i32;
            create.presets = vec![preset("c", false, SCOPE, "New", false)];
            catalog.fold(&create);
            let mut with_id = loaded(id("b", false, SCOPE));
            with_id.action = Action::Create as i32;
            with_id.presets = vec![preset("d", false, SCOPE, "Newer", false)];
            catalog.fold(&with_id);
            let view = catalog.view();
            assert_eq!(ids(&catalog), ["a", "b", "c", "d"]);
            assert_eq!(view.counters.creates, 2);
            assert_eq!(view.counters.loaded_ignored, 2);
            assert_eq!(view.counters.marker_updates, 0);
            assert_eq!(view.active_marker, None);
            assert!(catalog.admits("c", false, SCOPE).is_ok());
        }
    }

    #[test]
    fn delete_carrying_a_loaded_field_still_removes_the_entry() {
        let fields = other_loaded_fields();
        // Hash, row, column and capture-version cases.
        assert_eq!(fields.len(), 4);
        for field_only in fields {
            let mut catalog = seeded();
            assert!(catalog.admits("a", true, SCOPE).is_ok());
            let mut delete = field_only;
            delete.action = Action::Delete as i32;
            delete.presets = vec![key_only("a", true, SCOPE)];
            catalog.fold(&delete);
            assert_eq!(ids(&catalog), ["b"]);
            assert_eq!(catalog.view().counters.deletes, 1);
            assert_eq!(catalog.view().counters.loaded_ignored, 1);
            assert_eq!(
                catalog.admits("a", true, SCOPE).unwrap_err().code(),
                "not_announced"
            );
        }
        // The loaded preset id changes nothing for a delete either.
        let mut catalog = seeded();
        let mut delete = loaded(id("b", false, SCOPE));
        delete.action = Action::Delete as i32;
        delete.presets = vec![key_only("b", false, SCOPE)];
        catalog.fold(&delete);
        assert_eq!(ids(&catalog), ["a"]);
        assert!(catalog.admits("b", false, SCOPE).is_err());
    }

    #[test]
    fn keyed_delete_removes_exactly_the_tuple_and_counts_a_miss_when_absent() {
        let mut catalog = seeded();
        catalog.fold(&message(Action::Delete, vec![key_only("a", true, SCOPE)]));
        assert_eq!(names(&catalog), [("b".into(), "Mine".into(), false)]);
        assert!(catalog.admits("a", true, SCOPE).is_err());
        // Other factory flag, other hash: both miss.
        catalog.fold(&message(Action::Delete, vec![key_only("b", true, SCOPE)]));
        catalog.fold(&message(Action::Delete, vec![key_only("b", false, 12)]));
        let view = catalog.view();
        assert_eq!(view.counters.deletes, 1);
        assert_eq!(view.counters.delete_misses, 2);
        assert!(catalog.admits("b", false, SCOPE).is_ok());
        assert_eq!(view.counters.retained_other_hash_entries, 1);
    }

    #[test]
    fn empty_delete_wipes_every_list_and_the_marker_but_stays_folding() {
        let mut catalog = seeded();
        catalog.fold(&loaded(id("b", false, SCOPE)));
        catalog.fold(&message(Action::Delete, vec![]));
        let view = catalog.view();
        assert_eq!(view.state, "folding");
        assert_eq!(view.entries, Some(vec![]));
        assert_eq!(view.active_marker, None);
        assert_eq!(view.counters.wipes, 1);
        assert_eq!(view.counters.retained_entries, 0);
        assert!(catalog.admits("a", true, SCOPE).is_err());
        // A list announced after the wipe is usable again.
        catalog.fold(&message(
            Action::Create,
            vec![preset("z", false, SCOPE, "Z", false)],
        ));
        assert!(catalog.admits("z", false, SCOPE).is_ok());
    }

    #[test]
    fn read_and_unknown_actions_are_ignored_and_counted() {
        let mut catalog = seeded();
        catalog.fold(&message(
            Action::Read,
            vec![preset("r", false, SCOPE, "R", false)],
        ));
        // A read that carries a loaded field is still a read.
        let mut loaded_read = loaded(id("b", false, SCOPE));
        loaded_read.action = Action::Read as i32;
        catalog.fold(&loaded_read);
        catalog.fold(&message(
            Action::Move,
            vec![preset("m", false, SCOPE, "M", false)],
        ));
        catalog.fold(&message(Action::Swap, vec![]));
        let mut unknown = message(Action::Create, vec![preset("u", false, SCOPE, "U", false)]);
        unknown.action = 99;
        catalog.fold(&unknown);
        let view = catalog.view();
        assert_eq!(view.state, "folding");
        assert_eq!(view.counters.reads_ignored, 2);
        assert_eq!(view.counters.other_actions_ignored, 3);
        assert_eq!(view.counters.loaded_ignored, 1);
        assert_eq!(view.counters.ambiguous_markers, 0);
        assert_eq!(view.entries.unwrap().len(), 2);
    }

    #[test]
    fn missing_value_hash_or_is_factory_invalidates_and_never_defaults() {
        let strip: [fn(&mut pa::ModelPresetId); 3] = [
            |id| id.value = None,
            |id| id.hash = None,
            |id| id.is_factory = None,
        ];
        for (index, strip) in strip.iter().enumerate() {
            for action in [Action::Create, Action::Update, Action::Delete] {
                let mut catalog = seeded();
                let mut sparse = id("n", false, SCOPE);
                strip(&mut sparse);
                let bad = pa::ModelPreset {
                    id: Some(pa::model_preset::Id::Id(sparse)),
                    ..Default::default()
                };
                // Two entries so an update appends rather than renames.
                let presets = if action == Action::Update {
                    vec![bad, preset("k", false, SCOPE, "K", false)]
                } else {
                    vec![bad]
                };
                catalog.fold(&message(action, presets));
                let view = catalog.view();
                assert_eq!(view.state, "invalidated", "strip {index} {action:?}");
                assert_eq!(view.invalid_reason.as_deref(), Some("malformed-identity"));
                assert_eq!(view.entries, None);
                assert_eq!(view.counters.malformed_identities, 1);
                assert!(catalog.admits("a", true, SCOPE).is_err());
            }
        }
        // An entry with no id at all is the same failure.
        let mut catalog = seeded();
        catalog.fold(&message(Action::Create, vec![pa::ModelPreset::default()]));
        assert_eq!(catalog.view().state, "invalidated");
    }

    #[test]
    fn a_malformed_entry_leaves_earlier_entries_of_its_message_unapplied() {
        let mut catalog = seeded();
        let mut bad = id("n", false, SCOPE);
        bad.hash = None;
        catalog.fold(&message(
            Action::Create,
            vec![
                preset("fine", false, SCOPE, "Fine", false),
                pa::ModelPreset {
                    id: Some(pa::model_preset::Id::Id(bad)),
                    ..Default::default()
                },
            ],
        ));
        let view = catalog.view();
        assert_eq!(view.state, "invalidated");
        assert_eq!(view.counters.retained_entries, 0);
    }

    #[test]
    fn a_first_message_that_adds_no_entry_does_not_promote_the_fold() {
        let mut empty_update = ModelPresetCatalog::default();
        empty_update.fold(&message(Action::Update, vec![]));
        let mut empty_create = ModelPresetCatalog::default();
        empty_create.fold(&message(Action::Create, vec![]));
        let mut rename_miss = ModelPresetCatalog::default();
        rename_miss.fold(&message(
            Action::Update,
            vec![preset("a", true, SCOPE, "A", true)],
        ));
        let mut delete_miss = ModelPresetCatalog::default();
        delete_miss.fold(&message(Action::Delete, vec![key_only("a", true, SCOPE)]));
        let mut wipe = ModelPresetCatalog::default();
        wipe.fold(&message(Action::Delete, vec![]));
        let mut marker = ModelPresetCatalog::default();
        marker.fold(&loaded(id("a", true, SCOPE)));
        let mut read = ModelPresetCatalog::default();
        read.fold(&message(Action::Read, vec![]));
        for catalog in [
            &empty_update,
            &empty_create,
            &rename_miss,
            &delete_miss,
            &wipe,
            &marker,
            &read,
        ] {
            let view = catalog.view();
            assert_eq!(view.state, "unavailable");
            assert_eq!(view.entries, None);
            assert_eq!(view.counters.messages, 1);
            assert_eq!(
                catalog.admits("a", true, SCOPE).unwrap_err().code(),
                "state_unavailable"
            );
        }
        // The first message that adds an entry promotes: one create entry, or
        // an update with two.
        let mut one_create = ModelPresetCatalog::default();
        one_create.fold(&message(
            Action::Create,
            vec![preset("a", true, SCOPE, "A", true)],
        ));
        assert_eq!(one_create.view().state, "folding");
        let mut two_update = ModelPresetCatalog::default();
        two_update.fold(&message(
            Action::Update,
            vec![
                preset("a", true, SCOPE, "A", true),
                preset("b", true, SCOPE, "B", false),
            ],
        ));
        assert_eq!(two_update.view().state, "folding");
    }

    #[test]
    fn invalidated_is_sticky_until_a_new_fold_starts() {
        let mut catalog = seeded();
        let folding_generation = catalog.view().generation;
        catalog.note_decode_failure();
        assert_eq!(catalog.view().state, "invalidated");
        assert!(catalog.view().generation > folding_generation);
        catalog.fold(&message(
            Action::Create,
            vec![preset("later", false, SCOPE, "Later", false)],
        ));
        let view = catalog.view();
        assert_eq!(view.state, "invalidated");
        assert_eq!(view.invalid_reason.as_deref(), Some("decode-failure"));
        assert_eq!(view.counters.decode_failures, 1);
        assert!(catalog.admits("later", false, SCOPE).is_err());
        // A new session replaces the whole fold.
        let catalog = ModelPresetCatalog::default();
        assert_eq!(catalog.view().state, "unavailable");
        assert_eq!(catalog.view().generation, 0);
    }

    #[test]
    fn any_loss_after_the_seeded_baseline_invalidates_without_a_grace_period() {
        let mut catalog = seeded();
        catalog.seed_inbound_loss(3);
        assert!(!catalog.observe_inbound_loss(3));
        assert!(catalog.admits("a", true, SCOPE).is_ok());
        assert!(catalog.observe_inbound_loss(4));
        let view = catalog.view();
        assert_eq!(view.state, "invalidated");
        assert_eq!(view.invalid_reason.as_deref(), Some("inbound-loss"));
        assert_eq!(view.counters.inbound_loss, 1);
        // Already invalidated: no second transition, no second count.
        assert!(!catalog.observe_inbound_loss(9));
        assert_eq!(catalog.view().counters.inbound_loss, 1);
    }

    #[test]
    fn losses_counted_before_the_fold_was_created_are_not_this_sessions() {
        let mut catalog = ModelPresetCatalog::default();
        catalog.seed_inbound_loss(7);
        assert!(!catalog.observe_inbound_loss(7));
        catalog.fold(&message(
            Action::Update,
            vec![
                preset("a", true, SCOPE, "A", true),
                preset("b", false, SCOPE, "B", false),
            ],
        ));
        assert!(catalog.admits("a", true, SCOPE).is_ok());
    }

    #[test]
    fn a_nonzero_total_at_the_first_observation_of_an_unseeded_fold_invalidates() {
        let mut catalog = ModelPresetCatalog::default();
        assert!(catalog.observe_inbound_loss(1));
        assert_eq!(catalog.view().state, "invalidated");
        assert_eq!(
            catalog.view().invalid_reason.as_deref(),
            Some("inbound-loss")
        );
        // A zero total at the first observation is a healthy start.
        let mut healthy = ModelPresetCatalog::default();
        assert!(!healthy.observe_inbound_loss(0));
        assert_eq!(healthy.view().state, "unavailable");
    }

    #[test]
    fn a_loss_before_any_list_message_leaves_a_later_create_unusable() {
        // The boot update may be the very frame that was lost, and a later append cannot
        // prove the list whole.
        let mut catalog = ModelPresetCatalog::default();
        catalog.seed_inbound_loss(0);
        assert!(catalog.observe_inbound_loss(1));
        assert_eq!(catalog.view().state, "invalidated");
        catalog.fold(&message(
            Action::Create,
            vec![preset("a", true, SCOPE, "A", true)],
        ));
        catalog.fold(&message(
            Action::Update,
            vec![
                preset("b", true, SCOPE, "B", true),
                preset("c", true, SCOPE, "C", true),
            ],
        ));
        let view = catalog.view();
        assert_eq!(view.state, "invalidated");
        assert_eq!(view.entries, None);
        for value in ["a", "b", "c"] {
            assert_eq!(
                catalog.admits(value, true, SCOPE).unwrap_err().code(),
                "state_unavailable"
            );
        }
    }

    #[test]
    fn a_transport_tally_that_falls_is_a_restart_and_a_loss() {
        let mut catalog = seeded();
        catalog.seed_inbound_loss(5);
        assert!(catalog.observe_inbound_loss(0));
        assert_eq!(catalog.view().state, "invalidated");
    }

    #[test]
    fn duplicate_tuple_is_counted_kept_and_excluded_from_admission() {
        let mut catalog = seeded();
        catalog.fold(&message(
            Action::Create,
            vec![preset("a", true, SCOPE, "Flat again", false)],
        ));
        let view = catalog.view();
        assert_eq!(view.state, "folding");
        assert_eq!(view.counters.duplicate_tuples, 1);
        assert_eq!(view.counters.ambiguous_tuples, 1);
        assert_eq!(view.counters.retained_entries, 4);
        // Neither copy is offered, and neither is admitted; the rest is.
        assert_eq!(names(&catalog), [("b".into(), "Mine".into(), false)]);
        assert_eq!(
            catalog.admits("a", true, SCOPE).unwrap_err().code(),
            "ambiguous"
        );
        assert!(catalog.admits("b", false, SCOPE).is_ok());
        // Deleting the identity removes both copies and ends the ambiguity.
        catalog.fold(&message(Action::Delete, vec![key_only("a", true, SCOPE)]));
        assert_eq!(catalog.view().counters.ambiguous_tuples, 0);
        assert!(catalog.admits("a", true, SCOPE).is_err());
    }

    #[test]
    fn view_publishes_only_scope_hash_entries_and_counts_the_rest() {
        let catalog = seeded();
        let view = catalog.view();
        let entries = view.entries.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| entry.model_hash == Some(SCOPE)));
        assert_eq!(view.counters.retained_entries, 3);
        assert_eq!(view.counters.retained_other_hash_entries, 1);
        assert_eq!(view.session, "");
    }

    #[test]
    fn every_refusal_has_its_own_code_and_only_unavailable_ones_use_the_typed_prefix() {
        let mut catalog = ModelPresetCatalog::default();
        let unavailable = catalog.admits("a", true, SCOPE).unwrap_err();
        assert_eq!(unavailable, Refusal::Unavailable);
        catalog.fold(&message(
            Action::Update,
            vec![
                preset("a", true, SCOPE, "Flat", true),
                preset("x", false, 77, "Other", false),
                preset("d", false, SCOPE, "D1", false),
                preset("d", false, SCOPE, "D2", false),
            ],
        ));
        assert!(catalog.admits("a", true, SCOPE).is_ok());
        let wrong_family = catalog.admits("x", false, 77).unwrap_err();
        let not_announced = catalog.admits("nope", true, SCOPE).unwrap_err();
        let flag = catalog.admits("a", false, SCOPE).unwrap_err();
        let ambiguous = catalog.admits("d", false, SCOPE).unwrap_err();
        assert_eq!(wrong_family.code(), "wrong_family");
        assert_eq!(not_announced, Refusal::NotAnnounced);
        assert_eq!(flag, Refusal::NotAnnounced);
        assert_eq!(ambiguous.code(), "ambiguous");
        catalog.note_decode_failure();
        let invalidated = catalog.admits("a", true, SCOPE).unwrap_err();
        assert_eq!(invalidated, Refusal::Invalidated("decode-failure"));
        for refusal in [&unavailable, &invalidated] {
            assert_eq!(refusal.code(), "state_unavailable");
            assert!(refusal.message().starts_with("STATE_UNAVAILABLE: "));
        }
        for refusal in [&wrong_family, &not_announced, &ambiguous] {
            assert!(!refusal.message().starts_with("STATE_UNAVAILABLE"));
        }
        let codes = [
            unavailable.code(),
            wrong_family.code(),
            not_announced.code(),
            ambiguous.code(),
        ];
        assert_eq!(
            codes.iter().collect::<std::collections::HashSet<_>>().len(),
            4
        );
    }

    #[test]
    fn the_publication_boundary_accepts_exactly_the_limit_and_invalidates_one_more() {
        for (count, expected) in [
            (PUBLISHED_ENTRY_LIMIT - 1, "folding"),
            (PUBLISHED_ENTRY_LIMIT, "folding"),
            (PUBLISHED_ENTRY_LIMIT + 1, "invalidated"),
        ] {
            let mut catalog = ModelPresetCatalog::default();
            catalog.fold(&message(Action::Update, many(count, SCOPE)));
            let view = catalog.view();
            assert_eq!(view.state, expected, "{count} entries");
            if expected == "folding" {
                assert_eq!(view.entries.unwrap().len(), count);
                assert!(catalog.admits("p0", false, SCOPE).is_ok());
            } else {
                assert_eq!(view.invalid_reason.as_deref(), Some("publication-limit"));
            }
        }
    }

    #[test]
    fn growing_past_the_publication_limit_one_create_at_a_time_invalidates_stickily() {
        let mut catalog = ModelPresetCatalog::default();
        catalog.fold(&message(Action::Update, many(PUBLISHED_ENTRY_LIMIT, SCOPE)));
        assert_eq!(catalog.view().state, "folding");
        catalog.fold(&message(
            Action::Create,
            vec![preset("one-more", false, SCOPE, "More", false)],
        ));
        let view = catalog.view();
        assert_eq!(view.state, "invalidated");
        assert_eq!(view.invalid_reason.as_deref(), Some("publication-limit"));
        // Nothing is published, so nothing can read as a short or empty list,
        // and the retained fold admits nothing: not a listed entry, not the one
        // that tipped it over.
        assert_eq!(view.entries, None);
        for value in ["p0", "p255", "one-more"] {
            assert_eq!(
                catalog.admits(value, false, SCOPE).unwrap_err().code(),
                "state_unavailable"
            );
        }
        // Deleting entries does not bring it back.
        catalog.fold(&message(Action::Delete, vec![]));
        assert_eq!(catalog.view().state, "invalidated");
        assert_eq!(catalog.view().entries, None);
    }

    #[test]
    fn entries_of_other_families_do_not_count_toward_the_publication_limit() {
        let mut catalog = ModelPresetCatalog::default();
        let mut presets = many(PUBLISHED_ENTRY_LIMIT, SCOPE);
        presets.extend(many(2_000, 77));
        catalog.fold(&message(Action::Update, presets));
        let view = catalog.view();
        assert_eq!(view.state, "folding");
        assert_eq!(view.entries.unwrap().len(), PUBLISHED_ENTRY_LIMIT);
        assert_eq!(view.counters.retained_other_hash_entries, 2_000);
    }

    #[test]
    fn duplicate_tuple_rename_checks_prospective_bytes_before_copying_names() {
        let mut catalog = ModelPresetCatalog::default();
        catalog.fold(&message(
            Action::Create,
            vec![preset("same", false, 77, "", false); RETAINED_ENTRY_LIMIT],
        ));
        assert_eq!(catalog.view().state, "folding");
        let name = "x".repeat(MAX_RETAINED_STRING_BYTES);
        assert!(!catalog.rename_budget_allows("same", false, 77, Some(&name)));
        assert!(catalog.entries.iter().all(|entry| entry.name.is_empty()));
        assert!(catalog.rename_budget_allows("same", false, 77, Some("A")));
        catalog.fold(&message(
            Action::Update,
            vec![preset("same", false, 77, &name, false)],
        ));
        assert_eq!(
            catalog.view().invalid_reason.as_deref(),
            Some("retention-limit")
        );
        assert_eq!(catalog.retained_bytes(), 0);
        assert_eq!(catalog.view().counters.renames, 0);
    }

    #[test]
    fn oversized_identity_name_and_marker_invalidate_without_retaining_strings() {
        let too_long = "x".repeat(MAX_RETAINED_STRING_BYTES + 1);
        for entry in [
            preset(&too_long, false, SCOPE, "Normal", false),
            preset("normal", false, SCOPE, &too_long, false),
        ] {
            let mut catalog = seeded();
            catalog.fold(&message(Action::Create, vec![entry]));
            assert_eq!(
                catalog.view().invalid_reason.as_deref(),
                Some("retention-limit")
            );
            assert_eq!(catalog.retained_bytes(), 0);
            assert!(catalog.admits("b", false, SCOPE).is_err());
        }
        let mut catalog = ModelPresetCatalog::default();
        catalog.fold(&loaded(id(&too_long, false, SCOPE)));
        assert_eq!(
            catalog.view().invalid_reason.as_deref(),
            Some("retention-limit")
        );
        assert!(catalog.view().active_marker.is_none());
        assert_eq!(catalog.retained_bytes(), 0);
    }

    #[test]
    fn individually_bounded_creates_cannot_exhaust_the_session_byte_budget() {
        let mut catalog = ModelPresetCatalog::default();
        let name = "x".repeat(MAX_RETAINED_STRING_BYTES);
        for index in 0..4096 {
            catalog.fold(&message(
                Action::Create,
                vec![preset(&format!("id-{index}"), false, 77, &name, false)],
            ));
            if catalog.is_invalidated() {
                assert!(index < RETAINED_ENTRY_LIMIT);
                assert_eq!(catalog.retained_bytes(), 0);
                assert_eq!(
                    catalog.view().invalid_reason.as_deref(),
                    Some("retention-limit")
                );
                catalog.fold(&message(Action::Delete, vec![]));
                assert!(catalog.is_invalidated());
                return;
            }
            assert!(catalog.retained_bytes() <= MAX_RETAINED_BYTES);
        }
        panic!("repeated valid frames did not hit the retained-byte bound");
    }

    #[test]
    fn rename_delete_and_marker_replacement_account_for_current_strings_only() {
        let mut catalog = ModelPresetCatalog::default();
        let long = "x".repeat(MAX_RETAINED_STRING_BYTES);
        catalog.fold(&message(
            Action::Create,
            vec![preset("a", false, SCOPE, &long, false)],
        ));
        assert_eq!(catalog.retained_bytes(), long.len() + 2);
        catalog.fold(&message(
            Action::Update,
            vec![preset("a", false, SCOPE, "A", false)],
        ));
        assert_eq!(catalog.retained_bytes(), 3);
        catalog.fold(&loaded(id(&long, false, SCOPE)));
        assert_eq!(catalog.retained_bytes(), long.len() + 3);
        catalog.fold(&loaded(id("a", false, SCOPE)));
        assert_eq!(catalog.retained_bytes(), 4);
        catalog.fold(&message(Action::Delete, vec![key_only("a", false, SCOPE)]));
        assert_eq!(catalog.retained_bytes(), 1);
        catalog.fold(&message(Action::Delete, vec![]));
        assert_eq!(catalog.retained_bytes(), 0);
        assert_eq!(catalog.view().state, "folding");
        catalog.fold(&message(
            Action::Create,
            vec![preset("ok", false, SCOPE, "Normal", false)],
        ));
        assert!(catalog.admits("ok", false, SCOPE).is_ok());
    }
    #[test]
    fn the_retention_boundary_accepts_exactly_the_limit_and_invalidates_one_more() {
        for (count, expected) in [
            (RETAINED_ENTRY_LIMIT, "folding"),
            (RETAINED_ENTRY_LIMIT + 1, "invalidated"),
        ] {
            let mut catalog = ModelPresetCatalog::default();
            catalog.fold(&message(Action::Update, many(count, 77)));
            let view = catalog.view();
            assert_eq!(view.state, expected, "{count} entries");
            if expected == "invalidated" {
                assert_eq!(view.invalid_reason.as_deref(), Some("retention-limit"));
                assert_eq!(view.entries, None);
            }
        }
    }
}
