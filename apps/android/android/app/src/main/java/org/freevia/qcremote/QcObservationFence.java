package org.freevia.qcremote;

/** Native observation identity; publication scheduling cannot advance its sequence. */
final class QcObservationFence {
    final long connection, revision, sequence;
    final String sessionId;
    QcObservationFence(String ticket) {
        try {
            org.json.JSONObject value = new org.json.JSONObject(ticket);
            org.json.JSONObject epoch = value.optJSONObject("epoch");
            connection = value.getLong("connection");
            sequence = value.getLong("sequence");
            revision = epoch == null ? -1 : epoch.getLong("revision");
            sessionId = epoch == null ? null : epoch.getString("sessionId");
            if (connection < 0 || sequence < 0 || (epoch != null && (revision < 0 || sessionId.isEmpty())))
                throw new IllegalArgumentException("Invalid native observation fence");
        } catch (Exception error) {
            throw new IllegalArgumentException("Invalid native observation fence", error);
        }
    }
    boolean sameSession(QcObservationFence other) {
        return other != null && sessionId != null && connection == other.connection && revision == other.revision
            && sessionId.equals(other.sessionId);
    }
    boolean after(QcObservationFence other) { return sameSession(other) && sequence > other.sequence; }
}
