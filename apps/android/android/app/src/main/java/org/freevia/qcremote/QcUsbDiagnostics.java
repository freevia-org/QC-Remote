package org.freevia.qcremote;

import java.util.Collections;
import java.util.HashMap;
import java.util.Map;

/** Short-lock, immutable snapshots of related USB report and message counters.
 * Connection phase, decoder state and transport timing live behind other owners;
 * they are sampled independently by the plugin and are not part of this atomic group.
 */
final class QcUsbDiagnostics {
    /** Last failed startup only; retained across retries, without device content. */
    static final class StartupFailure {
        final long sessionEpoch;
        final String detail;

        private StartupFailure(long sessionEpoch, String detail) {
            this.sessionEpoch = sessionEpoch;
            this.detail = detail;
        }
    }

    static final class Snapshot {
        final long sessionEpoch;
        final long messagesReceived;
        final Map<Integer, Long> messagesReceivedByType;
        final long rawReportsReceived;
        final long messagesSent;
        final Map<Integer, Long> messagesSentByType;
        final StartupFailure startupFailure;

        private Snapshot(long sessionEpoch, long messagesReceived,
                Map<Integer, Long> messagesReceivedByType, long rawReportsReceived,
                long messagesSent, Map<Integer, Long> messagesSentByType,
                StartupFailure startupFailure) {
            this.sessionEpoch = sessionEpoch;
            this.messagesReceived = messagesReceived;
            this.messagesReceivedByType = Collections.unmodifiableMap(messagesReceivedByType);
            this.rawReportsReceived = rawReportsReceived;
            this.messagesSent = messagesSent;
            this.messagesSentByType = Collections.unmodifiableMap(messagesSentByType);
            this.startupFailure = startupFailure;
        }
    }

    private long sessionEpoch;
    private long messagesReceived;
    private final Map<Integer, Long> messagesReceivedByType = new HashMap<>();
    private long rawReportsReceived;
    private long messagesSent;
    private final Map<Integer, Long> messagesSentByType = new HashMap<>();
    private StartupFailure startupFailure;

    synchronized long beginSession() {
        sessionEpoch++;
        messagesReceived = 0;
        messagesReceivedByType.clear();
        rawReportsReceived = 0;
        messagesSent = 0;
        messagesSentByType.clear();
        return sessionEpoch;
    }

    synchronized boolean isCurrentSession(long epoch) { return epoch == sessionEpoch; }

    synchronized StartupFailure startupFailed(long epoch, String detail) {
        if (epoch != sessionEpoch || (startupFailure != null && startupFailure.sessionEpoch == epoch)) return null;
        startupFailure = new StartupFailure(epoch, detail);
        return startupFailure;
    }

    synchronized void rawReportReceived(long epoch) {
        if (epoch == sessionEpoch) rawReportsReceived++;
    }

    synchronized void messageReceived(long epoch, int messageType) {
        if (epoch != sessionEpoch) return;
        messagesReceived++;
        messagesReceivedByType.merge(messageType, 1L, Long::sum);
    }

    synchronized void messageSent(long epoch, int messageType) {
        if (epoch != sessionEpoch) return;
        messagesSent++;
        messagesSentByType.merge(messageType, 1L, Long::sum);
    }

    synchronized Snapshot snapshot() {
        return new Snapshot(sessionEpoch, messagesReceived,
            new HashMap<>(messagesReceivedByType), rawReportsReceived,
            messagesSent, new HashMap<>(messagesSentByType), startupFailure);
    }
}
