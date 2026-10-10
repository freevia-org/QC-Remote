package org.freevia.qcremote;

import java.util.concurrent.CompletableFuture;

/** Per-invocation deadline, device admission, and owned-task retirement state. */
final class RelayDeadline {
    private final QcUsbPlugin session;
    private final QcRelayDeadlineState state = new QcRelayDeadlineState();
    private final QcRelayTaskRetirement retirement = new QcRelayTaskRetirement();
    private volatile CompletableFuture<org.json.JSONObject> operation;
    private volatile boolean cancelledForShutdown;

    RelayDeadline(QcUsbPlugin session) { this.session = session; }

    QcUsbPlugin session() { return session; }
    QcRelayDeadlineState state() { return state; }
    QcRelayTaskRetirement retirementTracker() { return retirement; }

    boolean beginRead() { return state.beginRead(); }
    boolean beginMutation() { return state.beginMutation(); }

    void expire() { session.expireRelayDeadline(this); }

    boolean isExpired() { return state.isExpired(); }
    void attachOperation(CompletableFuture<org.json.JSONObject> value) {
        operation = value;
        if (cancelledForShutdown) value.completeExceptionally(
            new RelayException("NOT_CONNECTED", "QC Remote stopped before the relay operation completed."));
    }
    void cancelForShutdown() {
        cancelledForShutdown = true;
        // Reject deferred persistent-gate suppliers before they can plan or
        // admit work after teardown. Expiry shares the session admission guard
        // with read/mutation admission and quarantines an admitted write.
        expire();
        CompletableFuture<org.json.JSONObject> current = operation;
        if (current != null) current.completeExceptionally(
            new RelayException("NOT_CONNECTED", "QC Remote stopped before the relay operation completed."));
    }

    boolean taskQueued() { return retirement.taskQueued(); }
    void taskRetired() { retirement.taskRetired(); }
    void operationFinished() { retirement.operationFinished(); }
    CompletableFuture<Void> retirement() { return retirement.completion(); }
}
