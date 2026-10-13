package org.freevia.qcremote;

import java.util.function.BooleanSupplier;
import java.util.function.LongSupplier;

/** One handshake's timer; the caller holds the USB close/publication monitor when it runs. */
final class QcSilentSessionWatchdog implements Runnable {
    private final long generation;
    private final long baseline;
    private final LongSupplier currentGeneration;
    private final LongSupplier messagesReceived;
    private final BooleanSupplier ready;
    private final Runnable retire;

    QcSilentSessionWatchdog(long generation, long baseline, LongSupplier currentGeneration,
            LongSupplier messagesReceived, BooleanSupplier ready, Runnable retire) {
        this.generation = generation;
        this.baseline = baseline;
        this.currentGeneration = currentGeneration;
        this.messagesReceived = messagesReceived;
        this.ready = ready;
        this.retire = retire;
    }

    @Override public void run() {
        if (currentGeneration.getAsLong() != generation || !ready.getAsBoolean()
            || messagesReceived.getAsLong() != baseline) return;
        retire.run();
    }
}
