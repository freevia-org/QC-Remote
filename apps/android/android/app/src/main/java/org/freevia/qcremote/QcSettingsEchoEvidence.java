package org.freevia.qcremote;

/** Raw settings evidence, separate from the merged presentation cache. Caller holds stateEventLock. */
final class QcSettingsEchoEvidence {
    static final class Fence { String operation; }
    static final class Echo {
        final QcObservationFence observation;
        final String observationTicket;
        final byte[] payload;
        Echo(QcObservationFence observation, String ticket, byte[] payload) {
            this.observation = observation; this.observationTicket = ticket; this.payload = payload.clone();
        }
    }
    private final java.util.Map<Integer, Echo> latest = new java.util.HashMap<>();

    void observe(int messageType, String ticket, byte[] payload) {
        QcObservationFence observation = new QcObservationFence(ticket);
        Echo previous = latest.get(messageType);
        if (previous != null && previous.observation.sameSession(observation)
            && previous.observation.sequence >= observation.sequence) return;
        latest.put(messageType, new Echo(observation, ticket, payload));
    }
    Echo candidate(int messageType, Fence fence) {
        Echo echo = latest.get(messageType);
        return fence.operation != null ? echo : null;
    }
}
