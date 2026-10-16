package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.Test;

public class QcMidiWritePolicyTest {
    @Test public void replacementSessionNeverReceivesQueuedControls() {
        Object connection = new Object(), endpoint = new Object();
        AtomicInteger writes = new AtomicInteger();
        assertThrows(RelayException.class, () -> QcMidiWritePolicy.writeIfCurrent(
            1, 2, connection, new Object(), endpoint, new Object(), writes::incrementAndGet));
        assertEquals(0, writes.get());
        assertThrows(RelayException.class, () -> QcMidiWritePolicy.writeIfCurrent(
            1, 2, connection, connection, endpoint, endpoint, writes::incrementAndGet));
        assertThrows(RelayException.class, () -> QcMidiWritePolicy.writeIfCurrent(
            1, 1, connection, null, endpoint, null, writes::incrementAndGet));
        assertThrows(RelayException.class, () -> QcMidiWritePolicy.writeIfCurrent(
            1, 1, connection, connection, endpoint, new Object(), writes::incrementAndGet));
        assertEquals(0, writes.get());
    }
    @Test public void currentSessionWritesOnceAndPreservesTransferOutcome() {
        Object connection = new Object(), endpoint = new Object();
        AtomicInteger writes = new AtomicInteger();
        assertEquals(4, QcMidiWritePolicy.writeIfCurrent(7, 7, connection, connection, endpoint, endpoint, () -> {
            writes.incrementAndGet(); return 4;
        }));
        assertEquals(1, writes.get());
        assertEquals(-1, QcMidiWritePolicy.writeIfCurrent(7, 7, connection, connection, endpoint, endpoint, () -> -1));
    }
}
