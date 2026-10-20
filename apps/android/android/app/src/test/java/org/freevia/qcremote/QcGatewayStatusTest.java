package org.freevia.qcremote;

import static org.junit.Assert.*;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.Test;

public class QcGatewayStatusTest {
    @Test public void demoStatusNegotiatesTheRealGatewayWithoutClaimingUsb() throws Exception {
        QcBrokerTarget target = new QcBrokerTarget("demo", "native-demo", 1);
        JSONObject status = new JSONObject().put("platform", "Demo device")
            .put("gatewayAvailable", true).put("message", "Local simulation");
        QcUsbPlugin.projectBrokerTarget("system.status", status, target);
        QcUsbPlugin.projectBrokerTarget("system.status", status, target);
        assertEquals(GeneratedGatewayMethods.API_VERSION, status.getInt("gatewayApiVersion"));
        assertTrue(status.getBoolean("connected"));
        assertTrue(status.getBoolean("synchronized"));
        assertEquals("android-demo-relay", status.getString("transport"));
        assertEquals("Demo device", status.getString("platform"));
        assertTrue(target.same(QcBrokerTarget.parse(status.get("brokerTarget"))));
        assertFalse(status.has("usbDiagnostics"));
        JSONArray capabilities = status.getJSONArray("capabilities");
        int targetCapabilityCount = 0;
        for (int index = 0; index < capabilities.length(); index++)
            if ("expectedBrokerTarget".equals(capabilities.getString(index))) targetCapabilityCount++;
        assertEquals(1, targetCapabilityCount);
    }
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
