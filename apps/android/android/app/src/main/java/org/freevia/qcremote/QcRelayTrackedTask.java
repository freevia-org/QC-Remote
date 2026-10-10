package org.freevia.qcremote;

import java.util.concurrent.atomic.AtomicBoolean;

/** Idempotent wrapper for an executor task that can be returned by shutdownNow. */
final class QcRelayTrackedTask implements Runnable {
    private final QcRelayTaskRetirement retirement;
    private final Runnable action;
    private final Runnable onCancelled;
    private final AtomicBoolean claimed = new AtomicBoolean();

    QcRelayTrackedTask(QcRelayTaskRetirement retirement, Runnable action, Runnable onCancelled) {
        this.retirement = retirement;
        this.action = action;
        this.onCancelled = onCancelled;
    }

    @Override public void run() {
        if (!claimed.compareAndSet(false, true)) return;
        try { action.run(); }
        finally { retire(); }
    }

    void cancelBeforeRun() {
        if (!claimed.compareAndSet(false, true)) return;
        try { if (onCancelled != null) onCancelled.run(); }
        finally { retire(); }
    }

    private void retire() {
        if (retirement != null) retirement.taskRetired();
    }
}
