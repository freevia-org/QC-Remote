package org.freevia.qcremote;

import static org.junit.Assert.*;

import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import org.junit.Test;

public class RelaySecurityPolicyTest {
    private static String[] allowedRemoteMethods() throws Exception {
        Field field = GeneratedRemoteActions.class.getDeclaredField("ALLOWED");
        field.setAccessible(true);
        return (String[]) field.get(null);
    }

    @Test public void remoteDeletionsKeepInitiatingClientConfirmation() throws Exception {
        assertTrue(GeneratedRemoteActions.requiresConfirmation("device.deletePreset"));
        assertTrue(GeneratedRemoteActions.requiresConfirmation("device.deleteSetlist"));
        List<String> allowed = Arrays.asList(allowedRemoteMethods());
        for (String method : new String[] { "device.deletePreset", "device.deleteSetlist" }) {
            assertTrue(method, allowed.contains(method));
            assertTrue(method, GeneratedRemoteActions.requiresConfirmation(method));
        }
    }

    @Test public void relayEndpointIsTheProductionHostOnly() {
        assertEquals("https://qcrc.freevia.org", RelayEndpointPolicy.normalize(" https://qcrc.freevia.org/ ", false));
        assertEquals("https://qcrc.freevia.org", RelayEndpointPolicy.normalize("https://QCRC.freevia.org:443", false));
        assertNull(RelayEndpointPolicy.normalize("https://qcrc.freevia.org:8443", false));
        assertNull(RelayEndpointPolicy.normalize("http://qcrc.freevia.org", false));
        assertNull(RelayEndpointPolicy.normalize("https://relay.example.com", false));
        assertNull(RelayEndpointPolicy.normalize("https://qcrc.freevia.org.attacker.example", false));
        assertNull(RelayEndpointPolicy.normalize("https://attacker.example/qcrc.freevia.org", false));
        assertNull(RelayEndpointPolicy.normalize("https://user@qcrc.freevia.org", false));
        assertNull(RelayEndpointPolicy.normalize("https://qcrc.freevia.org?token=secret", false));
        assertNull(RelayEndpointPolicy.normalize("https://qcrc.freevia.org#fragment", false));
        assertNull(RelayEndpointPolicy.normalize("https://qcrc.freevia.org/v1", false));
        assertNull(RelayEndpointPolicy.normalize("", false));
        assertNull(RelayEndpointPolicy.normalize(null, false));
    }

    @Test public void loopbackRelaysAreDebugOnly() {
        for (String loopback : new String[] { "https://localhost:8787", "https://127.0.0.1:8787", "https://[::1]:8787" }) {
            assertNull(loopback, RelayEndpointPolicy.normalize(loopback, false));
            assertEquals(loopback, RelayEndpointPolicy.normalize(loopback + "/", true));
        }
        assertNull(RelayEndpointPolicy.normalize("http://localhost:8787", true));
        assertNull(RelayEndpointPolicy.normalize("https://10.0.2.2:8787", true));
        assertNull(RelayEndpointPolicy.normalize("https://localhost.attacker.example", true));
        // The plugin applies the build's own flag; unit tests run the debug build.
        assertEquals(BuildConfig.DEBUG ? "https://localhost:8787" : null,
            QcRelayPlugin.normalizedEndpoint("https://localhost:8787"));
        assertNull(QcRelayPlugin.normalizedEndpoint("https://relay.example.com"));
    }

    @Test public void deviceIdCannotAddQueryParametersToTheRelaySocket() {
        assertEquals("wss://qcrc.freevia.org" + GeneratedRelayProfile.DEVICE_CONNECT_PATH + "?deviceId=abc-123_XYZ",
            QcRelayService.relaySocketUrl("https://qcrc.freevia.org", "abc-123_XYZ"));
        String hostile = QcRelayService.relaySocketUrl("https://qcrc.freevia.org", "a&deviceId=b#x y/?");
        assertTrue(hostile, hostile.endsWith("?deviceId=a%26deviceId%3Db%23x%20y%2F%3F"));
        assertEquals(1, hostile.split("&", -1).length);
        assertFalse(hostile.contains("#"));
    }

    @Test public void aPairingSessionDeclaresThisPhonesLabelAndPlatform() throws Exception {
        org.json.JSONObject body = QcRelayPlugin.pairingSessionBody(QcRelayPlugin.DEFAULT_DEVICE_NAME);
        assertEquals("QC Remote on Android", body.getString("label"));
        assertEquals("android", body.getString("platform"));
        assertEquals(2, body.length());
        assertEquals("QC Remote on Android", QcRelayPlugin.pairingSessionBody("  ").getString("label"));
        assertEquals("QC Remote on Android", QcRelayPlugin.pairingSessionBody(null).getString("label"));
        String longName = new String(new char[120]).replace('\0', 'x');
        assertEquals(80, QcRelayPlugin.pairingSessionBody(longName).getString("label").length());
    }

    @Test public void aNewPairingMustProveTheSameAccount() {
        assertNull(QcRelayPlugin.pairingOwnerRefusal(false, "", "owner@example.com"));
        assertNull(QcRelayPlugin.pairingOwnerRefusal(false, "", ""));
        assertNull(QcRelayPlugin.pairingOwnerRefusal(true, "owner@example.com", "OWNER@example.com"));
        assertEquals("PAIRING_OWNER_CHANGED",
            QcRelayPlugin.pairingOwnerRefusal(true, "owner@example.com", "someone@example.com")[0]);
        assertEquals("PAIRING_OWNER_UNVERIFIED",
            QcRelayPlugin.pairingOwnerRefusal(true, "owner@example.com", "")[0]);
        // A legacy pairing that recorded no account is no longer a wildcard.
        String[] legacy = QcRelayPlugin.pairingOwnerRefusal(true, "", "someone@example.com");
        assertEquals("PAIRING_OWNER_UNKNOWN", legacy[0]);
        assertTrue(legacy[1], legacy[1].contains("Unpair"));
        assertFalse(legacy[1], legacy[1].contains("still works"));
        assertEquals("PAIRING_OWNER_UNKNOWN", QcRelayPlugin.pairingOwnerRefusal(true, null, null)[0]);
    }
}
