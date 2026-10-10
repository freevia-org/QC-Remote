//! `profile.rs` is auto-generated ("Do not edit by hand"). Repository-level
//! `npm run protocol:check` proves that it is current. This packaged test
//! proves the generated output retains the
//! explanation without reaching outside the crate artifact.
//!
//! `TUNER_CONTROLLER`, `GIG_VIEW_CONTROLLER`,
//! `MODE_SLOT_CONTROLLER`, `MIDI_FEATURE_OFF_VALUE`, and
//! `MIDI_FEATURE_ON_VALUE` have zero call sites anywhere in
//! `quadcortex` or `quadcortex::device` --
//! `device.showTuner`/`device.showGigView`/`device.selectModeSlot` all use the
//! generated gateway path, never host MIDI. They cannot simply be deleted from
//! the shared contract, though: other generated clients use the constants.
//! So the constants stay, with a comment saying plainly why nothing in this
//! crate calls them -- this test pins the generator template that produces
//! that comment, so it cannot silently drop out on a future regeneration.

use quadcortex::profile::{
    GIG_VIEW_CONTROLLER, MIDI_FEATURE_OFF_VALUE, MIDI_FEATURE_ON_VALUE, MODE_SLOT_CONTROLLER,
    TUNER_CONTROLLER,
};

#[test]
fn dead_midi_cc_constants_keep_their_documented_wire_values() {
    // Regression check: the comment change must not touch the values
    // themselves, which are cross-language contract constants.
    assert_eq!(TUNER_CONTROLLER, 45);
    assert_eq!(GIG_VIEW_CONTROLLER, 46);
    assert_eq!(MODE_SLOT_CONTROLLER, 47);
    assert_eq!(MIDI_FEATURE_OFF_VALUE, 0);
    assert_eq!(MIDI_FEATURE_ON_VALUE, 127);
}

#[test]
fn dead_midi_cc_constants_are_explained_in_the_packaged_profile() {
    let generated = include_str!("../src/profile.rs");

    assert!(
        generated.contains("zero call sites"),
        "the generated profile must explain why TUNER_CONTROLLER/GIG_VIEW_CONTROLLER/\
         MODE_SLOT_CONTROLLER/MIDI_FEATURE_OFF_VALUE/MIDI_FEATURE_ON_VALUE have no \
         call site in this crate"
    );
    assert!(
        generated.contains("gateway path, never host MIDI"),
        "the explanation must identify the public dispatch path these operations use"
    );
    assert!(
        generated.contains("other generated clients use them"),
        "the explanation must say why the fields remain in the shared contract"
    );
}
