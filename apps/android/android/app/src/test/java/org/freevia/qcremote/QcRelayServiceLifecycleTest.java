package org.freevia.qcremote;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class QcRelayServiceLifecycleTest {
    @Test public void demoNeverClaimsTheUsbForegroundServicePrerequisite() {
        assertEquals(QcRelayService.HostMode.NONE, QcRelayService.hostMode(false, false));
        assertEquals(QcRelayService.HostMode.VISIBLE_DEMO, QcRelayService.hostMode(false, true));
        assertEquals(QcRelayService.HostMode.CONNECTED_DEVICE, QcRelayService.hostMode(true, true));
        assertEquals(QcRelayService.HostMode.CONNECTED_DEVICE, QcRelayService.hostMode(true, false));
    }

    @Test public void demoAndUsbReadinessAreDistinctAndCarryTheExactTarget() throws Exception {
        QcBrokerTarget demo = new QcBrokerTarget("demo", "broker-demo", 1);
        org.json.JSONObject frame = QcRelayService.readinessPayload(demo);
        assertFalse(frame.getBoolean("usbConnected"));
        assertTrue(frame.getBoolean("targetReady"));
        assertTrue(demo.same(QcBrokerTarget.parse(frame.get("brokerTarget"))));
        QcBrokerTarget physical = new QcBrokerTarget("physical", "usb", 2);
        assertTrue(QcRelayService.readinessPayload(physical).getBoolean("usbConnected"));
        assertFalse(QcRelayService.readinessPayload(null).getBoolean("targetReady"));
        assertFalse(QcRelayService.readinessPayload(null).has("brokerTarget"));
    }
    @Test public void stickyServiceRecreationReconnectsWhenPersistedStateHasNoSocket() {
        // `connected` survives process death in RelayCredentialStore, but the
        // Service's WebSocket does not. This is the START_STICKY recreation
        // state that previously made the remote host appear disconnected.
        assertTrue(QcRelayService.shouldConnectOnStart("connected", false));
        assertTrue(QcRelayService.shouldConnectOnStart("connecting", false));
        assertTrue(QcRelayService.shouldConnectOnStart("reconnecting", false));
    }

    @Test public void activeServiceWithTransportDoesNotOpenADuplicateSocket() {
        assertFalse(QcRelayService.shouldConnectOnStart("connected", true));
        assertFalse(QcRelayService.shouldConnectOnStart("connecting", true));
        assertFalse(QcRelayService.shouldConnectOnStart("reconnecting", true));
    }

    @Test public void inactiveServiceStartsEvenIfAnOldSocketReferenceExists() {
        assertTrue(QcRelayService.shouldConnectOnStart("stopped", true));
        assertTrue(QcRelayService.shouldConnectOnStart("pairing_required", true));
    }

    @Test public void recreatedPluginUsesLiveServiceStateRatherThanSavedConnectedState() {
        // A new Capacitor plugin instance can attach to a still-running
        // Service after Activity/WebView recreation and should see its state.
        assertEquals("connected", QcRelayService.stateForStatus("connected", "connected"));
        assertEquals("connecting", QcRelayService.stateForStatus("connecting", "connected"));
    }

    @Test public void absentServiceCannotReportPersistedActiveStateAsConnected() {
        // Process death clears the WebSocket and the static Service instance,
        // while RelayCredentialStore may still contain the previous state.
        assertEquals("stopped", QcRelayService.stateForStatus(null, "connected"));
        assertEquals("stopped", QcRelayService.stateForStatus(null, "connecting"));
        assertEquals("stopped", QcRelayService.stateForStatus(null, "reconnecting"));
    }

    @Test public void absentServicePreservesNonConnectedTerminalStatus() {
        assertEquals("pairing_required", QcRelayService.stateForStatus(null, "pairing_required"));
        assertEquals("invalid_endpoint", QcRelayService.stateForStatus(null, "invalid_endpoint"));
        assertEquals("stopped", QcRelayService.stateForStatus(null, "stopped"));
    }
}
