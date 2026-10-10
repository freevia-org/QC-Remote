package org.freevia.qcremote;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

import org.junit.Test;

public class QcRelayTaskRetirementTest {
    @Test public void queuedWorkKeepsRetirementPendingAfterOperationCompletes() {
        QcRelayTaskRetirement retirement = new QcRelayTaskRetirement();
        assertTrue(retirement.taskQueued());

        retirement.operationFinished();

        assertFalse(retirement.completion().isDone());
        assertFalse(retirement.taskQueued());
        retirement.taskRetired();
        assertTrue(retirement.completion().isDone());
    }

    @Test public void rejectedSubmissionRetiresItsReservation() {
        QcRelayTaskRetirement retirement = new QcRelayTaskRetirement();
        assertTrue(retirement.taskQueued());

        // submitRelayTask balances the reservation when Executor.execute rejects.
        retirement.taskRetired();
        retirement.operationFinished();

        assertTrue(retirement.completion().isDone());
    }

    @Test public void runningTaskRetainsSlotUntilItsFinallyRetires() {
        QcRelayTaskRetirement retirement = new QcRelayTaskRetirement();
        assertTrue(retirement.taskQueued());
        assertTrue(retirement.taskQueued());

        retirement.operationFinished();
        retirement.taskRetired();
        assertFalse(retirement.completion().isDone());
        retirement.taskRetired();

        assertTrue(retirement.completion().isDone());
    }

    @Test public void shutdownNowRetiresReturnedQueuedTaskAndRunningTaskExits() throws Exception {
        QcRelayTaskRetirement retirement = new QcRelayTaskRetirement();
        ExecutorService executor = Executors.newSingleThreadExecutor();
        CountDownLatch running = new CountDownLatch(1);
        CountDownLatch dropped = new CountDownLatch(1);
        try {
            assertTrue(retirement.taskQueued());
            executor.execute(new QcRelayTrackedTask(retirement, () -> {
                running.countDown();
                try { new CountDownLatch(1).await(5, TimeUnit.SECONDS); }
                catch (InterruptedException ignored) { Thread.currentThread().interrupt(); }
            }, null));
            assertTrue(running.await(2, TimeUnit.SECONDS));

            assertTrue(retirement.taskQueued());
            executor.execute(new QcRelayTrackedTask(retirement, () -> {}, dropped::countDown));
            retirement.operationFinished();
            List<Runnable> neverStarted = executor.shutdownNow();
            for (Runnable task : neverStarted)
                if (task instanceof QcRelayTrackedTask) ((QcRelayTrackedTask) task).cancelBeforeRun();

            assertTrue(dropped.await(2, TimeUnit.SECONDS));
            assertTrue(executor.awaitTermination(2, TimeUnit.SECONDS));
            assertTrue(retirement.completion().get(2, TimeUnit.SECONDS) == null);
        } finally {
            executor.shutdownNow();
            assertTrue("executor must terminate within the test bound",
                executor.awaitTermination(2, TimeUnit.SECONDS));
        }
    }
}
