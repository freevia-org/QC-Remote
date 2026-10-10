package org.freevia.qcremote;

/** Error code and retry policy shared by the USB plugin and relay service. */
final class RelayException extends RuntimeException {
    final String code;
    final Boolean requestSent;
    final Boolean writesQuarantined;

    RelayException(String code, String message) {
        this(code, message, null, null);
    }

    private RelayException(String code, String message, Boolean requestSent, Boolean writesQuarantined) {
        super(message);
        this.code = code;
        this.requestSent = requestSent;
        this.writesQuarantined = writesQuarantined;
    }

    static RelayException quarantinedBeforeDispatch() {
        return new RelayException("WRITE_QUARANTINED",
            "A previous device write has an unknown outcome. Writes are disabled until the QC reconnects and synchronizes.",
            Boolean.FALSE, Boolean.TRUE);
    }

    static Boolean userActionRequired(String code) {
        if ("UNSAVED_STATE_UNCONFIRMED".equals(code)) return Boolean.TRUE;
        if ("UNSAVED_STATE_NOT_YET_OBSERVED".equals(code)) return Boolean.FALSE;
        return null;
    }

    boolean retryable() {
        return "NOT_CONNECTED".equals(code) || "READBACK_TIMEOUT".equals(code)
            || "USB_CONNECT_FAILED".equals(code) || "DEVICE_TRANSPORT".equals(code)
            || "UNSAVED_STATE_NOT_YET_OBSERVED".equals(code);
    }
}
