package org.freevia.qcremote;

/** Linearizes relay deadline expiry against read and mutation dispatch admission. */
final class QcRelayDeadlineState {
    static final int QUEUED = 0;
    static final int READ_ADMITTED = 1;
    static final int MUTATION_ADMITTED = 2;
    static final int EXPIRED_BEFORE_ADMISSION = 3;
    static final int EXPIRED_AFTER_READ = 4;
    static final int EXPIRED_AFTER_MUTATION = 5;

    private int state = QUEUED;

    synchronized boolean beginRead() {
        if (state >= EXPIRED_BEFORE_ADMISSION) return false;
        if (state == QUEUED) state = READ_ADMITTED;
        return true;
    }

    synchronized boolean beginMutation() {
        if (state >= EXPIRED_BEFORE_ADMISSION) return false;
        state = MUTATION_ADMITTED;
        return true;
    }

    /** Returns the pre-expiry state so the owner quarantines only mutations. */
    synchronized int expire() {
        int previous = state;
        if (previous < EXPIRED_BEFORE_ADMISSION) {
            state = previous == QUEUED ? EXPIRED_BEFORE_ADMISSION
                : previous == READ_ADMITTED ? EXPIRED_AFTER_READ : EXPIRED_AFTER_MUTATION;
        }
        return previous;
    }

    synchronized boolean isExpired() { return state >= EXPIRED_BEFORE_ADMISSION; }
}
