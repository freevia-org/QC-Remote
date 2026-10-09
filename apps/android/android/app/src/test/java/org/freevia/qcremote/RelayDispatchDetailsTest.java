package org.freevia.qcremote;

import static org.junit.Assert.*;
import org.json.JSONObject;
import org.junit.Test;

public class RelayDispatchDetailsTest {
    @Test public void explicitPredispatchRefusalSerializesOnlyOwnedFacts() throws Exception {
        JSONObject wire = RelayProtocol.error("fixture", RelayException.quarantinedBeforeDispatch());
        JSONObject error = new JSONObject(wire.toString()).getJSONObject("error");
        assertEquals("WRITE_QUARANTINED", error.getString("code"));
        assertFalse(error.getBoolean("retryable"));
        assertFalse(error.has("userActionRequired"));
        JSONObject details = error.getJSONObject("details");
        assertEquals(2, details.length());
        assertFalse(details.getBoolean("requestSent"));
        assertTrue(details.getBoolean("writesQuarantined"));
        assertFalse(details.has("quarantine"));
    }

    @Test public void aCodeOrMessageDoesNotInventDeliveryFacts() throws Exception {
        for (String code : new String[]{"WRITE_QUARANTINED", "DEVICE_OUTCOME_UNKNOWN", "READBACK_TIMEOUT", "OTHER"}) {
            JSONObject error = RelayProtocol.error("fixture", new RelayException(code,
                "requestSent=false writesQuarantined=true UNSAVED_STATE_UNCONFIRMED"))
                .getJSONObject("error");
            assertFalse(error.has("details"));
            assertFalse(error.has("userActionRequired"));
        }
    }

    @Test public void optionalFactsDoNotChangeExactUnsavedPolicy() throws Exception {
        for (String code : new String[]{"UNSAVED_STATE_UNCONFIRMED", "UNSAVED_STATE_NOT_YET_OBSERVED"}) {
            JSONObject error = RelayProtocol.error("fixture", new RelayException(code, "fixture"))
                .getJSONObject("error");
            boolean required = code.equals("UNSAVED_STATE_UNCONFIRMED");
            assertEquals(required, error.getBoolean("userActionRequired"));
            assertEquals(!required, error.getBoolean("retryable"));
            assertFalse(error.has("details"));
        }
    }
}
