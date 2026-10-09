package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.Executor;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.Test;

public class QcStatePublicationDrainTest {
    @Test public void reverseProducerWakeupsDrainNativeCommitOrderOnce() {
        ArrayDeque<Runnable> jobs = new ArrayDeque<>();
        ArrayDeque<String> nativeQueue = new ArrayDeque<>();
        List<String> delivered = new ArrayList<>();
        QcStatePublicationDrain drain = new QcStatePublicationDrain(jobs::add, () -> {
            while (!nativeQueue.isEmpty()) delivered.add(nativeQueue.remove());
        }, () -> !nativeQueue.isEmpty(), error -> { throw new AssertionError(error); });
        nativeQueue.add("wire reduction 1");
        nativeQueue.add("metadata reduction 2");
        drain.request(); // Metadata returns through JNI before the reader does.
        drain.request();
        assertEquals(1, jobs.size());
        jobs.remove().run();
        assertEquals(List.of("wire reduction 1", "metadata reduction 2"), delivered);
        assertTrue(jobs.isEmpty());
    }

    @Test public void aWakeupRacingTheEmptyPullIsNotLost() {
        ArrayDeque<Runnable> jobs = new ArrayDeque<>();
        AtomicInteger work = new AtomicInteger();
        QcStatePublicationDrain[] drain = new QcStatePublicationDrain[1];
        drain[0] = new QcStatePublicationDrain(jobs::add, () -> {
            if (work.incrementAndGet() == 1) drain[0].request();
        }, () -> work.get() == 1, error -> { throw new AssertionError(error); });
        drain[0].request();
        jobs.remove().run();
        assertEquals(1, jobs.size());
        jobs.remove().run();
        assertEquals(2, work.get());
        assertTrue(jobs.isEmpty());
    }

    @Test public void aFailedDrainCannotSpinOnAStickyNativeError() {
        ArrayDeque<Runnable> jobs = new ArrayDeque<>();
        List<Exception> failures = new ArrayList<>();
        QcStatePublicationDrain drain = new QcStatePublicationDrain(jobs::add, () -> {
            throw new IllegalStateException("publication overflow");
        }, () -> true, failures::add);
        drain.request();
        jobs.remove().run();
        assertEquals(1, failures.size());
        assertTrue(jobs.isEmpty());
    }

    @Test public void shutdownRevokesQueuedAndSubsequentWakeups() {
        ArrayDeque<Runnable> jobs = new ArrayDeque<>();
        AtomicInteger work = new AtomicInteger();
        QcStatePublicationDrain drain = new QcStatePublicationDrain(jobs::add, work::incrementAndGet,
            () -> true, error -> { throw new AssertionError(error); });
        drain.request();
        drain.close();
        jobs.remove().run();
        drain.request();
        assertEquals(0, work.get());
        assertTrue(jobs.isEmpty());
    }

    @Test public void executorRejectionReleasesTheWakeupAndReportsFailure() {
        AtomicInteger failures = new AtomicInteger();
        Executor rejected = work -> { throw new RejectedExecutionException("closed executor"); };
        QcStatePublicationDrain drain = new QcStatePublicationDrain(rejected, () -> fail("must not run"),
            () -> false, error -> failures.incrementAndGet());
        drain.request();
        drain.request();
        assertEquals(2, failures.get());
    }
}
