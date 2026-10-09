package org.freevia.qcremote;

/** Short-held admission/lifecycle lock. Never held across USB or other blocking I/O. */
final class QcRelayAdmissionGuard {
    private long epoch;
    private boolean terminal;
    private boolean writesQuarantined;
    private String writeQuarantineReason;

    synchronized long epoch() { return epoch; }
    synchronized boolean isTerminal() { return terminal; }
    synchronized boolean writesQuarantined() { return writesQuarantined; }
    synchronized String writeQuarantineReason() { return writeQuarantineReason; }

    synchronized boolean beginRead(QcRelayDeadlineState deadline) {
        return !terminal && (deadline == null || deadline.beginRead());
    }

    synchronized boolean beginMutation(QcRelayDeadlineState deadline) {
        return !terminal && !writesQuarantined
            && (deadline == null || deadline.beginMutation());
    }

    synchronized boolean beginRecovery(QcRelayDeadlineState deadline) {
        return !terminal && (deadline == null || deadline.beginMutation());
    }

    synchronized int expire(QcRelayDeadlineState deadline) {
        int previous = deadline.expire();
        if (previous == QcRelayDeadlineState.MUTATION_ADMITTED) {
            writesQuarantined = true;
            writeQuarantineReason = "The relay deadline expired after a device mutation may have been sent; the outcome is unknown.";
        }
        return previous;
    }

    synchronized void quarantineWrites(String reason) {
        writesQuarantined = true;
        writeQuarantineReason = reason;
    }

    synchronized void resetForNewSession() {
        writesQuarantined = false;
        writeQuarantineReason = null;
    }

    synchronized void markTerminal() {
        terminal = true;
        epoch++;
    }

    synchronized boolean publish(long expectedEpoch, Runnable publish) {
        if (terminal || expectedEpoch != epoch) return false;
        publish.run();
        return true;
    }
}
