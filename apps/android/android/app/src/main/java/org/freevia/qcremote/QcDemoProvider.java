package org.freevia.qcremote;

import java.util.HashMap;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import java.util.function.BooleanSupplier;
import java.util.function.Consumer;
import java.util.function.LongSupplier;
import org.json.JSONObject;

/** A foreground WebView owns the demo. Requests never fall through to USB. */
final class QcDemoProvider {
    private final BooleanSupplier physicalReady;
    private final LongSupplier physicalGeneration;
    private final Map<String, Pending> pending = new HashMap<>();
    private String sessionId;
    private long providerGeneration;
    private long sequence;
    private boolean foreground = true;
    private static final int MAX_PENDING = 8;

    private static final class Pending {
        final String session;
        final long generation;
        final CompletableFuture<JSONObject> result = new CompletableFuture<>();
        Pending(String session, long generation) { this.session = session; this.generation = generation; }
    }

    QcDemoProvider(BooleanSupplier physicalReady, LongSupplier physicalGeneration) {
        this.physicalReady = physicalReady;
        this.physicalGeneration = physicalGeneration;
    }

    void publish(String session, long generation) {
        Pending[] held;
        synchronized (this) {
            if (java.util.Objects.equals(sessionId, session) && providerGeneration == generation) return;
            held = drain();
            sessionId = session;
            providerGeneration = generation;
            sequence = 0;
        }
        retire(held);
    }

    void foreground(boolean value) {
        Pending[] held;
        synchronized (this) {
            held = foreground != value ? drain() : new Pending[0];
            foreground = value;
        }
        retire(held);
    }

    synchronized boolean available() {
        return foreground && sessionId != null && !physicalReady.getAsBoolean();
    }

    synchronized boolean canDeliver(String id) {
        Pending request = pending.get(id);
        return request != null && available() && request.session.equals(sessionId)
            && request.generation == physicalGeneration.getAsLong();
    }

    CompletableFuture<JSONObject> invoke(String method, JSONObject params,
            JSONObject expected, Consumer<JSONObject> emit) {
        String id = UUID.randomUUID().toString();
        Pending request;
        JSONObject envelope;
        synchronized (this) {
            if (!available()) return failed("NOT_CONNECTED", "The foreground demo provider is unavailable.");
            if (pending.size() >= MAX_PENDING) return failed("BUSY", "The demo provider is at its request limit.");
            request = new Pending(sessionId, physicalGeneration.getAsLong());
            try {
                envelope = new JSONObject().put("id", id).put("sessionId", sessionId)
                    .put("generation", providerGeneration).put("sequence", ++sequence)
                    .put("method", method).put("params", params).put("expectedState", expected);
            } catch (Exception malformed) {
                return failed("INVALID_PARAMS", "The demo request could not be encoded.");
            }
            pending.put(id, request);
        }
        try {
            emit.accept(envelope);
        } catch (Exception error) {
            synchronized (this) { pending.remove(id); }
            request.result.completeExceptionally(error);
        }
        return request.result;
    }

    void complete(String id, String session, long generation, JSONObject result, JSONObject error) {
        Pending request;
        boolean current;
        synchronized (this) {
            request = pending.get(id);
            if (request == null || !request.session.equals(session) || providerGeneration != generation) return;
            pending.remove(id);
            current = available() && request.session.equals(sessionId)
                && request.generation == physicalGeneration.getAsLong();
        }
        if (!current) {
            request.result.completeExceptionally(changed());
        } else if (error != null) {
            request.result.completeExceptionally(new RelayException(
                error.optString("code", "DEVICE_ERROR"), error.optString("message", "The demo request failed.")));
        } else if (result == null) {
            request.result.completeExceptionally(new RelayException("MALFORMED_RESPONSE", "The demo returned no result."));
        } else request.result.complete(result);
    }

    void expire(CompletableFuture<JSONObject> invocation) {
        synchronized (this) {
            String key = null;
            for (Map.Entry<String, Pending> entry : pending.entrySet())
                if (entry.getValue().result == invocation) { key = entry.getKey(); break; }
            if (key == null) return;
            pending.remove(key);
        }
        invocation.completeExceptionally(new RelayException("DEVICE_OUTCOME_UNKNOWN", "The demo request expired; do not replay it."));
    }

    private Pending[] drain() {
        Pending[] held = pending.values().toArray(new Pending[0]);
        pending.clear();
        return held;
    }

    private static void retire(Pending[] held) {
        for (Pending request : held) request.result.completeExceptionally(changed());
    }

    private static RelayException changed() {
        return new RelayException("DEVICE_IDENTITY_MISMATCH", "The active demo or physical device changed; read the current target again.");
    }

    private static CompletableFuture<JSONObject> failed(String code, String message) {
        CompletableFuture<JSONObject> result = new CompletableFuture<>();
        result.completeExceptionally(new RelayException(code, message));
        return result;
    }
}
