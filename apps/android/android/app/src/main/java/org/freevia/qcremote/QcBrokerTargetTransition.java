package org.freevia.qcremote;

import org.json.JSONObject;

/** Only native lifecycle evidence may acknowledge a deliberate target change. */
final class QcBrokerTargetTransition {
    static JSONObject verified(String method, QcBrokerTarget from, QcBrokerTarget to,
            String oldPrivateKey, String newPrivateKey, boolean oldControllerClosed) {
        if (from == null || !"physical".equals(from.source) || from.same(to) || !oldControllerClosed) return null;
        String verification;
        if ("device.disconnect".equals(method)) {
            if (to != null && !"demo".equals(to.source)) return null;
            verification = "closed_native_controller";
        } else if ("device.resetSession".equals(method) || "device.reconnect".equals(method)) {
            if (to == null || !"physical".equals(to.source) || oldPrivateKey == null || oldPrivateKey.trim().isEmpty()
                    || !oldPrivateKey.equals(newPrivateKey)) return null;
            verification = "same_native_controller";
        } else return null;
        try {
            return new JSONObject().put("method", method).put("from", from.json())
                .put("to", to == null ? JSONObject.NULL : to.json()).put("verification", verification);
        } catch (Exception impossible) { throw new IllegalStateException(impossible); }
    }
}
