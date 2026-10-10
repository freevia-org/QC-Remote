package org.freevia.qcremote;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

import org.junit.Test;

public class QcRelayAdmissionGuardTest {
    @Test public void expiryAndDestroyDoNotWaitForTransportMonitor() throws Exception {
        Object transportMonitor = new Object();
        QcRelayAdmissionGuard guard = new QcRelayAdmissionGuard();
        ExecutorService worker = Executors.newSingleThreadExecutor();
        try {
            synchronized (transportMonitor) {
                Future<Integer> expiry = worker.submit(() -> guard.expire(new QcRelayDeadlineState()));
                assertEquals(QcRelayDeadlineState.QUEUED,
                    (int) expiry.get(2, TimeUnit.SECONDS));
                Future<?> destroy = worker.submit(guard::markTerminal);
                destroy.get(2, TimeUnit.SECONDS);
            }
            assertTrue(guard.isTerminal());
            assertFalse(guard.beginRead(new QcRelayDeadlineState()));
        } finally {
            worker.shutdownNow();
            assertTrue("worker must terminate within the test bound",
                worker.awaitTermination(2, TimeUnit.SECONDS));
        }
    }

    @Test public void lifecyclePublicationAndDestroyShareOnlyTheShortGuard() {
        QcRelayAdmissionGuard guard = new QcRelayAdmissionGuard();
        long epoch = guard.epoch();
        AtomicBoolean published = new AtomicBoolean();

        assertTrue(guard.publish(epoch, () -> published.set(true)));
        guard.markTerminal();
        assertFalse(guard.publish(epoch, () -> published.set(false)));
        assertTrue(published.get());
        assertFalse(guard.beginMutation(new QcRelayDeadlineState()));
    }

    @Test public void expiryAndMutationAdmissionRemainLinearizedWithQuarantine() {
        QcRelayAdmissionGuard guard = new QcRelayAdmissionGuard();
        QcRelayDeadlineState deadline = new QcRelayDeadlineState();

        assertTrue(guard.beginMutation(deadline));
        assertEquals(QcRelayDeadlineState.MUTATION_ADMITTED, guard.expire(deadline));
        assertTrue(guard.writesQuarantined());
        assertFalse(guard.beginMutation(new QcRelayDeadlineState()));
    }
}
