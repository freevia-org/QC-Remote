// Generated from public contract snapshots and private maintainer specifications; do not edit by hand. The generator is kept in the private repository.
/// Number of rows in a preset's Grid; a row index is valid below this bound.
pub const GRID_ROWS: u32 = 4;
/// Number of columns in a preset's Grid; a column index is valid below this bound.
pub const GRID_COLUMNS: u32 = 8;
/// Number of scenes in a preset.
pub const SCENE_COUNT: u32 = 8;
/// Lowest tempo, in beats per minute, that a tempo write accepts.
pub const MINIMUM_TEMPO_BPM: u32 = 40;
/// Highest tempo, in beats per minute, that a tempo write accepts.
pub const MAXIMUM_TEMPO_BPM: u32 = 240;
/// Maximum number of characters in a preset name as the device stores it.
pub const STORED_PRESET_NAME_CHARACTERS: usize = 20;
/// Largest gateway IPC frame, in bytes. A frame whose length prefix is zero or exceeds this is rejected.
pub const IPC_MAX_FRAME_BYTES: usize = 16777216;
/// Number of buffered state events a read returns when the caller gives no limit.
pub const STATE_EVENT_DEFAULT_LIMIT: usize = 256;
/// Largest number of state events one read may request.
pub const STATE_EVENT_MAXIMUM_LIMIT: usize = 4096;
/// Byte budget for the encoded frames in one state-event page.
pub const STATE_EVENT_PAGE_MAXIMUM_BYTES: usize = 4194304;
/// Byte budget for the encoded frames the state-event history retains.
pub const STATE_EVENT_HISTORY_MAXIMUM_BYTES: usize = 8388608;
/// The scene colour swatches the vendor's desktop application offers, as 32-bit ARGB values in scene order, index-aligned with [`SCENE_COLORS`]. A preset stores raw ARGB values, so a scene colour is not an index into this table.
pub const SCENE_COLOR_ARGB: &[u32] = &[
    4294911783, 4278875360, 4294955574, 4294902466, 4282775650, 4294930432, 4285093119, 4278255581,
];
/// The same palette as `#rrggbb` strings, in scene order, index-aligned with [`SCENE_COLOR_ARGB`]. These are the colours the unit's screen draws, which is the stored value reduced to 5, 6 and 5 bits per channel; they are not the stored values, so they differ from the ARGB table in the low bits.
pub const SCENE_COLORS: &[&str] = &[
    "#ff2421", "#0875e7", "#ffd331", "#ff00c6", "#42fb63", "#ff7100", "#6b55ff", "#00ffde",
];
/// Colour constants as `#rrggbb` strings: one per block category, then the hardware
/// colours (lamp and metal tones). The category colours are the ones
/// `category_color` returns.
pub mod visual_colors {
    /// 24-bit colour of the plugin block category.
    pub const PLUGIN: &str = "#45f862";
    /// 24-bit colour of the amp block category.
    pub const AMP: &str = "#ff2727";
    /// 24-bit colour of the capture block category.
    pub const CAPTURE: &str = "#959595";
    /// 24-bit colour of the cab block category.
    pub const CAB: &str = "#6954ff";
    /// 24-bit colour of the overdrive block category.
    pub const OVERDRIVE: &str = "#ff7000";
    /// 24-bit colour of the delay block category.
    pub const DELAY: &str = "#00ffdd";
    /// 24-bit colour of the reverb block category.
    pub const REVERB: &str = "#00ffdd";
    /// 24-bit colour of the compressor block category.
    pub const COMPRESSOR: &str = "#45f862";
    /// 24-bit colour of the pitch block category.
    pub const PITCH: &str = "#ffd236";
    /// 24-bit colour of the modulation block category.
    pub const MODULATION: &str = "#3500f1";
    /// 24-bit colour of the morph block category.
    pub const MORPH: &str = "#87daff";
    /// 24-bit colour of the synth block category.
    pub const SYNTH: &str = "#e44a5d";
    /// 24-bit colour of the filter block category.
    pub const FILTER: &str = "#87daff";
    /// 24-bit colour of the equalizer block category.
    pub const EQUALIZER: &str = "#0a74e0";
    /// 24-bit colour of the IR loader block category.
    pub const IR_LOADER: &str = "#6954ff";
    /// 24-bit colour of the wah block category.
    pub const WAH: &str = "#959595";
    /// 24-bit colour of the FX loop block category.
    pub const FX_LOOP: &str = "#959595";
    /// 24-bit colour of the looper block category.
    pub const LOOPER: &str = "#ff2727";
    /// 24-bit colour of the utility block category.
    pub const UTILITY: &str = "#959595";
    /// Colour of a footswitch lamp lit white.
    pub const WHITE_LED: &str = "#f4f4f4";
    /// Colour of a footswitch lamp with nothing assigned to light it.
    pub const IDLE_LED: &str = "#626367";
    /// Highlight tone of the metal palette used for hardware surfaces.
    pub const METAL_HIGHLIGHT: &str = "#c5c6c8";
    /// Mid tone of the metal palette used for hardware surfaces.
    pub const METAL_MID: &str = "#8f9194";
    /// Shadow tone of the metal palette used for hardware surfaces.
    pub const METAL_SHADOW: &str = "#222327";
}
/// Input routes as `(route id, label)` pairs, in contract order.
/// Mono sources: ids 1, 2, 4, 5, 8, 9, 10, 11.
/// Stereo pairs: ids 3, 6, 12, 13.
/// Other: ids 7 ("Prev. Row"), 0 ("Not In Use").
pub const INPUT_ROUTES: &[(u32, &str)] = &[
    (1, "In 1"),
    (2, "In 2"),
    (4, "Return 1"),
    (5, "Return 2"),
    (8, "USB 5"),
    (9, "USB 6"),
    (10, "USB 7"),
    (11, "USB 8"),
    (3, "In 1/2"),
    (6, "Return 1/2"),
    (12, "USB 5/6"),
    (13, "USB 7/8"),
    (7, "Prev. Row"),
    (0, "Not In Use"),
];
/// Output routes as `(route id, label)` pairs, in contract order.
/// Stereo pairs: ids 19, 1, 2, 3, 22, 14, 15.
/// Mono outputs: ids 4, 5, 6, 7, 8, 9, 20, 21, 10, 11, 12, 13.
/// Other: ids 16 ("Row 3"), 17 ("Row 4"), 18 ("Row 3/4"), 0 ("Internal").
pub const OUTPUT_ROUTES: &[(u32, &str)] = &[
    (19, "Multi Out"),
    (1, "Out 1/2"),
    (2, "Out 3/4"),
    (3, "Send 1/2"),
    (22, "USB 3/4"),
    (14, "USB 5/6"),
    (15, "USB 7/8"),
    (4, "Out 1"),
    (5, "Out 2"),
    (6, "Out 3"),
    (7, "Out 4"),
    (8, "Send 1"),
    (9, "Send 2"),
    (20, "USB 3"),
    (21, "USB 4"),
    (10, "USB 5"),
    (11, "USB 6"),
    (12, "USB 7"),
    (13, "USB 8"),
    (16, "Row 3"),
    (17, "Row 4"),
    (18, "Row 3/4"),
    (0, "Internal"),
];
/// Input route ids as `(route id, wire enum name)` pairs: every id the wire schema declares for an input port except its count marker. It holds ids that are not in `INPUT_ROUTES`, because the wire declares them without making them selectable. Reading a Grid row's input id with this numbering is this library's assumption.
pub const INPUT_ROUTE_WIRE_NAMES: &[(u32, &str)] = &[
    (0, "EMPTY"),
    (1, "INPUT_1"),
    (2, "INPUT_2"),
    (3, "INPUT_1_2"),
    (4, "RETURN_1"),
    (5, "RETURN_2"),
    (6, "RETURN_1_2"),
    (7, "PREV_ROW"),
    (8, "USB_IN_5"),
    (9, "USB_IN_6"),
    (10, "USB_IN_7"),
    (11, "USB_IN_8"),
    (12, "USB_IN_5_6"),
    (13, "USB_IN_7_8"),
    (14, "SIDECHAIN_BUFFER"),
];
/// Output route ids as `(route id, wire enum name)` pairs: every id the wire schema declares for an output port except its count marker. Reading a Grid row's output id with this numbering is this library's assumption.
pub const OUTPUT_ROUTE_WIRE_NAMES: &[(u32, &str)] = &[
    (0, "EMPTY"),
    (1, "XLR_1_2"),
    (2, "OUTPUT_3_4"),
    (3, "SEND_1_2"),
    (4, "XLR_1"),
    (5, "XLR_2"),
    (6, "OUTPUT_3"),
    (7, "OUTPUT_4"),
    (8, "SEND_1"),
    (9, "SEND_2"),
    (10, "USB_OUT_5"),
    (11, "USB_OUT_6"),
    (12, "USB_OUT_7"),
    (13, "USB_OUT_8"),
    (14, "USB_OUT_5_6"),
    (15, "USB_OUT_7_8"),
    (16, "NEXT_ROW_3"),
    (17, "NEXT_ROW_4"),
    (18, "NEXT_ROW_3_4"),
    (19, "MULTIPLE_OUTS"),
    (20, "USB_OUT_3"),
    (21, "USB_OUT_4"),
    (22, "USB_OUT_3_4"),
];
/// One block-category classification rule: `(palette key, category terms, category words,
/// model-name terms)`. A term matches as a substring of the lowercased category string
/// or model name; a word matches a whole whitespace-separated word of the category.
pub type CategoryRule = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
);
/// The ordered block-category rules [`category_key`] applies. The first matching rule wins,
/// and the final `utility` rule is also the fallback for an unmatched block.
pub const CATEGORY_RULES: &[CategoryRule] = &[
    ("plugin", &["plugin"], &[], &[]),
    ("capture", &["neural capture", "capture"], &[], &[]),
    ("amp", &["amplifier"], &["amp"], &[]),
    ("looper", &["looper"], &[], &[]),
    ("irLoader", &["ir loader", "irloader"], &[], &[]),
    ("cab", &["cab", "impulse response"], &[], &[]),
    (
        "overdrive",
        &["overdrive", "distortion", "drive", "boost", "fuzz"],
        &[],
        &[],
    ),
    ("delay", &["delay"], &[], &[]),
    ("reverb", &["reverb"], &[], &[]),
    ("compressor", &["compressor"], &[], &[]),
    ("pitch", &["pitch"], &[], &["octav"]),
    (
        "modulation",
        &[
            "modulation",
            "chorus",
            "flanger",
            "phaser",
            "tremolo",
            "vibrato",
        ],
        &["mod"],
        &[],
    ),
    ("morph", &["morph"], &[], &[]),
    ("synth", &["synth"], &[], &[]),
    ("filter", &["filter"], &[], &[]),
    ("equalizer", &["equalizer"], &["eq"], &[]),
    ("wah", &["wah"], &[], &[]),
    ("fxLoop", &["fx loop", "effects loop"], &[], &[]),
    ("utility", &["utility"], &[], &["gate"]),
];

/// Classify a block by its CorOS category string.
///
/// Emitted from the same contract rules as the TypeScript qcCategoryKey and the
/// Python category_key, so the native runtime's lamp and the app's Grid ink
/// cannot disagree about which category a block is.
pub fn category_key(category: &str, name: &str) -> &'static str {
    let haystack = category.to_ascii_lowercase();
    let model = name.to_ascii_lowercase();
    for (key, terms, words, name_terms) in CATEGORY_RULES {
        if terms.iter().any(|term| haystack.contains(term))
            || words
                .iter()
                .any(|word| haystack.split_whitespace().any(|part| part == *word))
            || name_terms.iter().any(|term| model.contains(term))
        {
            return key;
        }
    }
    "utility"
}

/// The 24-bit colour CorOS stores for a block's category.
pub fn category_color(category: &str, name: &str) -> &'static str {
    match category_key(category, name) {
        "plugin" => visual_colors::PLUGIN,
        "capture" => visual_colors::CAPTURE,
        "amp" => visual_colors::AMP,
        "looper" => visual_colors::LOOPER,
        "irLoader" => visual_colors::IR_LOADER,
        "cab" => visual_colors::CAB,
        "overdrive" => visual_colors::OVERDRIVE,
        "delay" => visual_colors::DELAY,
        "reverb" => visual_colors::REVERB,
        "compressor" => visual_colors::COMPRESSOR,
        "pitch" => visual_colors::PITCH,
        "modulation" => visual_colors::MODULATION,
        "morph" => visual_colors::MORPH,
        "synth" => visual_colors::SYNTH,
        "filter" => visual_colors::FILTER,
        "equalizer" => visual_colors::EQUALIZER,
        "wah" => visual_colors::WAH,
        "fxLoop" => visual_colors::FX_LOOP,
        _ => visual_colors::UTILITY,
    }
}
