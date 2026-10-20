package org.freevia.qcremote;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.assertEquals;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.Test;

public final class RelayReadinessPublicationTest {
    @Test public void idleObservationDoesNotGenerateRecurringCloudMessages() {
        RelayReadinessPublication publication = new RelayReadinessPublication();
        AtomicInteger sends = new AtomicInteger();
        assertTrue(publication.publish(true, () -> { sends.incrementAndGet(); return true; }));
        // A full day's one-second local observations produce zero further sends.
        for (int tick = 0; tick < 86_400; tick++) {
            assertFalse(publication.publish(true, () -> { sends.incrementAndGet(); return true; }));
        }
        assertEquals(1, sends.get());
    }

    @Test public void bothReadinessTransitionsReachTheCloudOnce() {
        RelayReadinessPublication publication = new RelayReadinessPublication();
        List<Boolean> sent = new ArrayList<>();
        for (boolean ready : new boolean[] { false, false, true, true, false, false }) {
            publication.publish(ready, () -> { sent.add(ready); return true; });
        }
        assertEquals(Arrays.asList(false, true, false), sent);
    }

    @Test public void reconnectAlwaysPublishesAnInitialSnapshot() {
        RelayReadinessPublication publication = new RelayReadinessPublication();
        assertTrue(publication.publish(true, () -> true));
        assertFalse(publication.publish(true, () -> true));
        publication.reset();
        assertTrue(publication.publish(true, () -> true));
    }

    @Test public void aRejectedSendDoesNotSuppressTheNextAttempt() {
        RelayReadinessPublication publication = new RelayReadinessPublication();
        assertFalse(publication.publish(false, () -> false));
        assertTrue(publication.publish(false, () -> true));
        assertFalse(publication.publish(true, () -> false));
        assertTrue(publication.publish(true, () -> true));
    }

    @Test public void aThrowingSendDoesNotPublishState() {
        RelayReadinessPublication publication = new RelayReadinessPublication();
        try {
            publication.publish(true, () -> { throw new IllegalStateException("send failed"); });
            throw new AssertionError("send should throw");
        } catch (IllegalStateException expected) {
            assertEquals("send failed", expected.getMessage());
        }
        assertTrue(publication.publish(true, () -> true));
    }
}
