package org.freevia.qcremote;

import java.nio.charset.StandardCharsets;
import java.util.LinkedHashSet;
import java.util.Set;
import org.json.JSONObject;

/** Pure relay-frame admission and request-id replay handling. */
final class RelayFrameHandler {
    interface MethodPolicy { boolean allows(String method); }

    enum Kind { ACCEPT, ERROR, CLOSE }

    static final class Decision {
        final Kind kind;
        final String id;
        final String method;
        final JSONObject params;
        final JSONObject expected;
        final long deadlineMs;
        final int closeCode;
        final String closeReason;
        final String errorCode;
        final String errorMessage;
        final boolean retryable;

        private Decision(Kind kind, String id, String method, JSONObject params,
                JSONObject expected, long deadlineMs, int closeCode, String closeReason,
                String errorCode, String errorMessage, boolean retryable) {
            this.kind = kind;
            this.id = id;
            this.method = method;
            this.params = params;
            this.expected = expected;
            this.deadlineMs = deadlineMs;
            this.closeCode = closeCode;
            this.closeReason = closeReason;
            this.errorCode = errorCode;
            this.errorMessage = errorMessage;
            this.retryable = retryable;
        }

        static Decision accept(String id, String method, JSONObject params, JSONObject expected, long deadlineMs) {
            return new Decision(Kind.ACCEPT, id, method, params, expected, deadlineMs, 0, null, null, null, false);
        }
        static Decision error(String id, String code, String message, boolean retryable) {
            return new Decision(Kind.ERROR, id, null, null, null, 0, 0, null, code, message, retryable);
        }
        static Decision close(int code, String reason) {
            return new Decision(Kind.CLOSE, "", null, null, null, 0, code, reason, null, null, false);
        }
    }

    private final Set<String> completedIds = new LinkedHashSet<>();

    /**
     * Validation order is protocol behavior: valid IDs are remembered before
     * method/access/argument checks, so a rejected invocation cannot be replayed.
     */
    synchronized Decision inspect(String text, MethodPolicy methodAllowed, MethodPolicy accessAllowed) {
        if (text == null || text.getBytes(StandardCharsets.UTF_8).length > GeneratedRelayProfile.MAX_REQUEST_FRAME_BYTES) {
            return Decision.close(1009, GeneratedRelayProfile.MESSAGE_TOO_LARGE_TEXT);
        }
        final JSONObject request;
        final String id;
        try {
            request = new JSONObject(text);
            Object rawId = request.opt("id");
            if (!(rawId instanceof String)) throw new IllegalArgumentException("Request identifier must be a string.");
            id = (String) rawId;
            if (!(request.opt("type") instanceof String) || !"invoke".equals(request.opt("type")))
                throw new IllegalArgumentException("Unsupported message type.");
        } catch (Exception error) {
            return Decision.error("", "INVALID_REQUEST", GeneratedRelayProfile.INVALID_REQUEST_TEXT, false);
        }
        if (id.isEmpty() || id.length() > 128) {
            return Decision.error("", "INVALID_REQUEST", GeneratedRelayProfile.INVALID_REQUEST_ID_TEXT, false);
        }
        Object rawMethod = request.opt("method");
        if (completedIds.contains(id)) {
            return Decision.error(id, "REPLAYED_REQUEST", GeneratedRelayProfile.REPLAYED_REQUEST_TEXT, false);
        }
        completedIds.add(id);
        if (completedIds.size() > GeneratedRelayProfile.COMPLETED_REQUEST_CACHE_SIZE)
            completedIds.remove(completedIds.iterator().next());
        if (!(rawMethod instanceof String)) {
            return Decision.error(id, "INVALID_REQUEST", GeneratedRelayProfile.INVALID_REQUEST_TEXT, false);
        }
        String method = (String) rawMethod;
        if (!methodAllowed.allows(method)) {
            return Decision.error(id, "METHOD_NOT_ALLOWED", GeneratedRelayProfile.METHOD_NOT_ALLOWED_TEXT, false);
        }
        if (!accessAllowed.allows(method)) {
            return Decision.error(id, "ACCESS_MODE_RESTRICTED", GeneratedRelayProfile.ACCESS_MODE_RESTRICTED_TEXT, false);
        }
        Object rawParams = request.opt("params");
        Object rawExpected = request.opt("expectedState");
        final long deadlineMs;
        try { deadlineMs = requestDeadlineMs(request); }
        catch (IllegalArgumentException error) {
            return Decision.error(id, "INVALID_REQUEST", error.getMessage(), false);
        }
        if (rawParams != null && rawParams != JSONObject.NULL && !(rawParams instanceof JSONObject)) {
            return Decision.error(id, "INVALID_ARGUMENT", "params must be an object.", false);
        }
        if (rawExpected != null && rawExpected != JSONObject.NULL && !(rawExpected instanceof JSONObject)) {
            return Decision.error(id, "INVALID_ARGUMENT", "expectedState must be an object.", false);
        }
        return Decision.accept(id, method,
            (JSONObject) (rawParams == JSONObject.NULL ? null : rawParams),
            (JSONObject) (rawExpected == JSONObject.NULL ? null : rawExpected), deadlineMs);
    }

    static long requestDeadlineMs(JSONObject request) {
        Object value = request.opt("deadlineMs");
        if (value == null || value == JSONObject.NULL) return GeneratedRelayProfile.MAXIMUM_INVOKE_DEADLINE_MS;
        if (!(value instanceof Number)) throw new IllegalArgumentException("deadlineMs must be a positive integer.");
        double numeric = ((Number) value).doubleValue();
        long deadline = ((Number) value).longValue();
        if (!Double.isFinite(numeric) || numeric != deadline || deadline <= 0
            || deadline > GeneratedRelayProfile.MAXIMUM_INVOKE_DEADLINE_MS) {
            throw new IllegalArgumentException("deadlineMs is outside the supported request budget.");
        }
        return deadline;
    }
}
