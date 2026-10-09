package org.freevia.qcremote;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import org.junit.Test;

public final class QcUsbReaderTeardownTest {
    @Test public void replacementWaitsUntilDestroyedInstanceReaderExits() throws Exception {
        QcUsbReaderTeardown lifecycle = new QcUsbReaderTeardown();
        ExecutorService oldReader = Executors.newSingleThreadExecutor();
        CountDownLatch readerStarted = new CountDownLatch(1);
        CountDownLatch allowReaderExit = new CountDownLatch(1);
        oldReader.execute(() -> holdUntilReleased(readerStarted, allowReaderExit));
        try {
            assertTrue(readerStarted.await(1, TimeUnit.SECONDS));
            lifecycle.trackShutdown(oldReader);
            assertFalse(lifecycle.await(20));

            allowReaderExit.countDown();
            assertTrue(lifecycle.await(1_000));
        } finally {
            allowReaderExit.countDown();
            oldReader.shutdownNow();
        }
    }

    @Test public void replacementWaitsForEveryOverlappingInstanceTeardown() throws Exception {
        QcUsbReaderTeardown lifecycle = new QcUsbReaderTeardown();
        ExecutorService firstReader = Executors.newSingleThreadExecutor();
        ExecutorService secondReader = Executors.newSingleThreadExecutor();
        CountDownLatch firstStarted = new CountDownLatch(1);
        CountDownLatch secondStarted = new CountDownLatch(1);
        CountDownLatch releaseFirst = new CountDownLatch(1);
        CountDownLatch releaseSecond = new CountDownLatch(1);
        firstReader.execute(() -> holdUntilReleased(firstStarted, releaseFirst));
        secondReader.execute(() -> holdUntilReleased(secondStarted, releaseSecond));
        try {
            assertTrue(firstStarted.await(1, TimeUnit.SECONDS));
            assertTrue(secondStarted.await(1, TimeUnit.SECONDS));
            lifecycle.trackShutdown(firstReader);
            lifecycle.trackShutdown(secondReader);
            releaseFirst.countDown();
            assertFalse(lifecycle.await(20));

            releaseSecond.countDown();
            assertTrue(lifecycle.await(1_000));
        } finally {
            releaseFirst.countDown();
            releaseSecond.countDown();
            firstReader.shutdownNow();
            secondReader.shutdownNow();
        }
    }

    private static void holdUntilReleased(CountDownLatch started, CountDownLatch release) {
        started.countDown();
        boolean done = false;
        while (!done) {
            try {
                done = release.await(1, TimeUnit.SECONDS);
            } catch (InterruptedException ignored) {
                // UsbRequest cancellation interrupts the reader before its native wait exits.
            }
        }
    }
}
