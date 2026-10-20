package org.freevia.qcremote;

import static org.junit.Assert.*;
import org.json.JSONObject;
import org.junit.Test;

public class QcBrokerTargetTransitionTest {
    private final QcBrokerTarget old = new QcBrokerTarget("physical", "before", 1);
    private final QcBrokerTarget next = new QcBrokerTarget("physical", "after", 2);

    @Test public void resetRequiresSuccessfulClosureAndTheSamePrivatePhysicalKey() throws Exception {
        JSONObject ack = QcBrokerTargetTransition.verified("device.resetSession", old, next, "private-fixture", "private-fixture", true);
        assertNotNull(ack);
        assertEquals("device.resetSession", ack.getString("method"));
        assertTrue(old.same(QcBrokerTarget.parse(ack.get("from"))));
        assertTrue(next.same(QcBrokerTarget.parse(ack.get("to"))));
        assertEquals("same_native_controller", ack.getString("verification"));
        assertEquals(4, ack.length());
        assertFalse(ack.toString().contains("private-fixture"));
        assertNull(QcBrokerTargetTransition.verified("device.resetSession", old, next, null, null, true));
        assertNull(QcBrokerTargetTransition.verified("device.resetSession", old, next, "one", "two", true));
        assertNull(QcBrokerTargetTransition.verified("device.reconnect", old, next, "one", "one", false));
    }

    @Test public void disconnectAcknowledgesOnlyAClosedOldControllerWithoutAnyReplacementPhysicalQc() throws Exception {
        JSONObject ack = QcBrokerTargetTransition.verified("device.disconnect", old, null, null, null, true);
        assertEquals("closed_native_controller", ack.getString("verification"));
        assertTrue(ack.isNull("to"));
        assertNotNull(QcBrokerTargetTransition.verified("device.disconnect", old, new QcBrokerTarget("demo", "demo", 3), null, null, true));
        assertNull(QcBrokerTargetTransition.verified("device.disconnect", old, next, null, null, true));
        assertNull(QcBrokerTargetTransition.verified("device.disconnect", old, null, null, null, false));
    }

    @Test public void ordinaryWritesAndUnchangedOrDemoSourcesCannotClaimPhysicalLifecycleAcknowledgements() {
        assertNull(QcBrokerTargetTransition.verified("device.setMasterVolume", old, next, "same", "same", true));
        assertNull(QcBrokerTargetTransition.verified("device.reconnect", old, old, "same", "same", true));
        assertNull(QcBrokerTargetTransition.verified("device.reconnect", new QcBrokerTarget("demo", "demo", 1), next, "same", "same", true));
    }
}
