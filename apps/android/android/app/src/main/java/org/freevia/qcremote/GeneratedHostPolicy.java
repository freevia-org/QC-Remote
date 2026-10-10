// Generated from the private maintainer specification and public contract snapshots; do not edit by hand.
package org.freevia.qcremote;

final class GeneratedHostPolicy {
    static final String DEVICE_SUSPENDED_MARKER = "Quad Cortex has suspended host control";
    static final String UNCERTAIN_DEVICE_EFFECT_MARKER = "device effect may be uncertain";
    static final String PRESET_CATALOG_REFRESH_UNAVAILABLE = "The Quad Cortex publishes its complete preset directory once at the start of each USB session and then pushes File updates as the library changes. This host serves that retained directory and does not send an explicit directory refresh: the unit does answer a directory read when asked (observed on CorOS 4.1.0, 2026-10-07), but a refresh is not implemented here. Omit refresh to read the retained directory, or reconnect the device session to pull a new one.";
    // Android host dispatch policy: only retained reads and lifecycle escape paths.
    // The Rust owner independently fences every actual device submission.
    static final String[] SUSPENSION_CACHE_DISPATCH_KINDS = {"SYSTEM", "RECONNECT", "DISCONNECT", "SNAPSHOT", "CURRENT_PRESET", "STATE_EVENTS", "TEMPO_CLOCK", "GENERAL_SETTINGS", "IO_SETTINGS", "MODELS", "RETAINED_CATALOG", "MASTER_VOLUME"};
    private GeneratedHostPolicy() {}
}
