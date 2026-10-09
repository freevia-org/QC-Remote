// Generated from the private maintainer specification and public contract snapshots; do not edit by hand.
/// Prefix used to classify a refusal while the device is suspended.
pub const DEVICE_SUSPENDED_MARKER: &str = "Quad Cortex has suspended host control";
/// An earlier frame was sent; the request effect remains uncertain.
pub const PARTIAL_REQUEST_SUFFIX: &str =
    "an earlier frame of this request was already sent, so the device effect may be uncertain";
/// Why explicit catalog refresh is unavailable on this host.
pub const PRESET_CATALOG_REFRESH_UNAVAILABLE: &str = "The Quad Cortex publishes its complete preset directory once at the start of each USB session and then pushes File updates as the library changes. This host serves that retained directory and does not send an explicit directory refresh: the unit does answer a directory read when asked (observed on CorOS 4.1.0, 2026-10-07), but a refresh is not implemented here. Omit refresh to read the retained directory, or reconnect the device session to pull a new one.";
/// Readable GeneralSettings booleans and their write refusal reasons.
pub const READ_ONLY_GENERAL_TOGGLES: &[(&str, &str)] = &[(
    "internalMidiClockEnabled",
    "writes to it are refused by the device",
)];
