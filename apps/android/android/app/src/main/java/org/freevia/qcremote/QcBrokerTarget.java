package org.freevia.qcremote;

import java.util.Objects;
import java.util.UUID;
import java.util.function.LongPredicate;
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
        long integer = generation(target, 1);
        return new QcBrokerTarget((String) source, (String) session, integer);
    }

    static long generation(JSONObject envelope, long minimum) {
        Object value = envelope.opt("generation");
        if (!(value instanceof Number)) throw new IllegalArgumentException("The generation must be a number.");
        double numeric = ((Number) value).doubleValue();
        long integer = ((Number) value).longValue();
        if (!Double.isFinite(numeric) || numeric != integer || integer < minimum || integer > 9007199254740991L)
            throw new IllegalArgumentException("The generation must be a safe integer.");
        return integer;
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
        private String selectedSource = "physical";
        private long intentRevision;
        private String owner;
        private long revision;
        private long generation;
        private QcBrokerTarget selected;

        synchronized String selectedSource() { return selectedSource; }
        synchronized boolean demoSelected() { return "demo".equals(selectedSource); }
        synchronized long intentRevision() { return intentRevision; }

        /** Restore a durable user choice, or apply it after its durable commit. */
        synchronized void restoreIntent(String source, long revision) {
            if (!("demo".equals(source) || "physical".equals(source))
                    || revision < 0 || revision > 9007199254740991L)
                throw new IllegalArgumentException("The broker selection is invalid.");
            selectedSource = source;
            intentRevision = revision;
            clear();
        }

        synchronized long commitIntent(String source, LongPredicate persist) {
            if (!("demo".equals(source) || "physical".equals(source)))
                throw new IllegalArgumentException("The broker selection is invalid.");
            long revision = intentRevision + 1;
            if (revision > 9007199254740991L)
                throw new IllegalStateException("The broker selection could not be saved.");
            // Closing a physical route is immediate. A failed durable save must
            // not leave fresh requests eligible for the old physical target.
            if ("demo".equals(source)) restoreIntent(source, revision);
            if (!persist.test(revision))
                throw new IllegalStateException("The broker selection could not be saved.");
            if (!"demo".equals(source)) restoreIntent(source, revision);
            return revision;
        }

        /** Availability can retire an identity; it never changes the user's choice. */
        synchronized QcBrokerTarget selectAvailable(boolean physicalReady, String physicalOwner, long physicalRevision,
                boolean demoReady, String demoOwner, long demoRevision) {
            if (demoSelected()) return select(demoReady ? "demo" : null, demoOwner, demoRevision);
            return select(physicalReady ? "physical" : null, physicalOwner, physicalRevision);
        }

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
