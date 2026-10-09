//! Shared authoritative-catalog reconciliation for persistent preset writes.
//!
//! Native hosts may schedule reads differently, but they must accept and
//! publish the same stored preset.  Keeping the matching and result update in
//! one module also prevents an occupied destination's pre-save entry from
//! being mistaken for proof that an overwrite completed.

use crate::device::request::{stored_preset_name_matches, PresetMutationRecord};

/// Reconcile requested preset mutations against one fresh device catalog.
///
/// `observed` is intentionally a compact host-neutral projection. Every
/// requested slot must contain the requested name or CorOS's collision suffix
/// form. On success the records are replaced with the authoritative stored
/// name and instrument. The persistent write is never replayed on failure.
pub fn reconcile_saved_preset_records(
    requested: &mut [PresetMutationRecord],
    observed: &[PresetMutationRecord],
) -> Result<(), String> {
    let resolved = requested
        .iter()
        .map(|preset| {
            let stored = observed
                .iter()
                .find(|candidate| {
                    candidate.setlist_key.trim_end_matches('/')
                        == preset.setlist_key.trim_end_matches('/')
                        && candidate.position == preset.position
                        && !candidate.name.is_empty()
                        && if preset.allow_collision_suffix {
                            stored_preset_name_matches(&preset.name, &candidate.name)
                        } else {
                            preset.name == candidate.name
                        }
                })
                .ok_or_else(|| {
                    format!(
                        "The fresh QC catalog did not contain the saved preset at slot {}",
                        preset.position
                    )
                })?;
            Ok((stored.name.clone(), stored.instrument))
        })
        .collect::<Result<Vec<_>, String>>()?;
    for (preset, (name, instrument)) in requested.iter_mut().zip(resolved) {
        preset.name = name;
        preset.instrument = instrument;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        setlist_key: &str,
        position: u32,
        name: &str,
        instrument: i32,
    ) -> PresetMutationRecord {
        PresetMutationRecord {
            setlist_key: setlist_key.into(),
            position,
            name: name.into(),
            instrument,
            allow_collision_suffix: true,
        }
    }

    #[test]
    fn reconciles_exact_and_collision_resolved_names() {
        let mut requested = vec![record("/media/p4/Presets/User/", 7, "Crying Wah", 0)];
        let observed = vec![record("/media/p4/Presets/User", 7, "Crying Wah_12", 3)];
        reconcile_saved_preset_records(&mut requested, &observed).unwrap();
        assert_eq!(requested[0].name, "Crying Wah_12");
        assert_eq!(requested[0].instrument, 3);
    }

    #[test]
    fn reconciles_greater_existing_suffix_and_returns_authoritative_name() {
        let mut requested = vec![record("/media/p4/Presets/User", 30, "Scratch_1", 0)];
        let observed = vec![record("/media/p4/Presets/User/", 30, "Scratch_2", 3)];

        reconcile_saved_preset_records(&mut requested, &observed).unwrap();

        assert_eq!(requested[0].name, "Scratch_2");
        assert_eq!(requested[0].instrument, 3);
    }

    #[test]
    fn greater_suffix_does_not_match_a_different_slot_or_setlist() {
        let requested = vec![record("user", 30, "Scratch_1", 0)];
        let wrong_slot = vec![record("user", 31, "Scratch_2", 3)];
        let wrong_setlist = vec![record("other", 30, "Scratch_2", 3)];
        let mut requested_for_wrong_slot = requested.clone();
        let mut requested_for_wrong_setlist = requested;

        assert!(
            reconcile_saved_preset_records(&mut requested_for_wrong_slot, &wrong_slot).is_err()
        );
        assert!(
            reconcile_saved_preset_records(&mut requested_for_wrong_setlist, &wrong_setlist)
                .is_err()
        );
    }

    #[test]
    fn rejects_an_occupied_slots_stale_previous_entry() {
        let mut requested = vec![record("user", 7, "Replacement", 0)];
        let observed = vec![record("user", 7, "Previous", 2)];
        assert_eq!(
            reconcile_saved_preset_records(&mut requested, &observed).unwrap_err(),
            "The fresh QC catalog did not contain the saved preset at slot 7"
        );
        assert_eq!(requested[0].name, "Replacement");
    }

    #[test]
    fn rejects_cross_setlist_and_cross_slot_matches() {
        let mut requested = vec![record("user-a", 7, "Saved", 0)];
        let observed = vec![
            record("user-b", 7, "Saved", 1),
            record("user-a", 8, "Saved", 1),
        ];
        assert!(reconcile_saved_preset_records(&mut requested, &observed).is_err());
    }

    #[test]
    fn exact_records_reject_save_style_collision_suffixes() {
        let mut requested = vec![record("user", 7, "Selected", 0)];
        requested[0].allow_collision_suffix = false;
        let observed = vec![record("user", 7, "Selected_1", 0)];
        assert!(reconcile_saved_preset_records(&mut requested, &observed).is_err());
    }
}
