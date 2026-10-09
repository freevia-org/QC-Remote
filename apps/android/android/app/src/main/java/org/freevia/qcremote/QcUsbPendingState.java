package org.freevia.qcremote;

import java.util.concurrent.atomic.AtomicBoolean;

/** Passive operation records owned by QcUsbPlugin's asynchronous lanes. */
final class PendingBackup {
    final String name;
    final String admission;
    final long createdAt = System.currentTimeMillis();
    volatile int chunks;
    volatile int ignoredPrefixChunks;
    volatile int rawReports;
    volatile int decodedMessages;
    volatile int lastRawReportBytes;
    volatile boolean recoveryStarted;
    volatile org.json.JSONObject savedResult;

    PendingBackup(String name, String admission) {
        this.name = name;
        this.admission = admission;
    }
}

final class PendingReady {
    final String detail;
    final String backupOrigin;
    volatile boolean openingStarted;

    PendingReady(String detail) { this(detail, null); }

    PendingReady(String detail, String backupOrigin) {
        this.detail = detail;
        this.backupOrigin = backupOrigin;
    }
}

final class PendingGatewayRead {
    final QcNativeStateDecoder.PlannedGatewayRead plan;
    final long startedAtMs;
    final String scope;
    volatile long afterMessageSequence = Long.MAX_VALUE;

    PendingGatewayRead(QcNativeStateDecoder.PlannedGatewayRead plan, String scope, long startedAtMs) {
        this.plan = plan;
        this.scope = scope;
        this.startedAtMs = startedAtMs;
    }
}

final class PendingGatewayTransaction {
    final QcNativeStateDecoder.PlannedGatewayWrite plan;
    volatile long afterSequence;
    private long relevantSequence;
    volatile String admission;
    final long hostStartedAtUnixMs;
    final AtomicBoolean wakeScheduled = new AtomicBoolean();
    volatile long verificationId;
    volatile long dispatchLatencyMs;
    volatile boolean dispatched;

    PendingGatewayTransaction(
        QcNativeStateDecoder.PlannedGatewayWrite plan, long afterSequence,
        long hostStartedAtUnixMs
    ) {
        this.plan = plan;
        this.afterSequence = afterSequence;
        this.relevantSequence = afterSequence;
        this.hostStartedAtUnixMs = hostStartedAtUnixMs;
    }
    synchronized void observeAdmitted(QcObservationFence observation, java.util.List<com.getcapacitor.JSObject> states) {
        for (com.getcapacitor.JSObject state : states) {
            if (plan.observedStateKinds.contains(state.getString("kind", ""))) {
                relevantSequence = Math.max(relevantSequence, observation.sequence);
                return;
            }
        }
    }

    synchronized long observationSequence() { return relevantSequence; }

}

final class PendingCatalogRead {
    final String method;
    final String key;
    final String context;
    final long startedAtMs;
    final long timeoutMs;

    PendingCatalogRead(String method, String key, String context, long startedAtMs, long timeoutMs) {
        this.method = method;
        this.key = key;
        this.context = context;
        this.startedAtMs = startedAtMs;
        this.timeoutMs = timeoutMs;
    }
}

final class StatePublicationFailure extends RuntimeException {
    final long generation;
    StatePublicationFailure(long generation, Exception cause) {
        super(cause);
        this.generation = generation;
    }
}
