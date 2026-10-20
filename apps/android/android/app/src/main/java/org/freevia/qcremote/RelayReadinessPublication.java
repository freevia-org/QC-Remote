package org.freevia.qcremote;

import java.util.function.BooleanSupplier;

/** One connection's last successfully queued readiness state. The service
 * serializes access and resets this on every new WebSocket connection.
 * Keepalive belongs to OkHttp's protocol ping, not repeated JSON messages. */
final class RelayReadinessPublication {
    private Boolean published;

    boolean publish(boolean ready, BooleanSupplier send) {
        if (published != null && published == ready) return false;
        if (!send.getAsBoolean()) return false;
        published = ready;
        return true;
    }

    void reset() { published = null; }
}
