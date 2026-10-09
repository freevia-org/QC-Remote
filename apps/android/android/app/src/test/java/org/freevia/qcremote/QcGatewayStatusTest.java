package org.freevia.qcremote;

import static org.junit.Assert.*;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.Test;

public class QcGatewayStatusTest {
    @Test public void statusCarriesCapabilitiesAsAJsonArrayOfStrings() throws Exception {
        JSONObject status = QcUsbPlugin.systemStatus(true, false, new JSONObject());

        // Android's org.json stores a raw String[] as-is and later serialises
        // it through Object.toString(), so the value must already be a
        // JSONArray at put() time rather than relying on the serializer.
        Object capabilities = status.get("capabilities");
        assertTrue("capabilities must be a JSONArray, was " + capabilities.getClass().getName(),
            capabilities instanceof JSONArray);

        JSONArray parsed = new JSONObject(status.toString()).getJSONArray("capabilities");
        assertEquals(GeneratedGatewayMethods.CAPABILITIES.length, parsed.length());
        for (int index = 0; index < parsed.length(); index++) {
            assertTrue(parsed.get(index) instanceof String);
            assertEquals(GeneratedGatewayMethods.CAPABILITIES[index], parsed.getString(index));
        }
        assertFalse(status.toString().contains("[Ljava"));
        assertEquals("android-usb-relay", status.getString("transport"));
        assertEquals("Rust Android USB gateway", status.getString("platform"));
        assertTrue(status.getBoolean("connected"));
        assertFalse(status.getBoolean("synchronized"));
    }
}
