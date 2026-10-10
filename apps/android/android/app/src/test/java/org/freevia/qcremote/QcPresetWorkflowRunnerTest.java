package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.util.ArrayDeque;
import java.util.Deque;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.Test;

public class QcPresetWorkflowRunnerTest {
    @Test public void nativeSubmissionIsQueuedOnceAndDoesNotBlockOwnerPolling() {
        Deque<Runnable> timers = new ArrayDeque<>();
        Deque<Runnable> nativeQueue = new ArrayDeque<>();
        AtomicInteger turns = new AtomicInteger();
        AtomicInteger completions = new AtomicInteger();
        QcPresetWorkflowRunner<String> runner = new QcPresetWorkflowRunner<>(() -> {
            if (turns.getAndIncrement() == 0) return new QcPresetWorkflowRunner.Step<>(
                false, null, () -> nativeQueue.add(completions::incrementAndGet), 10);
            return new QcPresetWorkflowRunner.Step<>(completions.get() == 1, "verified", null, 10);
        }, (task, delay) -> { assertEquals(10, delay); timers.add(task); }, () -> fail("no cancellation"));
        CompletableFuture<String> result = runner.start();
        timers.remove().run();
        assertEquals(1, nativeQueue.size());
        assertFalse(result.isDone());
        nativeQueue.remove().run();
        timers.remove().run();
        assertEquals("verified", result.join());
        runner.run();
        assertEquals(3, turns.get());
        assertTrue(timers.isEmpty());
    }

    @Test public void callerCancellationDoesNotAbandonOwnerSettlementAndCleanup() {
        Deque<Runnable> timers = new ArrayDeque<>();
        AtomicInteger turns = new AtomicInteger();
        AtomicInteger cleanups = new AtomicInteger();
        QcPresetWorkflowRunner<String> runner = new QcPresetWorkflowRunner<>(() -> {
            int turn = turns.getAndIncrement();
            return new QcPresetWorkflowRunner.Step<>(turn == 2, "finished",
                turn == 1 ? cleanups::incrementAndGet : null, 0);
        }, (task, delay) -> { assertEquals(1, delay); timers.add(task); }, () -> fail("no cancellation"));
        CompletableFuture<String> result = runner.start();
        assertTrue(result.cancel(false));
        timers.remove().run();
        timers.remove().run();
        assertEquals(1, cleanups.get());
        assertTrue(result.isCancelled());
        assertTrue(timers.isEmpty());
    }

    @Test public void preparationSubmissionFailureStopsAndReleasesPreparation() {
        AtomicInteger cancelled = new AtomicInteger();
        QcPresetWorkflowRunner<String> runner = new QcPresetWorkflowRunner<>(
            () -> new QcPresetWorkflowRunner.Step<>(false, null,
                () -> { throw new IllegalStateException("queue closed"); }, 10),
            (task, delay) -> fail("failed preparation must not schedule another action"),
            cancelled::incrementAndGet);
        CompletableFuture<String> result = runner.start();
        assertTrue(result.isCompletedExceptionally());
        assertEquals(1, cancelled.get());
        runner.run();
        assertEquals(1, cancelled.get());
    }
}
