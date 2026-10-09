package org.freevia.qcremote;

import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.Set;

/**
 * Which relay requests the device's suspension refuses. While the Quad Cortex
 * holds the session suspended (a device backup or a reset is running on the
 * unit) nothing a client asks for may reach the wire. Status, session control
 * and reads served from the shared owner's retained state stay available. An
 * unlisted dispatch kind is refused: a new kind must be shown cache-served
 * before it is let through. The shared Rust owner enforces the same rule at
 * its wire boundary; this gate gives the relay caller the typed refusal first.
 */
final class QcSuspensionGate {
    static final String CODE = "DEVICE_SUSPENDED";

    private static final Set<String> ALLOWED = Collections.unmodifiableSet(new HashSet<>(Arrays.asList(
        GeneratedHostPolicy.SUSPENSION_CACHE_DISPATCH_KINDS)));

    private QcSuspensionGate() {}

    static boolean refuses(String dispatchKind) {
        return !ALLOWED.contains(dispatchKind);
    }

    /** Every refusal the shared owner produces starts with this text. */
    static final String MARKER = GeneratedHostPolicy.DEVICE_SUSPENDED_MARKER;

    /**
     * The relay code for a failure: {@link #CODE} when the shared owner refused
     * the request before anything was sent, otherwise {@code fallback}. A
     * refusal that says part of the request already left keeps the fallback,
     * because its outcome is genuinely unknown.
     */
    static String code(Throwable error, String fallback) {
        String message = error == null ? null : error.getMessage();
        if (message != null && message.contains(MARKER)
            && !message.contains(GeneratedHostPolicy.UNCERTAIN_DEVICE_EFFECT_MARKER)) return CODE;
        return fallback;
    }
}
