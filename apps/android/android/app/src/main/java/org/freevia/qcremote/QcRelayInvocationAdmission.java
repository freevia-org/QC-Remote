package org.freevia.qcremote;

import java.util.concurrent.atomic.AtomicInteger;

/** Bounds requests admitted to the Android device host. */
final class QcRelayInvocationAdmission {
    private final int maximum;
    private final AtomicInteger active = new AtomicInteger();

    QcRelayInvocationAdmission(int maximum) {
        if (maximum < 1) throw new IllegalArgumentException("maximum must be positive");
        this.maximum = maximum;
    }

    boolean tryAcquire() {
        while (true) {
            int current = active.get();
            if (current >= maximum) return false;
            if (active.compareAndSet(current, current + 1)) return true;
        }
    }

    void release() {
        while (true) {
            int current = active.get();
            if (current == 0) return;
            if (active.compareAndSet(current, current - 1)) return;
        }
    }

    int activeCount() { return active.get(); }
}
