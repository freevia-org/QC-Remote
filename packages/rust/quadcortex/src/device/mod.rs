//! Platform-neutral QC domain state used by both desktop and mobile hosts.
//! USB/HID ownership and host UI IPC remain in their platform adapters.

use crate::state::{
    FootswitchState, GridBlock, GridRoute, IoPortState, MidiOutMessage, MidiOutSource, ModeSlot,
    PresetFolderListing, StateUpdate,
};
use serde::Serialize;
use std::collections::HashMap;

pub mod backup;
pub mod catalog;
pub mod correlation;
pub mod delivery;
/// Gateway API version, capability and method lists, and result and parameter
/// validation for the public gateway.
///
/// Generated from the project-maintained contracts, documentation included.
pub mod generated_gateway;
pub mod initialization;
pub mod native_catalog;
pub mod native_library;
pub mod operands;
pub mod owner;
pub mod preset_catalog;
pub mod preset_save;
pub mod preset_workflow;
pub mod publication;
pub mod read;
pub mod request;
pub mod session;
pub mod state_runtime;
pub mod touch_guard;
pub mod transport;

/// Whether the broker serves a gateway method without its request-wide lock.
///
/// Once served, each of these copies out state the device workers already
/// cached. That does not make all four free of USB: only `system.status` also
/// skips the broker's idle-release check. A participant's `device.stateEvents`,
/// `device.tempoClock` or `device.masterVolume` is device demand, so after the
/// broker released an idle device it starts or joins a USB reopen and waits
/// for it. See [`answers_beside_running_request`] for the narrower list that
/// may travel beside a running request.
pub fn runs_without_request_lock(method: &str) -> bool {
    matches!(
        generated_gateway::broker_dispatch(method),
        Some(
            generated_gateway::BrokerDispatch::SystemStatus
                | generated_gateway::BrokerDispatch::StateEvents
                | generated_gateway::BrokerDispatch::TempoClock
                | generated_gateway::BrokerDispatch::MasterVolume
        )
    )
}

/// Whether a method may be sent, and answered, while another request on the
/// same broker connection is still running: `system.status` alone.
///
/// It is the one method that neither takes the broker's request lock nor
/// counts as device demand, so answering it can never open USB, wait for a
/// reopen, or wait for the running request. The broker answers it straight
/// from its connection reader; hosts send it beside a busy gateway. The other
/// lock-free reads in [`runs_without_request_lock`] can wait on a USB reopen
/// that itself queues behind the running request, and stay in line.
pub fn answers_beside_running_request(method: &str) -> bool {
    matches!(
        generated_gateway::broker_dispatch(method),
        Some(generated_gateway::BrokerDispatch::SystemStatus)
    )
}

/// One occupied or empty preset slot of a setlist, as projected for hosts and clients.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetEntry {
    /// Zero-based slot index within the setlist, in `0..256`.
    pub position: u32,
    /// Slot label shown on the device: a bank number followed by a letter `A` to `H`, for example
    /// `3B` for position 17.
    pub location: String,
    /// Preset name. In a setlist listing an empty slot is named `Unsaved`.
    pub name: String,
    /// Raw instrument value of the record; zero when the device omitted it (see
    /// `instrument_present`).
    ///
    /// Hardware listings have been observed to report `1` for items tagged as
    /// guitar and `2` for items tagged as bass; `0` means untagged or absent.
    /// Values `3`, `4` and `5` are believed to be Synth, Vocal and Other, the
    /// order in which the unit's capture-save screen offers Guitar, Bass, Synth,
    /// Vocal and Other as the preferred instrument (CorOS 4.1.0). Values `4` and
    /// `5` have been seen rarely in full library listings (`4` on four factory
    /// presets, three of them named for vocal use, and one capture; `5` on one
    /// capture), which fits that reading; `3` has not been seen. The names for
    /// `3` to `5` have not been verified against the unit used for this
    /// project's tests.
    pub instrument: i32,
    /// Whether the device supplied an instrument value for this record.
    pub instrument_present: bool,
    /// Catalog key identifying the stored preset; `None` when the record has none.
    pub product_key: Option<String>,
    /// Author name published by the device, if any.
    pub author: Option<String>,
    /// Original device date text, retained when the device publishes no epoch value.
    pub date: Option<String>,
    /// Device publication timestamp in milliseconds since the Unix epoch, if published.
    pub date_ms_since_epoch: Option<i64>,
}

/// A preset setlist folder known to the device.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetFolder {
    /// Device folder key, a path-like string.
    pub key: String,
    /// Display name of the folder.
    pub name: String,
    /// Whether the folder is factory content, such as the factory library.
    pub is_factory: bool,
    /// Position in which the device first published the folder, when it was observed. Orders
    /// user folders the way the unit lists them; absent for projections that never saw arrival
    /// order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
}

/// The 256 preset slots of one setlist together with the known setlist folders.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetList {
    /// Device folder key of the setlist.
    pub setlist_key: String,
    /// Display name of the setlist.
    pub setlist_name: String,
    /// Zero-based position of the active preset when the active setlist is this one, otherwise
    /// `-1`.
    pub current_position: i32,
    /// One entry for each of the 256 positions, in position order.
    pub presets: Vec<PresetEntry>,
    /// Known preset setlist folders, factory library first, then user folders in the device's order when observed, else by name.
    pub folders: Vec<PresetFolder>,
}

/// One destination slot of the active setlist, as listed for saving a preset.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetSlot {
    /// Zero-based slot index within the setlist, in `0..256`.
    pub position: u32,
    /// Slot label shown on the device: a bank number followed by a letter `A` to `H`.
    pub location: String,
    /// Name stored in the slot; empty when the slot has no name.
    pub name: String,
    /// Whether the slot holds a preset, judged by its catalog key or a non-empty name.
    pub occupied: bool,
    /// Raw instrument value of the slot record; zero when absent.
    ///
    /// Hardware listings have been observed to report `1` for items tagged as
    /// guitar and `2` for items tagged as bass; `0` means untagged or absent.
    /// Values `3`, `4` and `5` are believed to be Synth, Vocal and Other, the
    /// order in which the unit's capture-save screen offers Guitar, Bass, Synth,
    /// Vocal and Other as the preferred instrument (CorOS 4.1.0). Values `4` and
    /// `5` have been seen rarely in full library listings (`4` on four factory
    /// presets, three of them named for vocal use, and one capture; `5` on one
    /// capture), which fits that reading; `3` has not been seen. The names for
    /// `3` to `5` have not been verified against the unit used for this
    /// project's tests.
    pub instrument: i32,
}

/// All 256 slots of the active setlist.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetSlotList {
    /// Device folder key of the setlist.
    pub setlist_key: String,
    /// Display name of the setlist.
    pub setlist_name: String,
    /// Zero-based position of the active preset.
    pub current_position: u32,
    /// One entry for each of the 256 positions, in position order.
    pub slots: Vec<PresetSlot>,
}

/// Preset folder listings retained from the device, keyed by folder key (a trailing `/` is
/// ignored).
///
/// Each listing is kept twice: a primary set that backs [`PresetLibrary::list`],
/// [`PresetLibrary::slots`], [`PresetLibrary::entry`] and the folder list, and an observed set that
/// backs the `observed_*` projections used to verify what the device reported.
#[derive(Debug, Default)]
pub struct PresetLibrary {
    listings: HashMap<String, PresetFolderListing>,
    observed_listings: HashMap<String, PresetFolderListing>,
}

fn slot_name(position: u32) -> String {
    format!(
        "{}{}",
        position / 8 + 1,
        (b'A' + (position % 8) as u8) as char
    )
}

impl PresetLibrary {
    #[cfg(test)]
    pub(crate) fn observed_listing(&self, key: &str) -> Option<&PresetFolderListing> {
        self.observed_listings.get(key.trim_end_matches('/'))
    }

    /// Discards every retained listing.
    pub fn clear(&mut self) {
        self.listings.clear();
        self.observed_listings.clear();
    }

    /// Stores `listing` in both sets under its key, replacing any earlier listing for that key,
    /// with its files sorted by position.
    ///
    /// The listing is installed as given; File messages from the device should be reduced by the
    /// native preset catalog first.
    pub fn ingest(&mut self, mut listing: PresetFolderListing) {
        let key = listing.key.trim_end_matches('/').to_string();
        self.observed_listings.insert(key.clone(), listing.clone());
        listing.files.sort_by_key(|file| file.position);
        // Install an already materialized listing. Native File messages must
        // pass through NativePresetCatalog's action-aware reduction first.
        self.listings.insert(key, listing);
    }

    /// Registers an empty setlist folder for `key` in the primary set if none is retained. An
    /// existing listing is left untouched.
    pub fn ensure_setlist(&mut self, key: &str) {
        let key = key.trim_end_matches('/').to_string();
        self.listings
            .entry(key.clone())
            .or_insert_with(|| PresetFolderListing {
                order: None,
                name: key.rsplit('/').next().unwrap_or_default().to_string(),
                key,
                file_type: Some(0),
                is_factory: false,
                is_plugin: false,
                files: Vec::new(),
            });
    }

    /// Returns the setlist folders of the primary set: the factory library first, then the rest
    /// in the order the device published them when that order was observed, else by name,
    /// ignoring case.
    pub fn folders(&self) -> Vec<PresetFolder> {
        Self::project_folders(self.listings.values())
    }

    /// Like [`PresetLibrary::folders`], over the observed set.
    pub fn observed_folders(&self) -> Vec<PresetFolder> {
        Self::project_folders(self.observed_listings.values())
    }

    fn project_folders<'a>(
        listings: impl Iterator<Item = &'a PresetFolderListing>,
    ) -> Vec<PresetFolder> {
        let mut folders = listings
            .filter(|folder| Self::is_preset_setlist_listing(folder))
            .map(|folder| PresetFolder {
                key: folder.key.clone(),
                name: folder.name.clone(),
                is_factory: folder.is_factory
                    || folder.key.trim_end_matches('/') == "/opt/neuraldsp/Factory Library",
                order: folder.order,
            })
            .collect::<Vec<_>>();
        // Factory first. User folders follow the device's own order when it was observed, then
        // (and for folders without one) by name, ignoring case.
        folders.sort_by(|left, right| {
            right
                .is_factory
                .cmp(&left.is_factory)
                .then_with(|| left.order.is_none().cmp(&right.order.is_none()))
                .then_with(|| left.order.cmp(&right.order))
                .then_with(|| {
                    left.name
                        .to_ascii_lowercase()
                        .cmp(&right.name.to_ascii_lowercase())
                })
        });
        folders
    }

    /// Whether one firmware File folder belongs to the public preset-setlist
    /// catalog. The desktop routes plugin folders to a separate provider and
    /// presents the fixed factory model before its custom-folder list.
    pub fn is_preset_setlist_listing(folder: &PresetFolderListing) -> bool {
        if !matches!(folder.file_type, None | Some(0)) || folder.is_plugin {
            return false;
        }
        let key = folder.key.trim_end_matches('/');
        key == "/opt/neuraldsp/Factory Library"
            || key
                .rsplit_once('/')
                .is_some_and(|(parent, child)| parent == "/media/p4/Presets" && !child.is_empty())
    }

    /// Projects the setlist `setlist_key` into a [`PresetList`] covering all 256 positions, using
    /// `active` to fill in the current position.
    ///
    /// Returns `None` when no listing is retained for the key.
    pub fn list(&self, setlist_key: &str, active: &GatewaySnapshot) -> Option<PresetList> {
        let listing = self.listings.get(setlist_key.trim_end_matches('/'))?;
        Some(Self::project_list(listing, active, self.folders()))
    }

    /// Device observations used for verification, before display overrides.
    pub fn observed_list(&self, setlist_key: &str, active: &GatewaySnapshot) -> Option<PresetList> {
        let listing = self
            .observed_listings
            .get(setlist_key.trim_end_matches('/'))?;
        Some(Self::project_list(listing, active, self.observed_folders()))
    }

    pub(crate) fn project_list(
        listing: &PresetFolderListing,
        active: &GatewaySnapshot,
        folders: Vec<PresetFolder>,
    ) -> PresetList {
        let by_position = listing
            .files
            .iter()
            .map(|file| (file.position, file))
            .collect::<HashMap<_, _>>();
        let presets = (0..256)
            .map(|position| {
                let file = by_position.get(&position);
                PresetEntry {
                    position,
                    location: slot_name(position),
                    name: file
                        .filter(|file| !file.name.is_empty())
                        .map(|file| file.name.clone())
                        .unwrap_or_else(|| "Unsaved".into()),
                    instrument: file.map(|file| file.instrument).unwrap_or(0),
                    instrument_present: file.is_some_and(|file| file.instrument_present),
                    product_key: file.and_then(|file| file.product_key.clone()),
                    author: file.and_then(|file| file.author.clone()),
                    date: file.and_then(|file| file.date.clone()),
                    date_ms_since_epoch: file.and_then(|file| file.date_ms_since_epoch),
                }
            })
            .collect();
        PresetList {
            setlist_key: listing.key.clone(),
            setlist_name: listing.name.clone(),
            current_position: if active.setlist_key.trim_end_matches('/')
                == listing.key.trim_end_matches('/')
            {
                active.preset_position as i32
            } else {
                -1
            },
            presets,
            folders,
        }
    }

    /// Lists the slots of the setlist that `active` belongs to.
    ///
    /// Returns `None` when that setlist is not retained or its listing does not cover all 256
    /// positions.
    pub fn slots(&self, active: &GatewaySnapshot) -> Option<PresetSlotList> {
        let listing = self
            .listings
            .get(active.setlist_key.trim_end_matches('/'))?;
        Self::project_slots(listing, active)
    }

    /// Like [`PresetLibrary::slots`], over the observed set.
    pub fn observed_slots(&self, active: &GatewaySnapshot) -> Option<PresetSlotList> {
        let listing = self
            .observed_listings
            .get(active.setlist_key.trim_end_matches('/'))?;
        Self::project_slots(listing, active)
    }

    fn project_slots(
        listing: &PresetFolderListing,
        active: &GatewaySnapshot,
    ) -> Option<PresetSlotList> {
        let by_position = listing
            .files
            .iter()
            .map(|file| (file.position, file))
            .collect::<HashMap<_, _>>();
        // The public destination list has no unknown state. Do not manufacture
        // writable empty slots from omitted records in a partial publication.
        if by_position.len() != 256 || !(0..256).all(|position| by_position.contains_key(&position))
        {
            return None;
        }
        Some(PresetSlotList {
            setlist_key: listing.key.clone(),
            setlist_name: listing.name.clone(),
            current_position: active.preset_position,
            slots: (0..256)
                .map(|position| {
                    let file = by_position.get(&position);
                    PresetSlot {
                        position,
                        location: slot_name(position),
                        // Same occupancy rule as `entry()`: both call
                        // `PresetFileListing::is_occupied()` so the two
                        // lookups cannot drift apart again. See that
                        // method's doc comment for why either signal alone
                        // is insufficient. These two lookups must agree, or
                        // a client sees `device.listPresetSlots` report a
                        // slot as free while `savePresetAs`'s overwrite
                        // guard (backed by `entry()`) treats the very same
                        // named-but-keyless slot as occupied and demands
                        // confirmOverwrite.
                        occupied: file.is_some_and(|file| file.is_occupied()),
                        name: file.map(|file| file.name.clone()).unwrap_or_default(),
                        instrument: file.map(|file| file.instrument).unwrap_or(0),
                    }
                })
                .collect(),
        })
    }

    /// Lists the destination slots of the active setlist for saving a preset.
    ///
    /// # Errors
    ///
    /// Returns the refusal message from [`PresetLibrary::validate_writable_slot_target`] when the
    /// active preset is not in a writable user setlist. Returns `Ok(None)` when the setlist listing
    /// is not fully retained.
    pub fn writable_slots(
        &self,
        active: &GatewaySnapshot,
    ) -> Result<Option<PresetSlotList>, String> {
        Self::validate_writable_slot_target(active)?;
        Ok(self.slots(active))
    }

    /// Gate for the slot *listing* surface (`device.listPresetSlots` and the
    /// Android `validatePresetSlotsTarget`), which reports writable Save As
    /// destinations in the active setlist. This must fail closed off a
    /// writable user setlist, so these messages must
    /// describe the refused listing. The save planner carries its own wording:
    /// borrowing "before saving" here made a read-only call tell its caller it
    /// had attempted a write.
    pub fn validate_writable_slot_target(active: &GatewaySnapshot) -> Result<(), String> {
        if active.preset_position >= 256 {
            return Err(
                "The active preset position is outside the 256-slot setlist, so its destination slots cannot be listed."
                    .into(),
            );
        }
        if active.setlist_key.starts_with("/opt/") {
            return Err(
                "The active setlist is the read-only Factory Library, which has no writable destination slots. Recall a user setlist to list its slots."
                    .into(),
            );
        }
        let key = active.setlist_key.trim_end_matches('/');
        if !key
            .rsplit_once('/')
            .is_some_and(|(parent, child)| parent == "/media/p4/Presets" && !child.is_empty())
        {
            return Err(
                "The active preset is not in a writable user setlist, so it has no destination slots to list."
                    .into(),
            );
        }
        Ok(())
    }

    /// Returns the occupied slot at `position` of the setlist `setlist_key`.
    ///
    /// Returns `None` when the setlist or slot is not retained, or when the slot is empty.
    pub fn entry(&self, setlist_key: &str, position: u32) -> Option<PresetEntry> {
        let listing = self.listings.get(setlist_key.trim_end_matches('/'))?;
        let file = listing
            .files
            .iter()
            .find(|file| file.position == position)?;
        // Occupancy uses the shared `PresetFileListing::is_occupied()`
        // predicate (see its doc comment) so this stays in lockstep with
        // `project_slots()` above -- see the `*_keyless_*` copy/rename tests
        // below for why either signal alone is insufficient.
        file.is_occupied().then(|| PresetEntry {
            position,
            location: slot_name(position),
            name: file.name.clone(),
            instrument: file.instrument,
            instrument_present: file.instrument_present,
            product_key: file.product_key.clone(),
            author: file.author.clone(),
            date: file.date.clone(),
            date_ms_since_epoch: file.date_ms_since_epoch,
        })
    }

    /// Return the raw ProductData records from one authoritative folder frame.
    /// CorOS may include keyless placeholder records for empty slots; callers
    /// that need occupied presets must test `product_key` explicitly.
    pub fn preset_files(&self, setlist_key: &str) -> Option<&[crate::state::PresetFileListing]> {
        self.listings
            .get(setlist_key.trim_end_matches('/'))
            .map(|listing| listing.files.as_slice())
    }

    /// Returns the catalog key of the preset at `position` of the setlist `setlist_key`, or `None`
    /// when the setlist, the slot or its key is absent.
    pub fn entry_product_key(&self, setlist_key: &str, position: u32) -> Option<String> {
        self.listings
            .get(setlist_key.trim_end_matches('/'))?
            .files
            .iter()
            .find(|file| file.position == position)?
            .product_key
            .clone()
    }

    /// Test-only presentation edit: it must never become native evidence or
    /// survive a subsequent materialized native catalog update.
    #[cfg(test)]
    pub(crate) fn edit_cached_preset(
        &mut self,
        key: &str,
        position: u32,
        name: &str,
        instrument: i32,
    ) {
        let Some(folder) = self.listings.get_mut(key.trim_end_matches('/')) else {
            return;
        };
        if let Some(file) = folder
            .files
            .iter_mut()
            .find(|file| file.position == position)
        {
            file.name = name.into();
            file.instrument = instrument;
        }
    }

    pub(crate) fn remove_folder(&mut self, key: &str) {
        let matches = |candidate: &str| {
            candidate
                .trim_end_matches('/')
                .eq_ignore_ascii_case(key.trim_end_matches('/'))
        };
        self.listings.retain(|candidate, _| !matches(candidate));
        self.observed_listings
            .retain(|candidate, _| !matches(candidate));
    }
}

/// The current device state as reduced from native state updates and served to hosts and clients.
///
/// Fields marked as skipped are bookkeeping for the session owner and are not serialized.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySnapshot {
    /// Version stamp of the native state stream this snapshot reflects, if one has been assigned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_version: Option<crate::generated_payloads::NativeStateVersion>,
    /// Opaque identity of the current preset address within this session, for use as an
    /// expected-state guard.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset_identity: Option<String>,
    /// Device name. Starts as `Quad Cortex`; [`GatewaySnapshot::apply`] does not change it.
    pub device_name: String,
    /// Name of the current preset; `Unsaved` when the preset has no name.
    pub preset_name: String,
    /// Slot label of the current preset, a bank number followed by a letter `A` to `H`.
    pub preset_location: String,
    /// Zero-based slot index of the current preset within its setlist.
    pub preset_position: u32,
    /// Device folder key of the current setlist.
    pub setlist_key: String,
    /// Display name of the current setlist, taken from the last segment of its key.
    pub setlist_name: String,
    /// Current footswitch mode: `PRESET`, `SCENE`, `STOMP` or `HYBRID`; `UNKNOWN` for a reported
    /// value outside the nine this library names. Until the device reports a mode the value is
    /// the placeholder `STOMP` and `unobserved` lists `mode`.
    pub mode: String,
    /// The unmodified wire value of the current mode, once the device has reported one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_mode: Option<u32>,
    /// The modes selectable in each mode slot (up to three), when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_slots: Option<Vec<ModeSlot>>,
    /// Two mode names (`PRESET`, `SCENE`, `STOMP` or `UNKNOWN`) reported with the mode; they differ
    /// in a hybrid mode. In a hybrid the first name is believed to apply to
    /// footswitches A-D and the second to footswitches E-H.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footswitch_modes: Option<Vec<String>>,
    /// Footswitch states derived from the current preset, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footswitch_states: Option<Vec<FootswitchState>>,
    /// MIDI output messages grouped by source, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub midi_out: Option<Vec<MidiOutSource>>,
    /// Preset-level MIDI output messages, kept apart from the per-source `midi_out` groups, when
    /// reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset_load_midi_out: Option<Vec<MidiOutMessage>>,
    /// Zero-based index of the active scene.
    pub active_scene: u32,
    /// Scene labels for display, initially `Scene A` to `Scene H`; a stored label that is empty or
    /// blank is shown as that default, which is this library's text. `scene_labels_raw` holds the
    /// labels as the device stored them.
    pub scenes: Vec<String>,
    /// The scene labels exactly as the device stored them, when a preset has reported them; empty
    /// strings where none is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_labels_raw: Option<Vec<String>>,
    /// Scene colors as `#rrggbb` strings, when reported. The alpha byte of the stored value is
    /// dropped; `scene_colors_argb` keeps it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_colors: Option<Vec<String>>,
    /// Scene colors as the device stores them, 32-bit ARGB, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_colors_argb: Option<Vec<u32>>,
    /// Blocks of the current preset grid.
    pub blocks: Vec<GridBlock>,
    /// Whether the block list has complete Grid topology. False after a sparse structural update
    /// until a complete preset publication restores it.
    pub grid_layout_complete: bool,
    /// Routes between blocks of the current preset grid.
    pub routes: Vec<GridRoute>,
    /// Input and output port states, merged by port kind and id, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io_ports: Option<Vec<IoPortState>>,
    /// Sticky software-retention failure; the I/O projection is unknown until session reset.
    pub io_settings_error: String,
    /// Preset tempo in beats per minute.
    pub tempo: u32,
    /// Whether the tempo LED is enabled.
    pub tempo_led_enabled: bool,
    /// Master volume as a whole percentage in `0..=100`.
    pub master_volume: u32,
    /// Whether the working preset has been edited since it was loaded or saved.
    pub dirty: bool,
    /// Fields of this snapshot that still hold the library's placeholder because no device message
    /// has set them in this session: any of `mode`, `tempo`, `tempoLedEnabled`, `masterVolume`,
    /// `dirty` and `presetLocation`. The placeholder values are `STOMP`, `120`, `false`, `0`,
    /// `false` and `1A`, and none of them is something a device said. Each name leaves the list when its value is
    /// observed; the list is absent once empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unobserved: Vec<&'static str>,
    /// The model preset list as the session's fold last projected it. It is what lets a client
    /// that subscribed after the list was announced, or whose event journal dropped the
    /// announcement, read the list from a snapshot. It carries the session it belongs to, never
    /// claims the list is complete, and is absent until the device has pushed the list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_preset_catalog: Option<crate::generated_payloads::ModelPresetCatalogState>,
    /// Whether a current preset has been observed in this session.
    #[serde(skip)]
    pub has_preset: bool,
    /// Counter incremented on every position update, including repeated observations of the same
    /// address.
    #[serde(skip)]
    pub position_revision: u64,
    /// Changes of address, including A-B-A, rather than repeated observations.
    #[serde(skip)]
    pub address_revision: u64,
    /// Counter incremented on every preset update, including partial projections.
    #[serde(skip)]
    pub preset_revision: u64,
    /// Full RecallPreset observations only; catalog and move projections do
    /// not establish that a newly selected sound has arrived.
    #[serde(skip)]
    pub full_preset_revision: u64,
}

impl Default for GatewaySnapshot {
    fn default() -> Self {
        Self {
            preset_identity: None,
            state_version: None,
            device_name: "Quad Cortex".into(),
            preset_name: "Unsaved".into(),
            preset_location: "1A".into(),
            preset_position: 0,
            setlist_key: String::new(),
            setlist_name: String::new(),
            mode: "STOMP".into(),
            raw_mode: None,
            mode_slots: None,
            footswitch_modes: None,
            footswitch_states: None,
            midi_out: None,
            preset_load_midi_out: None,
            active_scene: 0,
            scenes: (b'A'..=b'H')
                .map(|letter| format!("Scene {}", letter as char))
                .collect(),
            scene_labels_raw: None,
            scene_colors: None,
            scene_colors_argb: None,
            blocks: Vec::new(),
            grid_layout_complete: false,
            routes: Vec::new(),
            io_ports: None,
            io_settings_error: String::new(),
            tempo: 120,
            tempo_led_enabled: false,
            master_volume: 0,
            dirty: false,
            unobserved: vec![
                "mode",
                "tempo",
                "tempoLedEnabled",
                "masterVolume",
                "dirty",
                "presetLocation",
            ],
            model_preset_catalog: None,
            has_preset: false,
            position_revision: 0,
            address_revision: 0,
            preset_revision: 0,
            full_preset_revision: 0,
        }
    }
}

impl GatewaySnapshot {
    /// Records that a device message has set `field`, so it is no longer a placeholder.
    fn observe(&mut self, field: &'static str) {
        self.unobserved.retain(|name| *name != field);
    }

    /// Folds one state update into the snapshot according to its `kind`.
    ///
    /// Fields absent from the update leave the snapshot unchanged, and update kinds the snapshot
    /// does not track are ignored.
    pub fn apply(&mut self, state: &StateUpdate) {
        match state.kind.as_str() {
            "scene" => {
                self.active_scene = state.active_scene.unwrap_or(self.active_scene);
                // Scene changes never replace routing. Adopt only the mute
                // observation for the same witnessed route, or withdraw it.
                for route in &mut self.routes {
                    route.split_muted = state
                        .routes
                        .as_ref()
                        .and_then(|routes| {
                            routes.iter().find(|observed| {
                                observed.row == route.row
                                    && observed.input_id == route.input_id
                                    && observed.output_id == route.output_id
                                    && observed.split_column == route.split_column
                                    && observed.mix_column == route.mix_column
                                    && observed.splitter_model_id == route.splitter_model_id
                            })
                        })
                        .and_then(|observed| observed.split_muted);
                }
                if let Some(projection) = &state.blocks {
                    for block in &mut self.blocks {
                        block.bypassed = projection
                            .iter()
                            .find(|observed| {
                                observed.row == block.row
                                    && observed.column == block.column
                                    && observed.model_id.is_some()
                                    && observed.model_id == block.model_id
                            })
                            .and_then(|observed| observed.bypassed);
                    }
                }
                if state.footswitch_states.is_some() {
                    self.footswitch_states.clone_from(&state.footswitch_states);
                }
            }
            "dirty" => {
                if let Some(value) = state.dirty {
                    self.dirty = value;
                    self.observe("dirty");
                }
            }
            "master" => {
                if let Some(value) = state.master_volume {
                    self.master_volume = (value.clamp(0.0, 1.0) * 100.0).round() as u32;
                    self.observe("masterVolume");
                }
            }
            "mode" => {
                if let Some(value) = &state.mode {
                    self.mode.clone_from(value);
                    self.raw_mode = state.raw_mode;
                    self.observe("mode");
                    // The per-row pair belongs to the mode it was reported with. An update that
                    // reports a mode but no pair (an unknown mode value) clears the old pair
                    // instead of leaving the previous mode's split in place.
                    self.footswitch_modes.clone_from(&state.footswitch_modes);
                } else if state.footswitch_modes.is_some() {
                    self.footswitch_modes.clone_from(&state.footswitch_modes);
                }
                if state.mode_slots.is_some() {
                    self.mode_slots.clone_from(&state.mode_slots);
                }
            }
            "sceneLabel" => {
                if let (Some(index), Some(label)) = (state.index, &state.label) {
                    // The same display rule a full preset applies: an empty or blank stored label
                    // shows as `Scene A` to `Scene H`, and the stored text is kept as it came.
                    if let Some(current) = self.scenes.get_mut(index as usize) {
                        *current = crate::state::scene_display_label(index as usize, Some(label));
                    }
                    if let Some(raw) = self
                        .scene_labels_raw
                        .as_mut()
                        .and_then(|labels| labels.get_mut(index as usize))
                    {
                        raw.clone_from(label);
                    }
                }
            }
            "sceneColor" => {
                if let (Some(index), Some(color)) = (state.index, &state.color) {
                    let colors = self
                        .scene_colors
                        .get_or_insert_with(|| vec![String::new(); 8]);
                    if let Some(current) = colors.get_mut(index as usize) {
                        current.clone_from(color);
                    }
                    if let Some(raw) = state.raw_color {
                        // Only meaningful beside a full list; a lone push cannot start one.
                        if let Some(current) = self
                            .scene_colors_argb
                            .as_mut()
                            .and_then(|colors| colors.get_mut(index as usize))
                        {
                            *current = raw;
                        }
                    }
                }
            }
            "midiOut" => {
                if let Some(sources) = &state.midi_out {
                    crate::state::merge_midi_out_sources(
                        self.midi_out.get_or_insert_with(Vec::new),
                        sources,
                    );
                }
                if let Some(messages) = &state.preset_load_midi_out {
                    self.preset_load_midi_out = Some(messages.clone());
                }
            }
            "position" => {
                self.position_revision = self.position_revision.wrapping_add(1);
                if state
                    .position
                    .is_some_and(|position| position != self.preset_position)
                    || state.setlist_key.as_ref().is_some_and(|key| {
                        key.trim_end_matches('/') != self.setlist_key.trim_end_matches('/')
                    })
                {
                    self.address_revision = self.address_revision.saturating_add(1);
                }
                if let Some(position) = state.position {
                    self.observe("presetLocation");
                    self.preset_position = position;
                    self.preset_location = format!(
                        "{}{}",
                        position / 8 + 1,
                        (b'A' + (position % 8) as u8) as char
                    );
                }
                if let Some(key) = &state.setlist_key {
                    self.setlist_key.clone_from(key);
                    self.setlist_name = key
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or_default()
                        .into();
                }
            }
            "preset" => {
                self.has_preset = true;
                self.preset_revision = self.preset_revision.wrapping_add(1);
                // Type-15 includes ordinary full-preset reads. It cannot prove
                // that the edit buffer is clean; type-34 carries that state.
                if let Some(value) = state.dirty {
                    self.dirty = value;
                    self.observe("dirty");
                }
                if let Some(value) = &state.preset_name {
                    self.preset_name = if value.is_empty() {
                        "Unsaved".into()
                    } else {
                        value.clone()
                    };
                }
                if let Some(value) = state.tempo {
                    self.tempo = value;
                    self.observe("tempo");
                }
                if let Some(value) = state.tempo_led_enabled {
                    self.tempo_led_enabled = value;
                    self.observe("tempoLedEnabled");
                }
                if let Some(value) = &state.scenes {
                    self.scenes.clone_from(value);
                }
                if let Some(value) = &state.scene_labels_raw {
                    self.scene_labels_raw = Some(value.clone());
                }
                if let Some(value) = &state.scene_colors {
                    self.scene_colors = Some(value.clone());
                }
                if let Some(value) = &state.scene_colors_argb {
                    self.scene_colors_argb = Some(value.clone());
                }
                if let Some(value) = &state.footswitch_states {
                    self.footswitch_states = Some(value.clone());
                }
                if let Some(value) = &state.midi_out {
                    self.midi_out = Some(value.clone());
                }
                if let Some(value) = &state.preset_load_midi_out {
                    self.preset_load_midi_out = Some(value.clone());
                }
                if let Some(value) = &state.blocks {
                    self.blocks.clone_from(value);
                }
                if let Some(value) = state.grid_layout_complete {
                    self.grid_layout_complete = value;
                }
                if let Some(value) = &state.routes {
                    self.routes.clone_from(value);
                }
            }
            "tempo" => {
                if let Some(value) = state.tempo {
                    self.tempo = value;
                    self.observe("tempo");
                }
                if let Some(value) = state.tempo_led_enabled {
                    self.tempo_led_enabled = value;
                    self.observe("tempoLedEnabled");
                }
            }
            "ioPorts" => {
                if state.io_settings_error.is_some() {
                    self.io_ports = None;
                    self.io_settings_error = state.io_settings_error.clone().unwrap_or_default();
                    return;
                }
                if !self.io_settings_error.is_empty() {
                    return;
                }
                if let Some(updates) = &state.io_ports {
                    let ports = self.io_ports.get_or_insert_with(Vec::new);
                    for update in updates {
                        if let Some(current) = ports
                            .iter_mut()
                            .find(|port| port.kind == update.kind && port.id == update.id)
                        {
                            current.clone_from(update);
                        } else {
                            if ports.len() >= crate::responses::MAX_RETAINED_IO_RECORDS {
                                self.io_ports = None;
                                self.io_settings_error =
                                    crate::responses::ResponseDecodeError::RetainedIoBudget
                                        .to_string();
                                return;
                            }
                            ports.push(update.clone());
                        }
                    }
                }
            }
            "blockRemoved" => {
                if state.footswitch_states.is_some() {
                    self.footswitch_states.clone_from(&state.footswitch_states);
                }
                if let (Some(row), Some(column)) = (state.row, state.column) {
                    self.blocks
                        .retain(|block| block.row != row || block.column != column);
                }
                if let Some(value) = state.grid_layout_complete {
                    self.grid_layout_complete = value;
                }
            }
            "gridTopology" => {
                if state.refresh_required == Some(true) {
                    self.has_preset = false;
                    self.preset_identity = None;
                    self.preset_revision = self.preset_revision.wrapping_add(1);
                    self.address_revision = self.address_revision.saturating_add(1);
                }
                if let Some(value) = state.grid_layout_complete {
                    self.grid_layout_complete = value;
                }
            }
            // Each publication is the whole projection, so the latest replaces the last. It is
            // retained for snapshot reads and touches nothing about the current preset.
            "modelPresetCatalog" => {
                if let Some(value) = &state.model_preset_catalog {
                    self.model_preset_catalog = Some(value.clone());
                }
            }
            "bypass" => {
                if let (Some(row), Some(column), Some(bypassed)) =
                    (state.row, state.column, state.bypassed)
                {
                    if let Some(block) = self
                        .blocks
                        .iter_mut()
                        .find(|block| block.row == row && block.column == column)
                    {
                        block.bypassed = Some(bypassed);
                    }
                }
            }
            "bypassBatch" => {
                if state.footswitch_states.is_some() {
                    self.footswitch_states.clone_from(&state.footswitch_states);
                }
                if let Some(projection) = &state.blocks {
                    for observed in projection {
                        if let Some(block) = self.blocks.iter_mut().find(|block| {
                            block.row == observed.row
                                && block.column == observed.column
                                && observed.model_id.is_some()
                                && observed.model_id == block.model_id
                        }) {
                            block.bypassed = observed.bypassed;
                        }
                    }
                    return;
                }
                for update in state.bypass_updates.iter().flatten() {
                    if let Some(block) = self
                        .blocks
                        .iter_mut()
                        .find(|block| block.row == update.row && block.column == update.column)
                    {
                        block.bypassed = Some(update.bypassed);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_native_updates_to_the_app_snapshot_contract() {
        let mut snapshot = GatewaySnapshot::default();
        let mut position = StateUpdate::empty("position");
        position.position = Some(17);
        position.setlist_key = Some("/media/p4/Presets/Live/".into());
        snapshot.apply(&position);
        let mut preset = StateUpdate::empty("preset");
        preset.preset_name = Some("Direct Rust".into());
        preset.tempo = Some(96);
        snapshot.apply(&preset);
        let mut volume = StateUpdate::empty("master");
        volume.master_volume = Some(0.57);
        snapshot.apply(&volume);

        assert!(snapshot.has_preset);
        assert_eq!(snapshot.preset_name, "Direct Rust");
        assert_eq!(snapshot.preset_location, "3B");
        assert_eq!(snapshot.setlist_name, "Live");
        assert_eq!(snapshot.tempo, 96);
        assert_eq!(snapshot.master_volume, 57);
    }

    #[test]
    fn incomplete_grid_topology_survives_internal_preset_reprojections() {
        let mut snapshot = GatewaySnapshot::default();
        let mut full = StateUpdate::empty("preset");
        full.grid_layout_complete = Some(true);
        snapshot.apply(&full);
        let mut stale = StateUpdate::empty("gridTopology");
        stale.grid_layout_complete = Some(false);
        snapshot.apply(&stale);
        for kind in ["preset", "position", "catalog"] {
            let mut projection = StateUpdate::empty(kind);
            if kind == "preset" {
                projection.grid_layout_complete = Some(false);
            }
            snapshot.apply(&projection);
            assert!(
                !snapshot.grid_layout_complete,
                "{kind} must not restore topology exactness"
            );
        }
        let mut authoritative = StateUpdate::empty("preset");
        authoritative.grid_layout_complete = Some(true);
        snapshot.apply(&authoritative);
        assert!(snapshot.grid_layout_complete);
    }

    #[test]
    fn the_model_preset_projection_is_retained_whole_and_leaves_the_preset_alone() {
        let mut snapshot = GatewaySnapshot::default();
        assert!(snapshot.model_preset_catalog.is_none());
        let mut full = StateUpdate::empty("preset");
        full.grid_layout_complete = Some(true);
        snapshot.apply(&full);
        let revision = (snapshot.preset_revision, snapshot.full_preset_revision);

        let mut first = StateUpdate::empty("modelPresetCatalog");
        first.model_preset_catalog = Some(crate::generated_payloads::ModelPresetCatalogState {
            state: "folding".into(),
            session: String::new(),
            generation: 1,
            scope_hash: 4004,
            entries: Some(Vec::new()),
            active_marker: None,
            invalid_reason: None,
            counters: Default::default(),
        });
        snapshot.apply(&first);
        assert_eq!(
            snapshot.model_preset_catalog.as_ref().unwrap().generation,
            1
        );
        let mut second = first.clone();
        second.model_preset_catalog.as_mut().unwrap().generation = 2;
        second.model_preset_catalog.as_mut().unwrap().state = "invalidated".into();
        snapshot.apply(&second);
        let retained = snapshot.model_preset_catalog.as_ref().unwrap();
        assert_eq!(
            (retained.generation, retained.state.as_str()),
            (2, "invalidated")
        );
        // Neither the Grid completeness nor any preset revision moved.
        assert!(snapshot.grid_layout_complete);
        assert_eq!(
            revision,
            (snapshot.preset_revision, snapshot.full_preset_revision)
        );
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["modelPresetCatalog"]["state"], "invalidated");
    }

    #[test]
    fn preserves_a_nonempty_guard_name_for_unsaved_presets() {
        let mut snapshot = GatewaySnapshot::default();
        let mut preset = StateUpdate::empty("preset");
        preset.preset_name = Some(String::new());
        snapshot.apply(&preset);

        assert_eq!(snapshot.preset_name, "Unsaved");
    }

    #[test]
    fn full_preset_observations_do_not_infer_the_dirty_flag() {
        let mut snapshot = GatewaySnapshot {
            dirty: true,
            ..GatewaySnapshot::default()
        };
        let mut preset = StateUpdate::empty("preset");
        preset.tempo = Some(143);
        for catalog_refresh in [false, true] {
            preset.catalog_refresh = Some(catalog_refresh);
            snapshot.apply(&preset);
            assert!(
                snapshot.dirty,
                "a preset projection is not a dirty observation"
            );
            assert_eq!(snapshot.tempo, 143);
        }
        let mut dirty = StateUpdate::empty("dirty");
        dirty.dirty = Some(false);
        snapshot.apply(&dirty);
        assert!(!snapshot.dirty);
        dirty.dirty = Some(true);
        snapshot.apply(&dirty);
        snapshot.apply(&preset);
        assert!(snapshot.dirty);
    }

    #[test]
    fn native_block_removal_does_not_claim_a_complete_preset_or_clear_dirty() {
        let block = |column| {
            serde_json::from_value(serde_json::json!({
                "id": format!("0:{column}"), "name": "Test", "kind": "utility",
                "row": 0, "column": column, "modelId": 9013, "bypassed": true
            }))
            .unwrap()
        };
        let mut snapshot = GatewaySnapshot {
            dirty: true,
            tempo: 147,
            blocks: vec![block(1), block(2)],
            ..GatewaySnapshot::default()
        };
        let mut removed = StateUpdate::empty("blockRemoved");
        removed.row = Some(0);
        removed.column = Some(1);
        snapshot.apply(&removed);
        assert_eq!(snapshot.blocks, vec![block(2)]);
        assert!(snapshot.dirty);
        assert_eq!(snapshot.tempo, 147);
        assert!(!snapshot.has_preset);
        assert_eq!(snapshot.preset_revision, 0);
        snapshot.apply(&removed);
        assert_eq!(snapshot.blocks, vec![block(2)]);
    }

    #[test]
    fn merges_partial_io_pushes_without_losing_other_port_states() {
        let mut snapshot = GatewaySnapshot::default();
        let mut inputs = StateUpdate::empty("ioPorts");
        inputs.io_ports = Some(vec![IoPortState {
            kind: "input".into(),
            id: 1,
            label: "In 1".into(),
            plugged: true,
        }]);
        snapshot.apply(&inputs);

        let mut output = StateUpdate::empty("ioPorts");
        output.io_ports = Some(vec![IoPortState {
            kind: "output".into(),
            id: 4,
            label: "Out 1".into(),
            plugged: false,
        }]);
        snapshot.apply(&output);

        let ports = snapshot.io_ports.as_ref().unwrap();
        assert_eq!(ports.len(), 2);
        assert!(ports
            .iter()
            .any(|port| port.kind == "input" && port.plugged));
        assert!(ports
            .iter()
            .any(|port| port.kind == "output" && !port.plugged));
    }

    #[test]
    fn preset_library_expands_device_listings_to_all_qc_slots() {
        let mut library = PresetLibrary::default();
        library.ingest(PresetFolderListing {
            order: None,
            key: "/media/p4/Presets/Live".into(),
            name: "Live".into(),
            file_type: Some(0),
            is_factory: false,
            is_plugin: false,
            files: vec![crate::state::PresetFileListing {
                position: 17,
                name: "Direct Rust".into(),
                instrument: 1,
                instrument_present: true,
                product_key: Some("/media/p4/Presets/Live/Direct Rust.pb".into()),
                ..Default::default()
            }],
        });
        let snapshot = GatewaySnapshot {
            setlist_key: "/media/p4/Presets/Live".into(),
            preset_position: 17,
            ..GatewaySnapshot::default()
        };
        let list = library.list(&snapshot.setlist_key, &snapshot).unwrap();
        assert_eq!(list.presets.len(), 256);
        assert_eq!(list.presets[17].location, "3B");
        assert_eq!(list.presets[17].name, "Direct Rust");
        assert_eq!(list.current_position, 17);
    }

    #[test]
    fn preset_folders_follow_desktop_categories_and_exclude_other_providers() {
        let mut library = PresetLibrary::default();
        let folder =
            |key: &str, name: &str, file_type, is_factory, is_plugin| PresetFolderListing {
                order: None,
                key: key.into(),
                name: name.into(),
                file_type,
                is_factory,
                is_plugin,
                files: Vec::new(),
            };
        library.ingest(folder(
            "/media/p4/Presets/zeta",
            "zeta",
            Some(0),
            false,
            false,
        ));
        library.ingest(folder(
            "/media/p4/Presets/Alpha",
            "Alpha",
            Some(0),
            false,
            false,
        ));
        library.ingest(folder(
            "/opt/neuraldsp/Factory Library",
            "Factory Library",
            Some(0),
            false,
            false,
        ));
        library.ingest(folder(
            "/media/p4/Presets/Plugin",
            "Plugin",
            Some(0),
            false,
            true,
        ));
        library.ingest(folder(
            "/media/p4/Presets/Captures",
            "Captures",
            Some(1),
            false,
            false,
        ));
        library.ingest(folder(
            "/media/p4/Presets/Alpha/Nested",
            "Nested",
            Some(0),
            false,
            false,
        ));

        let folders = library.folders();
        assert_eq!(
            folders
                .iter()
                .map(|folder| folder.name.as_str())
                .collect::<Vec<_>>(),
            ["Factory Library", "Alpha", "zeta"]
        );
        assert!(folders[0].is_factory);
    }

    #[test]
    fn folders_follow_the_observed_device_order_with_factory_pinned_first() {
        let mut library = PresetLibrary::default();
        let folder = |name: &str, order: Option<u32>, is_factory: bool| PresetFolderListing {
            order,
            key: format!("/media/p4/Presets/{name}"),
            name: name.into(),
            file_type: Some(0),
            is_factory,
            is_plugin: false,
            files: Vec::new(),
        };
        // Alphabetical order disagrees with the device order on purpose.
        library.ingest(folder("Folder C", Some(0), false));
        library.ingest(folder("Folder A", Some(2), false));
        library.ingest(folder("Folder B", Some(1), false));
        library.ingest(folder("Factory", Some(3), true));
        let names = |folders: Vec<PresetFolder>| {
            folders
                .into_iter()
                .map(|folder| folder.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(library.observed_folders()),
            ["Factory", "Folder C", "Folder B", "Folder A"]
        );
        assert_eq!(
            library.observed_folders()[1].order,
            Some(0),
            "the order is carried through to the projected folder"
        );
        // Folders that were never observed in order fall back to the name sort, after the
        // ordered ones, and the sort stays total.
        library.ingest(folder("Folder E", None, false));
        library.ingest(folder("folder D", None, false));
        assert_eq!(
            names(library.observed_folders()),
            ["Factory", "Folder C", "Folder B", "Folder A", "folder D", "Folder E"]
        );
        // Without any order the projection is the alphabetical one it always was.
        let mut plain = PresetLibrary::default();
        plain.ingest(folder("Folder B", None, false));
        plain.ingest(folder("Folder A", None, false));
        assert_eq!(names(plain.folders()), ["Folder A", "Folder B"]);
        let json = serde_json::to_value(plain.folders()).unwrap();
        assert!(json[0].get("order").is_none());
    }

    #[test]
    fn a_shorter_folder_frame_replaces_the_complete_cached_listing() {
        let mut library = PresetLibrary::default();
        let key = "/media/p4/Presets/Live";
        let file = |position, name: &str| crate::state::PresetFileListing {
            position,
            name: name.into(),
            instrument: 1,
            instrument_present: true,
            product_key: Some(format!("{key}/{name}.pb")),
            ..Default::default()
        };
        library.ingest(PresetFolderListing {
            order: None,
            key: key.into(),
            name: "Live".into(),
            file_type: Some(0),
            is_factory: false,
            is_plugin: false,
            files: vec![file(1, "Keep"), file(2, "Deleted Elsewhere")],
        });
        library.ingest(PresetFolderListing {
            order: None,
            key: key.into(),
            name: "Live".into(),
            file_type: Some(0),
            is_factory: false,
            is_plugin: false,
            files: vec![file(1, "Keep")],
        });

        assert_eq!(library.entry(key, 1).unwrap().name, "Keep");
        assert!(library.entry(key, 2).is_none());
    }

    #[test]
    fn catalog_name_updates_preserve_the_authoritative_product_key() {
        let mut library = PresetLibrary::default();
        let key = "/media/p4/Presets/Live";
        let product_key = "opaque-device-product-key";
        library.ingest(PresetFolderListing {
            order: None,
            key: key.into(),
            name: "Live".into(),
            file_type: Some(0),
            is_factory: false,
            is_plugin: false,
            files: vec![crate::state::PresetFileListing {
                position: 8,
                name: "Before".into(),
                instrument: 2,
                instrument_present: true,
                product_key: Some(product_key.into()),
                ..Default::default()
            }],
        });

        library.edit_cached_preset(key, 8, "After", 2);

        assert_eq!(
            library.entry_product_key(key, 8).as_deref(),
            Some(product_key)
        );
    }

    #[test]
    fn a_later_device_publication_replaces_the_previous_product_key() {
        let mut library = PresetLibrary::default();
        let key = "/media/p4/Presets/My Presets";
        let frame = |name: &str| PresetFolderListing {
            order: None,
            key: key.into(),
            name: "My Presets".into(),
            file_type: Some(0),
            is_factory: false,
            is_plugin: false,
            files: vec![crate::state::PresetFileListing {
                position: 40,
                name: name.into(),
                instrument: 1,
                instrument_present: true,
                product_key: Some(format!("{key}/{name}.pb")),
                ..Default::default()
            }],
        };

        // save_preset_as("Foo", 40): the verified frame names Foo.pb.
        library.ingest(frame("Foo"));
        assert_eq!(
            library.entry_product_key(key, 40).as_deref(),
            Some(format!("{key}/Foo.pb").as_str())
        );

        // A later native aggregate replaces the visible name and identity.
        library.ingest(frame("Baz"));

        assert_eq!(
            library.entry_product_key(key, 40).as_deref(),
            Some(format!("{key}/Baz.pb").as_str()),
            "a following delete_preset(40, \"Baz\") would send File DELETE with the stale Foo key"
        );
    }

    #[test]
    fn writable_preset_slots_reject_the_factory_library_once() {
        let library = PresetLibrary::default();
        let factory = GatewaySnapshot {
            setlist_key: "/opt/neuraldsp/Factory Library".into(),
            ..GatewaySnapshot::default()
        };
        assert!(library.writable_slots(&factory).is_err());
    }

    #[test]
    fn a_slot_listing_refusal_describes_the_listing_and_never_a_save() {
        // The device answers HTTP 422 ("Factory Library is read-only. Recall
        // a user setlist before saving.") for `device.listPresetSlots {}`
        // while sitting in the Factory Library, even though a slot listing
        // never writes anything. This validator is reached only from the
        // slot-listing paths (`writable_slots` and the Android
        // `validatePresetSlotsTarget`); the save planner in request.rs
        // carries its own message. So the refusal must describe what the
        // caller actually asked for, or a read teaches its caller that it
        // attempted a write.
        let library = PresetLibrary::default();
        for (active, expected) in [
            (
                GatewaySnapshot {
                    setlist_key: "/opt/neuraldsp/Factory Library".into(),
                    ..GatewaySnapshot::default()
                },
                "read-only Factory Library",
            ),
            (
                GatewaySnapshot {
                    setlist_key: "/media/p4/Presets/Live/Nested".into(),
                    ..GatewaySnapshot::default()
                },
                "writable user setlist",
            ),
            (
                GatewaySnapshot {
                    setlist_key: "/media/p4/Presets/Live".into(),
                    preset_position: 256,
                    ..GatewaySnapshot::default()
                },
                "256-slot setlist",
            ),
        ] {
            let error = library
                .writable_slots(&active)
                .expect_err("a non-writable target must fail closed");
            assert!(
                error.contains(expected),
                "{error:?} must explain the listing refusal with {expected:?}"
            );
            assert!(
                !error.contains("saving") && !error.contains("renaming"),
                "{error:?} is answering a listing call and must not name a write"
            );
            assert!(
                error.contains("slot") || error.contains("destination"),
                "{error:?} must name the slots the caller asked to list"
            );
        }
    }

    #[test]
    fn preset_slot_occupancy_comes_from_catalog_identity_not_display_name() {
        let key = "/media/p4/Presets/Live";
        let mut library = PresetLibrary::default();
        library.ingest(PresetFolderListing {
            order: None,
            key: key.into(),
            name: "Live".into(),
            file_type: Some(0),
            is_factory: false,
            is_plugin: false,
            files: vec![
                crate::state::PresetFileListing {
                    position: 0,
                    name: "Unsaved".into(),
                    instrument: 1,
                    instrument_present: true,
                    product_key: Some("stored-unsaved-name".into()),
                    ..Default::default()
                },
                crate::state::PresetFileListing {
                    position: 255,
                    name: String::new(),
                    instrument: 2,
                    instrument_present: true,
                    product_key: Some("stored-empty-name".into()),
                    ..Default::default()
                },
            ],
        });
        let active = GatewaySnapshot {
            setlist_key: key.into(),
            preset_position: 255,
            ..GatewaySnapshot::default()
        };
        assert!(library.writable_slots(&active).unwrap().is_none());
        let mut complete = library.observed_listing(key).unwrap().clone();
        complete
            .files
            .extend((1..255).map(|position| crate::state::PresetFileListing {
                position,
                name: String::new(),
                instrument: 0,
                instrument_present: false,
                product_key: None,
                ..Default::default()
            }));
        library.ingest(complete);
        let result = library.writable_slots(&active).unwrap().unwrap();
        assert_eq!(result.slots.len(), 256);
        assert_eq!(
            (
                result.slots[0].location.as_str(),
                result.slots[255].location.as_str()
            ),
            ("1A", "32H")
        );
        assert!(result.slots[0].occupied);
        assert_eq!(result.slots[0].name, "Unsaved");
        assert!(result.slots[255].occupied);
        assert_eq!(result.slots[255].name, "");
        assert!(!result.slots[1].occupied);
    }

    #[test]
    fn entry_reports_an_empty_named_occupied_slot_as_present() {
        // Same occupancy rule as `preset_slot_occupancy_comes_from_catalog_identity_not_display_name`:
        // a slot with a product_key is occupied even when its display name is
        // empty. `entry()` is the lookup savePresetAs' overwrite guard uses, so
        // it must agree, or an empty-named occupied slot silently bypasses the
        // confirmOverwrite requirement.
        let key = "/media/p4/Presets/Live";
        let mut library = PresetLibrary::default();
        library.ingest(PresetFolderListing {
            order: None,
            key: key.into(),
            name: "Live".into(),
            file_type: Some(0),
            is_factory: false,
            is_plugin: false,
            files: vec![crate::state::PresetFileListing {
                position: 255,
                name: String::new(),
                instrument: 2,
                instrument_present: true,
                product_key: Some("stored-empty-name".into()),
                ..Default::default()
            }],
        });

        let entry = library
            .entry(key, 255)
            .expect("a slot with a product_key is occupied even with an empty display name");
        assert_eq!(entry.position, 255);
        assert_eq!(entry.name, "");
        assert_eq!(entry.instrument, 2);
        assert!(entry.instrument_present);
        assert_eq!(entry.product_key.as_deref(), Some("stored-empty-name"));
    }

    #[test]
    fn slot_listing_occupancy_agrees_with_entry_for_a_named_keyless_slot() {
        // Regression: `entry()` (used by savePresetAs' overwrite guard) treats
        // a named-but-keyless slot (a real compatibility-copy/rename result,
        // see the `*_keyless_*` tests in request.rs) as occupied, but
        // `project_slots()` (backing `device.listPresetSlots`/`slots()`) used
        // to gate purely on `product_key.is_some()`. That let a client see
        // "occupied: false" from the listing while the very same slot's
        // overwrite guard demanded confirmOverwrite -- a contradiction for
        // identical underlying data. The two lookups must agree.
        let key = "/media/p4/Presets/Live";
        let mut library = PresetLibrary::default();
        library.ingest(PresetFolderListing {
            order: None,
            key: key.into(),
            name: "Live".into(),
            file_type: Some(0),
            is_factory: false,
            is_plugin: false,
            files: std::iter::once(crate::state::PresetFileListing {
                position: 255,
                name: "Compatibility copy".into(),
                instrument: 2,
                instrument_present: true,
                product_key: None,
                ..Default::default()
            })
            .chain((0..255).map(|position| crate::state::PresetFileListing {
                position,
                name: String::new(),
                instrument: 0,
                instrument_present: false,
                product_key: None,
                ..Default::default()
            }))
            .collect(),
        });
        let active = GatewaySnapshot {
            setlist_key: key.into(),
            preset_position: 255,
            ..GatewaySnapshot::default()
        };

        assert!(
            library.entry(key, 255).is_some(),
            "entry() must treat the named-but-keyless slot as present"
        );
        let result = library
            .writable_slots(&active)
            .unwrap()
            .expect("complete 256-slot listing");
        assert!(
            result.slots[255].occupied,
            "project_slots()/slots() must agree with entry(): a named-but-keyless \
             slot is occupied, or listPresetSlots contradicts savePresetAs' overwrite guard"
        );
        assert!(!result.slots[0].occupied);
    }

    #[test]
    fn writable_preset_slots_fail_closed_on_invalid_active_address() {
        let library = PresetLibrary::default();
        for active in [
            GatewaySnapshot {
                setlist_key: "/media/p4/Presets/Live/Nested".into(),
                preset_position: 0,
                ..GatewaySnapshot::default()
            },
            GatewaySnapshot {
                setlist_key: "/media/p4/Presets/Live".into(),
                preset_position: 256,
                ..GatewaySnapshot::default()
            },
        ] {
            assert!(library.writable_slots(&active).is_err());
        }
    }

    #[test]
    fn a_new_snapshot_claims_nothing_the_device_has_not_said() {
        let snapshot = GatewaySnapshot::default();
        assert_eq!(
            snapshot.mode, "STOMP",
            "a placeholder, listed as unobserved"
        );
        assert_eq!(snapshot.raw_mode, None);
        assert_eq!(snapshot.scene_labels_raw, None);
        assert_eq!(snapshot.scene_colors_argb, None);
        assert_eq!(
            snapshot.unobserved,
            [
                "mode",
                "tempo",
                "tempoLedEnabled",
                "masterVolume",
                "dirty",
                "presetLocation"
            ]
        );
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["unobserved"][0], "mode");
        assert!(json.get("rawMode").is_none());
    }

    #[test]
    fn each_placeholder_leaves_the_unobserved_list_when_a_device_message_sets_it() {
        let mut snapshot = GatewaySnapshot::default();
        let mut master = StateUpdate::empty("master");
        master.master_volume = Some(0.5);
        snapshot.apply(&master);
        assert!(!snapshot.unobserved.contains(&"masterVolume"));
        assert_eq!(snapshot.master_volume, 50);

        let mut mode = StateUpdate::empty("mode");
        mode.mode = Some("SCENE".into());
        mode.raw_mode = Some(1);
        snapshot.apply(&mode);
        assert!(!snapshot.unobserved.contains(&"mode"));

        // A dirty update that carries no value observes nothing.
        snapshot.apply(&StateUpdate::empty("dirty"));
        assert!(snapshot.unobserved.contains(&"dirty"));
        let mut dirty = StateUpdate::empty("dirty");
        dirty.dirty = Some(false);
        snapshot.apply(&dirty);
        assert!(!snapshot.unobserved.contains(&"dirty"));

        let mut position = StateUpdate::empty("position");
        position.position = Some(9);
        snapshot.apply(&position);
        assert!(!snapshot.unobserved.contains(&"presetLocation"));

        // A preset without a tempo leaves the tempo a placeholder.
        let mut preset = StateUpdate::empty("preset");
        preset.preset_name = Some("No tempo".into());
        snapshot.apply(&preset);
        assert!(snapshot.unobserved.contains(&"tempo"));
        assert_eq!(snapshot.tempo, 120);
        preset.tempo = Some(212);
        preset.tempo_led_enabled = Some(true);
        snapshot.apply(&preset);
        assert_eq!(snapshot.unobserved, Vec::<&str>::new());
        let json = serde_json::to_value(&snapshot).unwrap();
        assert!(json.get("unobserved").is_none());
    }

    #[test]
    fn mode_updates_carry_the_raw_value_and_an_unknown_value_is_not_mapped_to_a_mode() {
        let mut snapshot = GatewaySnapshot::default();
        let mut known = StateUpdate::empty("mode");
        known.mode = Some("HYBRID".into());
        known.raw_mode = Some(6);
        known.footswitch_modes = Some(vec!["SCENE".into(), "STOMP".into()]);
        snapshot.apply(&known);
        assert_eq!(snapshot.raw_mode, Some(6));
        assert_eq!(snapshot.footswitch_modes.as_ref().unwrap()[1], "STOMP");

        // An unknown value keeps its number, and the previous mode's pair does not outlive it.
        let mut unknown = StateUpdate::empty("mode");
        unknown.mode = Some("UNKNOWN".into());
        unknown.raw_mode = Some(11);
        snapshot.apply(&unknown);
        assert_eq!(snapshot.mode, "UNKNOWN");
        assert_eq!(snapshot.raw_mode, Some(11));
        assert_eq!(snapshot.footswitch_modes, None);
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["rawMode"], 11);
        assert!(json.get("footswitchModes").is_none());
    }

    #[test]
    fn a_scene_label_push_uses_the_display_rule_a_full_preset_uses_and_keeps_the_stored_text() {
        let mut snapshot = GatewaySnapshot::default();
        let mut preset = StateUpdate::empty("preset");
        preset.scenes = Some(
            (0..8)
                .map(|index| format!("Scene {}", (b'A' + index) as char))
                .collect(),
        );
        preset.scene_labels_raw = Some(vec![String::new(); 8]);
        snapshot.apply(&preset);

        let mut label = StateUpdate::empty("sceneLabel");
        label.index = Some(2);
        label.label = Some("Lead".into());
        snapshot.apply(&label);
        assert_eq!(snapshot.scenes[2], "Lead");
        assert_eq!(snapshot.scene_labels_raw.as_ref().unwrap()[2], "Lead");

        // Clearing a label shows the default text, as a full preset would, and stores nothing.
        label.label = Some(String::new());
        snapshot.apply(&label);
        assert_eq!(snapshot.scenes[2], "Scene C");
        assert_eq!(snapshot.scene_labels_raw.as_ref().unwrap()[2], "");
    }
}
