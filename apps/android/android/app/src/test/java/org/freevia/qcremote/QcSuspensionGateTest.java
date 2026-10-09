package org.freevia.qcremote;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class QcSuspensionGateTest {
    @Test
    public void everyRequestThatReachesTheDeviceIsRefused() {
        for (String kind : new String[] {
            "CORRELATED_READ", "BLOCK_DETAILS", "SET_DEVICE_NAME", "TAP_SCREEN", "BACKUP",
            "PREVIEW_PARAMETER", "PLANNED_WRITE", "PRESET_WRITE", "PERSISTENT_WRITE"
        }) {
            assertTrue(kind + " reaches the device", QcSuspensionGate.refuses(kind));
        }
    }

    @Test
    public void statusSessionControlAndRetainedReadsStayAvailable() {
        for (String kind : new String[] {
            "SYSTEM", "RECONNECT", "DISCONNECT", "SNAPSHOT", "CURRENT_PRESET", "STATE_EVENTS",
            "TEMPO_CLOCK", "GENERAL_SETTINGS", "IO_SETTINGS", "MODELS", "RETAINED_CATALOG",
            "MASTER_VOLUME"
        }) {
            assertFalse(kind + " is not a device request", QcSuspensionGate.refuses(kind));
        }
    }

    @Test
    public void anUnclassifiedDispatchKindFailsClosed() {
        assertTrue(QcSuspensionGate.refuses("SOMETHING_NEW"));
    }

    @Test
    public void anOwnerRefusalIsTypedUnlessPartOfTheRequestLeft() {
        String refusal = QcSuspensionGate.MARKER + ": BACKUP in progress (a device backup) for 3 s";
        assertTrue("DEVICE_SUSPENDED".equals(QcSuspensionGate.code(
            new IllegalStateException(refusal), "DEVICE_ERROR")));
        assertTrue("DEVICE_ERROR".equals(QcSuspensionGate.code(new IllegalStateException(
            refusal + "; an earlier frame of this request was already sent, so the device effect may be uncertain"),
            "DEVICE_ERROR")));
        assertTrue("DEVICE_ERROR".equals(QcSuspensionGate.code(
            new IllegalStateException("USB HID write failed"), "DEVICE_ERROR")));
    }
}
