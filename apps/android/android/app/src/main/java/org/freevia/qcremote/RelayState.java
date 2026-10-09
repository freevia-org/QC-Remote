package org.freevia.qcremote;

/** Typed relay connection lifecycle values exposed by the shared status contract. */
enum RelayState {
    STOPPED("stopped"),
    CONNECTING("connecting"),
    CONNECTED("connected"),
    RECONNECTING("reconnecting"),
    PAIRING_REQUIRED("pairing_required"),
    INVALID_ENDPOINT("invalid_endpoint");

    private final String wireValue;

    RelayState(String wireValue) { this.wireValue = wireValue; }

    String wireValue() { return wireValue; }

    static RelayState parse(String value) {
        for (RelayState state : values()) if (state.wireValue.equals(value)) return state;
        return STOPPED;
    }

    boolean ownsActiveSocket() {
        return this == CONNECTING || this == CONNECTED || this == RECONNECTING;
    }
}
