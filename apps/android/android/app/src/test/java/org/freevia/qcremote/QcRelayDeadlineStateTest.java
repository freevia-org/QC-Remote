package org.freevia.qcremote;

import static org.junit.Assert.*;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import org.junit.Test;

public class QcRelayDeadlineStateTest {
    @Test public void mutationAdmissionAndExpiryHaveOneLinearizedOutcome() throws Exception {
        ExecutorService workers = Executors.newFixedThreadPool(2);
        try {
            for (int attempt = 0; attempt < 256; attempt++) {
                QcRelayDeadlineState state = new QcRelayDeadlineState();
                CountDownLatch start = new CountDownLatch(1);
                Future<Boolean> admitted = workers.submit(() -> {
                    start.await();
                    return state.beginMutation();
                });
                Future<Integer> expiredFrom = workers.submit(() -> {
                    start.await();
                    return state.expire();
                });
                start.countDown();
                boolean mutationAdmitted = admitted.get(2, TimeUnit.SECONDS);
                int priorState = expiredFrom.get(2, TimeUnit.SECONDS);
                assertEquals(priorState == QcRelayDeadlineState.MUTATION_ADMITTED, mutationAdmitted);
                assertTrue(state.isExpired());
            }
        } finally {
            workers.shutdownNow();
            assertTrue(workers.awaitTermination(2, TimeUnit.SECONDS));
        }
    }

    @Test public void readAdmissionExpiresWithoutClassifyingTheOutcomeAsAMutation() throws Exception {
        ExecutorService workers = Executors.newFixedThreadPool(2);
        try {
            for (int attempt = 0; attempt < 256; attempt++) {
                QcRelayDeadlineState state = new QcRelayDeadlineState();
                CountDownLatch start = new CountDownLatch(1);
                Future<Boolean> admitted = workers.submit(() -> {
                    start.await();
                    return state.beginRead();
                });
                Future<Integer> expiredFrom = workers.submit(() -> {
                    start.await();
                    return state.expire();
                });
                start.countDown();
                boolean readAdmitted = admitted.get(2, TimeUnit.SECONDS);
                int priorState = expiredFrom.get(2, TimeUnit.SECONDS);
                assertEquals(priorState == QcRelayDeadlineState.READ_ADMITTED, readAdmitted);
                assertNotEquals(QcRelayDeadlineState.MUTATION_ADMITTED, priorState);
                assertTrue(state.isExpired());
            }
        } finally {
            workers.shutdownNow();
            assertTrue(workers.awaitTermination(2, TimeUnit.SECONDS));
        }
    }

    @Test public void mutationAdmissionAfterPreparationReadStillQuarantinesOnExpiry() {
        QcRelayDeadlineState state = new QcRelayDeadlineState();
        assertTrue(state.beginRead());
        assertTrue(state.beginMutation());
        assertEquals(QcRelayDeadlineState.MUTATION_ADMITTED, state.expire());
        assertTrue(state.isExpired());
    }

    @Test public void expiryBeforeAdmissionPreventsBothReadsAndMutations() {
        QcRelayDeadlineState state = new QcRelayDeadlineState();
        assertEquals(QcRelayDeadlineState.QUEUED, state.expire());
        assertFalse(state.beginRead());
        assertFalse(state.beginMutation());
    }
}
