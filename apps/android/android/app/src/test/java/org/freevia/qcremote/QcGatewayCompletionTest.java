package org.freevia.qcremote;

import org.json.JSONObject;
import org.junit.Test;
import static org.junit.Assert.*;

public final class QcGatewayCompletionTest {
    private static JSONObject complete(JSONObject action, QcGatewayCompletion.Snapshot snapshot) throws Exception {
        return QcGatewayCompletion.assemble(action, "device.recallPreset", "Preset recall requested", 900,
            100, 7, 42, snapshot);
    }

    @Test public void timeoutCarriesOnlyUnverifiedFactsAndOptionalMirror() throws Exception {
        JSONObject mirror = new JSONObject().put("presetName", "mirror only");
        JSONObject result = complete(new JSONObject().put("kind", "timedOut"), () -> mirror);
        assertTrue(result.getBoolean("accepted"));
        assertFalse(result.getBoolean("verified"));
        assertEquals("accepted_unverified", result.getString("verification"));
        assertEquals(QcTimedOutDetail.of("device.recallPreset", "Preset recall requested", 900), result.getString("detail"));
        assertSame(mirror, result.getJSONObject("snapshot"));
        assertEquals(100, result.getLong("hostStartedAtUnixMs"));
        assertEquals(7, result.getLong("dispatchLatencyMs"));
        for (String key : new String[]{"changed", "readback", "observation", "stateSequence"}) assertFalse(key, result.has(key));
        assertFalse(complete(new JSONObject().put("kind", "timedOut"), () -> { throw new Exception("no snapshot"); }).has("snapshot"));
    }

    @Test public void verifiedRecallUsesNativeUnverifiedOutcomeAndRetainsMirror() throws Exception {
        JSONObject observation = new JSONObject().put("kind", "preset_address_observed");
        JSONObject outcome = new JSONObject().put("accepted", true).put("verified", false)
            .put("verification", "accepted_unverified").put("detail", "address only").put("observation", observation);
        JSONObject result = complete(new JSONObject().put("kind", "verified").put("outcome", outcome), JSONObject::new);
        assertFalse(result.getBoolean("verified"));
        assertEquals("accepted_unverified", result.getString("verification"));
        assertEquals("address only", result.getString("detail"));
        assertSame(observation, result.getJSONObject("observation"));
        assertTrue(result.has("snapshot"));
        assertEquals(42, result.getLong("stateSequence"));
        assertFalse(result.has("readback"));
        assertFalse(result.has("changed"));
    }

    @Test public void verifiedSnapshotFailureStillPropagatesButOrdinaryVerifiedPreservesReadback() throws Exception {
        try {
            complete(new JSONObject().put("kind", "verified"), () -> { throw new Exception("mirror failed"); });
            fail("verified mirror exception must propagate");
        } catch (Exception expected) { assertEquals("mirror failed", expected.getMessage()); }
        JSONObject readback = new JSONObject().put("note", "A note");
        JSONObject result = QcGatewayCompletion.assemble(new JSONObject().put("kind", "verified")
            .put("pluginBlockReadback", readback), "device.addBlock", "Requested.", 900, 100, 7, 42,
            () -> { throw new AssertionError("non preset write must not read mirror"); });
        assertTrue(result.getBoolean("verified"));
        assertEquals("authoritative_readback", result.getString("verification"));
        assertSame(readback, result.getJSONObject("pluginBlockReadback"));
        assertEquals("Requested. A note", result.getString("detail"));
    }

    @Test public void waitAndMalformedActionsCannotBecomeSuccessfulCompletions() throws Exception {
        for (JSONObject action : new JSONObject[]{new JSONObject().put("kind", "wait"), new JSONObject()}) {
            try { complete(action, JSONObject::new); fail("nonterminal action accepted"); }
            catch (IllegalStateException | org.json.JSONException expected) { }
        }
    }
}
