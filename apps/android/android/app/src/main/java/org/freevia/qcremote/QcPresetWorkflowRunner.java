package org.freevia.qcremote;

import java.util.concurrent.CompletableFuture;

/** Schedules owner actions independently of native USB execution and caller lifetime. */
final class QcPresetWorkflowRunner<R> implements Runnable {
    interface Advance<R> { Step<R> next() throws Exception; }
    interface Scheduler { void schedule(Runnable task, long delayMs); }
    static final class Step<R> {
        final boolean finished;
        final R result;
        final Runnable submit;
        final long delayMs;
        Step(boolean finished, R result, Runnable submit, long delayMs) {
            this.finished = finished;
            this.result = result;
            this.submit = submit;
            this.delayMs = delayMs;
        }
    }
    private final Advance<R> advance;
    private final Scheduler scheduler;
    private final Runnable cancelPreparation;
    private final CompletableFuture<R> result = new CompletableFuture<>();
    private boolean terminal;

    QcPresetWorkflowRunner(Advance<R> advance, Scheduler scheduler, Runnable cancelPreparation) {
        this.advance = advance;
        this.scheduler = scheduler;
        this.cancelPreparation = cancelPreparation;
    }

    CompletableFuture<R> start() { run(); return result; }

    @Override public void run() {
        if (terminal) return;
        try {
            Step<R> step = advance.next();
            if (step.finished) {
                terminal = true;
                result.complete(step.result);
                return;
            }
            if (step.submit != null) step.submit.run();
            // Caller cancellation does not revoke an already admitted native call.
            // Only a terminal owner result can stop driving settlement and cleanup.
            scheduler.schedule(this, Math.max(1L, step.delayMs));
        } catch (Exception error) {
            terminal = true;
            try { cancelPreparation.run(); }
            catch (Exception cancellationError) { error.addSuppressed(cancellationError); }
            result.completeExceptionally(error);
        }
    }
}
