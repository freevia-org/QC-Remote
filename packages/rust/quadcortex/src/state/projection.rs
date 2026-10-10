use super::*;

pub(super) fn mode_base_name(value: u32) -> Option<&'static str> {
    match value {
        0 => Some("PRESET"),
        1 => Some("SCENE"),
        2 => Some("STOMP"),
        _ => None,
    }
}

/// The `(first, second)` base modes of a hybrid wire value `3..=8`, in the order the desktop
/// application names them (`3` is Preset then Scene, ... `8` is Stomp then Scene).
///
/// Which footswitches the first and second mode govern is believed to be A-D and E-H. That
/// split is this library's assumption; no read of the unit has shown it.
pub(super) fn mode_pair(value: u32) -> Option<[u32; 2]> {
    const HYBRID: [[u32; 2]; 6] = [[0, 1], [0, 2], [1, 0], [1, 2], [2, 0], [2, 1]];
    match value {
        0..=2 => Some([value, value]),
        3..=8 => Some(HYBRID[(value - 3) as usize]),
        _ => None,
    }
}

/// Coarse kind and per-row base modes for a mode value, or `None` for a value outside `0..=8`.
/// The one table every mode comparison in the crate reads.
pub(crate) fn mode_identity(value: u32) -> Option<(&'static str, [&'static str; 2])> {
    let pair = mode_pair(value)?;
    let names = pair.map(|base| mode_base_name(base).unwrap_or(UNKNOWN_MODE));
    let kind = if (3..=8).contains(&value) {
        "HYBRID"
    } else {
        names[0]
    };
    Some((kind, names))
}

pub(super) fn mode_name(value: u32) -> String {
    mode_base_name(value).unwrap_or(UNKNOWN_MODE).into()
}
pub(super) fn mode_kind(value: u32) -> String {
    mode_identity(value)
        .map(|(kind, _)| kind)
        .unwrap_or(UNKNOWN_MODE)
        .into()
}
pub(super) fn mode_label(value: u32) -> String {
    match value {
        3 => "PRESET / SCENE",
        4 => "PRESET / STOMP",
        5 => "SCENE / PRESET",
        6 => "SCENE / STOMP",
        7 => "STOMP / PRESET",
        8 => "STOMP / SCENE",
        _ => return mode_name(value),
    }
    .into()
}
/// The label a scene is shown with. A label that is empty or only blanks is replaced by `Scene A`
/// to `Scene H`; the replacement is this library's text, and the stored label stays available
/// as `sceneLabelsRaw`.
pub(crate) fn scene_display_label(index: usize, stored: Option<&String>) -> String {
    stored
        .filter(|label| !label.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| format!("Scene {}", (b'A' + index as u8) as char))
}

/// The `#rrggbb` form of a 32-bit ARGB scene colour. The alpha byte is dropped here and only here;
/// the raw value travels beside this string as `rawColor` and `sceneColorsArgb`.
pub(super) fn color_hex(value: u32) -> String {
    format!("#{:06x}", value & 0x00ff_ffff)
}

/// Display strings the decoder keeps for two route ids although the label tables name them
/// differently. The Grid screens compare these exact strings to recognise an unrouted port and a
/// row 3/4 output, so changing them is a change to those screens and is left to them. They are
/// the only places the decoded label and the table label disagree; a test pins the list.
///
/// * input 0, wire name `EMPTY`: the table and the unit's own screen say `Not In Use`; the decoder
///   says `Internal`, a word this library chose.
/// * output 18, wire name `NEXT_ROW_3_4`: the table and the unit's screen say `Row 3/4`; the
///   decoder says `Rows 3/4`.
pub(crate) const INPUT_LABEL_ALIASES: &[(u32, &str)] = &[(0, "Internal")];
pub(crate) const OUTPUT_LABEL_ALIASES: &[(u32, &str)] = &[(18, "Rows 3/4")];

pub(super) fn route_label(
    value: u32,
    aliases: &[(u32, &'static str)],
    labels: &[(u32, &'static str)],
    wire_names: &[(u32, &'static str)],
    unknown: &str,
) -> String {
    let find = |table: &[(u32, &'static str)]| {
        table
            .iter()
            .find(|(id, _)| *id == value)
            .map(|(_, text)| *text)
    };
    // One table of labels, keyed by wire value. A wire value the label table does not name reads
    // as its wire enum name; a value the wire schema does not declare reads as unknown. Nothing is
    // mapped to a plausible label.
    find(aliases)
        .or_else(|| find(labels))
        .or_else(|| find(wire_names))
        .map(str::to_string)
        .unwrap_or_else(|| format!("{unknown} {value}"))
}
