package org.freevia.qcremote;

import org.json.JSONObject;

/** Versioned phone/relay protocol generated from the shared MCP action contract. */
final class RelayProtocol {
    static final String VERSION = GeneratedRelayProfile.PROTOCOL_VERSION;

    static JSONObject result(String id, JSONObject value) throws Exception {
        return new JSONObject().put("type", "result").put("id", id).put("ok", true).put("result", value);
    }

    static JSONObject error(String id, RelayException failure) throws Exception {
        JSONObject frame = error(id, failure.code, failure.getMessage(), failure.retryable());
        JSONObject details = new JSONObject();
        if (failure.requestSent != null) details.put("requestSent", failure.requestSent.booleanValue());
        if (failure.writesQuarantined != null) details.put("writesQuarantined", failure.writesQuarantined.booleanValue());
        if (details.length() > 0) frame.getJSONObject("error").put("details", details);
        return frame;
    }

    static JSONObject error(String id, String code, String message, boolean retryable) throws Exception {
        JSONObject error = new JSONObject().put("code", code).put("message", message).put("retryable", retryable);
        Boolean userActionRequired = RelayException.userActionRequired(code);
        if (userActionRequired != null) error.put("userActionRequired", userActionRequired.booleanValue());
        return new JSONObject().put("type", "result").put("id", id).put("ok", false).put("error", error);
    }

    static boolean isAllowed(String method) { return GeneratedRemoteActions.contains(method); }
    static boolean isReadOnly(String method) { return GeneratedRemoteActions.isReadOnly(method); }
    static boolean requiresConfirmation(String method) { return GeneratedRemoteActions.requiresConfirmation(method); }

    private RelayProtocol() {}
}
