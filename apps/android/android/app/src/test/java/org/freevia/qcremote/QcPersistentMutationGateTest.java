package org.freevia.qcremote;

import static org.junit.Assert.*;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.atomic.AtomicBoolean;
import org.junit.Test;

public class QcPersistentMutationGateTest {
    @Test
    public void nextMutationWaitsForPriorVerificationToFinish() {
        QcPersistentMutationGate gate = new QcPersistentMutationGate();
        CompletableFuture<String> firstResult = new CompletableFuture<>();
        AtomicBoolean secondStarted = new AtomicBoolean(false);

        CompletableFuture<String> first = gate.submit(() -> firstResult);
        CompletableFuture<String> second = gate.submit(() -> {
            secondStarted.set(true);
            return CompletableFuture.completedFuture("second");
        });

        assertFalse(secondStarted.get());
        firstResult.complete("first");
        assertEquals("first", first.join());
        assertEquals("second", second.join());
        assertTrue(secondStarted.get());
    }

    @Test
    public void failedMutationDoesNotPermanentlyBlockTheLane() {
        QcPersistentMutationGate gate = new QcPersistentMutationGate();
        CompletableFuture<String> failed = gate.submit(() -> {
            throw new IllegalStateException("failed");
        });
        CompletableFuture<String> next = gate.submit(
            () -> CompletableFuture.completedFuture("next"));

        assertThrows(java.util.concurrent.CompletionException.class, failed::join);
        assertEquals("next", next.join());
    }
}
