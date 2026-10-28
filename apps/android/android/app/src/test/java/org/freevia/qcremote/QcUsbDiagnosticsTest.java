package org.freevia.qcremote;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import org.junit.Test;

public class QcUsbDiagnosticsTest {
    @Test public void startupFailureSurvivesRetryWithoutAcceptingStaleAttemptOrMutatingSnapshots() {
        QcUsbDiagnostics diagnostics = new QcUsbDiagnostics();
        long firstEpoch = diagnostics.beginSession();
        diagnostics.startupFailed(firstEpoch, "startup phase=initializing; type 35 decode-failure");
        QcUsbDiagnostics.Snapshot first = diagnostics.snapshot();
        assertEquals(null, diagnostics.startupFailed(firstEpoch, "duplicate failure"));
        long retryEpoch = diagnostics.beginSession();
        assertEquals(firstEpoch, diagnostics.snapshot().startupFailure.sessionEpoch);
        diagnostics.startupFailed(firstEpoch, "stale callback");
        assertEquals(first.startupFailure.detail, diagnostics.snapshot().startupFailure.detail);
        diagnostics.startupFailed(retryEpoch, "startup phase=initializing; type 35 empty-statistics");
        assertEquals(retryEpoch, diagnostics.snapshot().startupFailure.sessionEpoch);
        assertEquals("startup phase=initializing; type 35 empty-statistics", diagnostics.snapshot().startupFailure.detail);
        assertEquals("startup phase=initializing; type 35 decode-failure", first.startupFailure.detail);
        assertEquals(0, diagnostics.snapshot().messagesReceived);
    }

    @Test public void immutableSnapshotKeepsReportAndMessageUnitsDistinctAcrossSessions() {
        QcUsbDiagnostics diagnostics = new QcUsbDiagnostics();
        long firstEpoch = diagnostics.beginSession();
        diagnostics.rawReportReceived(firstEpoch);
        diagnostics.rawReportReceived(firstEpoch);
        diagnostics.messageReceived(firstEpoch, 9);
        diagnostics.messageSent(firstEpoch, 3);

        QcUsbDiagnostics.Snapshot first = diagnostics.snapshot();
        assertEquals(1, first.sessionEpoch);
        assertEquals(1, first.messagesReceived);
        assertEquals(Long.valueOf(1), first.messagesReceivedByType.get(9));
        assertEquals(2, first.rawReportsReceived);
        assertEquals(1, first.messagesSent);
        assertEquals(Long.valueOf(1), first.messagesSentByType.get(3));
        assertThrows(UnsupportedOperationException.class,
            () -> first.messagesReceivedByType.put(4, 1L));

        long secondEpoch = diagnostics.beginSession();
        diagnostics.rawReportReceived(firstEpoch);
        diagnostics.messageReceived(firstEpoch, 7);
        diagnostics.messageSent(firstEpoch, 6);
        diagnostics.messageReceived(secondEpoch, 8);
        QcUsbDiagnostics.Snapshot second = diagnostics.snapshot();
        assertEquals(2, second.sessionEpoch);
        assertEquals(1, second.messagesReceived);
        assertEquals(Long.valueOf(1), second.messagesReceivedByType.get(8));
        assertEquals(0, second.rawReportsReceived);
        assertEquals(0, second.messagesSent);
        assertEquals(1, first.sessionEpoch);
        assertEquals(2, first.rawReportsReceived);
    }

    @Test public void concurrentSnapshotNeverSplitsRelatedMessageCounterAndMapUpdate() throws Exception {
        QcUsbDiagnostics diagnostics = new QcUsbDiagnostics();
        long epoch = diagnostics.beginSession();
        int updates = 10_000;
        CountDownLatch start = new CountDownLatch(1);
        ExecutorService pool = Executors.newFixedThreadPool(2);
        try {
            Future<?> writer = pool.submit(() -> {
                start.await();
                for (int index = 0; index < updates; index++) diagnostics.messageReceived(epoch, index % 4);
                return null;
            });
            Future<?> reader = pool.submit(() -> {
                start.await();
                for (int index = 0; index < updates; index++) {
                    QcUsbDiagnostics.Snapshot snapshot = diagnostics.snapshot();
                    long byType = snapshot.messagesReceivedByType.values().stream()
                        .mapToLong(Long::longValue).sum();
                    assertEquals(snapshot.messagesReceived, byType);
                }
                return null;
            });
            start.countDown();
            writer.get(10, TimeUnit.SECONDS);
            reader.get(10, TimeUnit.SECONDS);
            assertEquals(updates, diagnostics.snapshot().messagesReceived);
            assertTrue(diagnostics.snapshot().messagesReceivedByType.size() <= 4);
        } finally {
            pool.shutdownNow();
            assertTrue(pool.awaitTermination(2, TimeUnit.SECONDS));
        }
    }
}
