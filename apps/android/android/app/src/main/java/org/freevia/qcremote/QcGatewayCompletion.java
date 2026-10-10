package org.freevia.qcremote;

/** Completion assembly only; native advancement and operation lifetime stay in QcUsbPlugin. */
final class QcGatewayCompletion {
    interface Snapshot { org.json.JSONObject get() throws Exception; }

    static org.json.JSONObject assemble(
        org.json.JSONObject action, String method, String detail, long confirmationTimeoutMs,
        long hostStartedAtUnixMs, long dispatchLatencyMs, long observationSequence, Snapshot snapshot
    ) throws Exception {
        switch (action.getString("kind")) {
            case "verified": {
                org.json.JSONObject completion = new org.json.JSONObject()
                    .put("accepted", true).put("verified", true)
                    .put("verification", "authoritative_readback")
                    .put("hostStartedAtUnixMs", hostStartedAtUnixMs)
                    .put("dispatchLatencyMs", dispatchLatencyMs)
                    .put("stateSequence", observationSequence).put("detail", detail);
                // Keep the current mirror for preset-write callers; native outcome may
                // explicitly qualify it as an unbound body, not a verified recall readback.
                if ("PRESET_WRITE".equals(GeneratedGatewayMethods.dispatchKind(method))) {
                    completion.put("snapshot", snapshot.get());
                }
                attachPluginBlockReadback(completion, action);
                applyNativeOutcome(completion, action);
                return completion;
            }
            case "timedOut": {
                org.json.JSONObject completion = new org.json.JSONObject()
                    .put("accepted", true).put("verified", false)
                    .put("verification", "accepted_unverified")
                    .put("hostStartedAtUnixMs", hostStartedAtUnixMs)
                    .put("dispatchLatencyMs", dispatchLatencyMs)
                    .put("detail", QcTimedOutDetail.of(method, detail,
                        confirmationTimeoutMs));
                if ("PRESET_WRITE".equals(GeneratedGatewayMethods.dispatchKind(method))) {
                    // The mirror's snapshot rides along so a caller that reads
                    // `snapshot` keeps getting what the host currently holds.
                    try { completion.put("snapshot", snapshot.get()); }
                    catch (Exception noSnapshot) { /* no preset synchronized: omit it */ }
                }
                return completion;
            }
            default: throw new IllegalStateException("Native QC verification is not terminal: " + action);
        }
    }

    static void attachPluginBlockReadback(
        org.json.JSONObject completion, org.json.JSONObject action
    ) throws org.json.JSONException {
        org.json.JSONObject readback = action.optJSONObject("pluginBlockReadback");
        if (readback == null) return;
        completion.put("pluginBlockReadback", readback);
        String note = readback.optString("note", "");
        if (note.isEmpty()) return;
        String detail = completion.optString("detail", "");
        completion.put("detail", detail.isEmpty() ? note
            : detail.endsWith(".") ? detail + " " + note : detail + ". " + note);
    }

    /**
     * A recall, bank navigation or reload only observes the address in a
     * position update (no request id, so not causal proof), and no preset body
     * can be bound to it. The native verifier then
     * returns the shared honest outcome under {@code outcome}
     * (accepted_unverified with an address observation), and its fields replace
     * the verified ones above, matching the desktop broker. The snapshot the
     * caller navigates by stays attached. No {@code outcome}: nothing changes.
     */
    static void applyNativeOutcome(
        org.json.JSONObject completion, org.json.JSONObject action
    ) throws org.json.JSONException {
        org.json.JSONObject outcome = action.optJSONObject("outcome");
        if (outcome == null) return;
        for (String key : new String[] {"accepted", "verified", "verification", "detail", "observation"}) {
            if (outcome.has(key)) completion.put(key, outcome.get(key));
        }
        completion.remove("changed");
    }

}
