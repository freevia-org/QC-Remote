package org.freevia.qcremote;

import java.util.Objects;
import java.util.UUID;
import org.json.JSONObject;

/** Broker-issued identity, deliberately distinct from the WebView registration. */
final class QcBrokerTarget {
    final String source;
    final String sessionId;
    final long generation;

    QcBrokerTarget(String source, String sessionId, long generation) {
        this.source = source;
        this.sessionId = sessionId;
        this.generation = generation;
    }

    static QcBrokerTarget parse(Object value) {
        if (!(value instanceof JSONObject)) throw new IllegalArgumentException("expectedBrokerTarget must be an object.");
        JSONObject target = (JSONObject) value;
        Object source = target.opt("source"), session = target.opt("sessionId"), generation = target.opt("generation");
        if (target.length() != 3 || !("physical".equals(source) || "demo".equals(source))
                || !(session instanceof String) || ((String) session).trim().isEmpty() || ((String) session).length() > 256
                || !(generation instanceof Number)) throw new IllegalArgumentException("The broker target is invalid.");
        double numeric = ((Number) generation).doubleValue();
        long integer = ((Number) generation).longValue();
        if (!Double.isFinite(numeric) || numeric != integer || integer < 1 || integer > 9007199254740991L)
            throw new IllegalArgumentException("The broker target generation is invalid.");
        return new QcBrokerTarget((String) source, (String) session, integer);
    }

    JSONObject json() {
        try { return new JSONObject().put("source", source).put("sessionId", sessionId).put("generation", generation); }
        catch (Exception impossible) { throw new IllegalStateException(impossible); }
    }

    boolean same(QcBrokerTarget other) {
        return other != null && source.equals(other.source) && sessionId.equals(other.sessionId) && generation == other.generation;
    }

    void assertCurrent(QcBrokerTarget current) {
        if (!same(current)) throw new RelayException("TARGET_SUPERSEDED", "The broker target changed before this operation was admitted.");
    }

    static final class Registry {
        private String owner;
        private long revision;
        private long generation;
        private QcBrokerTarget selected;

        synchronized QcBrokerTarget select(String source, String owner, long revision) {
            if (source == null) { clear(); return null; }
            if (selected != null && selected.source.equals(source) && Objects.equals(this.owner, owner)
                    && this.revision == revision) return selected;
            this.owner = owner;
            this.revision = revision;
            selected = new QcBrokerTarget(source, UUID.randomUUID().toString(), ++generation);
            return selected;
        }

        synchronized void clear() { selected = null; owner = null; }
    }
}
