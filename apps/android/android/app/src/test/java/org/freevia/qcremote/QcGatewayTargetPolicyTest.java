package org.freevia.qcremote;

import static org.junit.Assert.*;
import org.junit.Test;

public class QcGatewayTargetPolicyTest {
    @Test public void managedGlobalAndPersistentWritesNeedAnAcceptedTarget() {
        for (String method : new String[]{"device.setTempo", "device.setMasterVolume", "device.renameCurrentPreset", "device.setDeviceName", "device.createBackup", "device.resetSession", "device.disconnect"}) {
            try { QcGatewayTargetPolicy.requireAccepted(method, true, false); fail(method); }
            catch (RelayException refused) { assertEquals("TARGET_SUPERSEDED", refused.code); }
            QcGatewayTargetPolicy.requireAccepted(method, true, true);
        }
    }

    @Test public void readDiscoveryAndIntentionalReconnectCanAcquireAContext() {
        for (String method : new String[]{"system.status", "device.currentPreset", "device.identity", "device.reconnect"})
            QcGatewayTargetPolicy.requireAccepted(method, true, false);
    }

    @Test public void legacyAndRelayCallsUseTheirOwnNativeAdmissionRatherThanUiContext() {
        QcGatewayTargetPolicy.requireAccepted("device.setTempo", false, false);
    }
}
