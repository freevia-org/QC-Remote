package org.freevia.qcremote;
import static org.junit.Assert.*;
import org.junit.Test;
import org.json.JSONObject;
public class RelayUserActionSerializationTest {
    @Test public void unsavedRefusalsSerializeDistinctActionAndRetryPolicies() throws Exception {
        assertPolicy("UNSAVED_STATE_UNCONFIRMED", false, true);
        assertPolicy("UNSAVED_STATE_NOT_YET_OBSERVED", true, false);
    }
    private static void assertPolicy(String code, boolean retryable, boolean required) throws Exception {
        RelayException exception = new RelayException(code, "arbitrary text, not a policy input");
        JSONObject wire = RelayProtocol.error("request", code, exception.getMessage(), exception.retryable());
        JSONObject error = new JSONObject(wire.toString()).getJSONObject("error");
        assertEquals(code, error.getString("code"));
        assertEquals(retryable, error.getBoolean("retryable"));
        assertEquals(required, error.getBoolean("userActionRequired"));
    }
    @Test public void unrelatedErrorKeepsLegacyShape() throws Exception {
        JSONObject error = RelayProtocol.error("request", "DEVICE_TRANSPORT", "UNSAVED_STATE_UNCONFIRMED", true).getJSONObject("error");
        assertFalse(error.has("userActionRequired")); assertTrue(error.getBoolean("retryable"));
    }
}
