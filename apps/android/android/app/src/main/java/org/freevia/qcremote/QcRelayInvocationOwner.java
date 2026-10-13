package org.freevia.qcremote;

import java.util.LinkedHashMap;
import java.util.Map;

/** Tracks only this relay service's operations until their native work retires. */
final class QcRelayInvocationOwner<T> {
    private final Map<T, Runnable> expirations = new LinkedHashMap<>();

    synchronized void track(T invocation, Runnable expire) { expirations.put(invocation, expire); }
    synchronized void retire(T invocation) { expirations.remove(invocation); }
    synchronized int size() { return expirations.size(); }

    synchronized void expireAll() {
        // Expiry can synchronously complete an invocation and retire its entry.
        for (Runnable expire : new java.util.ArrayList<>(expirations.values())) expire.run();
    }
}
