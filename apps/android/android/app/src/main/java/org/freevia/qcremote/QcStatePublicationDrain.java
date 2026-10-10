package org.freevia.qcremote;

import java.util.concurrent.Executor;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.BooleanSupplier;
import java.util.function.Consumer;

/** One coalesced wakeup for a native queue, with no payload retained in the executor. */
final class QcStatePublicationDrain implements AutoCloseable {
    interface Work { void run() throws Exception; }

    private final Executor executor;
    private final Work work;
    private final BooleanSupplier ready;
    private final Consumer<Exception> failed;
    private final AtomicBoolean scheduled = new AtomicBoolean(false);
    private final AtomicBoolean closed = new AtomicBoolean(false);

    QcStatePublicationDrain(Executor executor, Work work, BooleanSupplier ready, Consumer<Exception> failed) {
        this.executor = executor;
        this.work = work;
        this.ready = ready;
        this.failed = failed;
    }

    void request() {
        if (closed.get() || !scheduled.compareAndSet(false, true)) return;
        try {
            executor.execute(() -> {
                boolean completed = false;
                try {
                    if (!closed.get()) { work.run(); completed = true; }
                } catch (Exception error) {
                    if (!closed.get()) failed.accept(error);
                } finally {
                    scheduled.set(false);
                    // A reduction can arrive after an empty pull while this
                    // wakeup still owns the flag. Recheck after releasing it.
                    if (completed && !closed.get()) {
                        try { if (ready.getAsBoolean()) request(); }
                        catch (Exception error) { failed.accept(error); }
                    }
                }
            });
        } catch (java.util.concurrent.RejectedExecutionException error) {
            scheduled.set(false);
            if (!closed.get()) failed.accept(error);
        }
    }

    @Override public void close() { closed.set(true); }
}
