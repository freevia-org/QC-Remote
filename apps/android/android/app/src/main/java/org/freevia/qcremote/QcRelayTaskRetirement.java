package org.freevia.qcremote;

import java.util.concurrent.CompletableFuture;

/** Tracks executor work independently from the relay response future. */
final class QcRelayTaskRetirement {
    private final Object lock = new Object();
    private final CompletableFuture<Void> completion = new CompletableFuture<>();
    private int outstandingTasks;
    private boolean operationFinished;

    boolean taskQueued() {
        synchronized (lock) {
            if (operationFinished || completion.isDone()) return false;
            outstandingTasks++;
            return true;
        }
    }

    void taskRetired() {
        synchronized (lock) {
            if (outstandingTasks > 0) outstandingTasks--;
            completeIfIdle();
        }
    }

    void operationFinished() {
        synchronized (lock) {
            operationFinished = true;
            completeIfIdle();
        }
    }

    CompletableFuture<Void> completion() { return completion; }

    private void completeIfIdle() {
        if (operationFinished && outstandingTasks == 0) completion.complete(null);
    }
}
