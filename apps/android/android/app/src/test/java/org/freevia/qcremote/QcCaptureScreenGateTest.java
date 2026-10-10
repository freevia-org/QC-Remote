package org.freevia.qcremote;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class QcCaptureScreenGateTest {
    @Test
    public void oneUncorrelatedReplyCanBelongToOnlyOneCapture() {
        QcCaptureScreenGate gate = new QcCaptureScreenGate();
        assertTrue(gate.tryAcquire());
        assertFalse("a simultaneous capture must be rejected", gate.tryAcquire());
        gate.release();
        assertTrue("completion releases ownership for the next capture", gate.tryAcquire());
    }
}
