package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.AbstractExecutorService;
import java.util.concurrent.TimeUnit;
import org.junit.Test;

public class QcInternetForwarderTest {
    private static final class Queue extends AbstractExecutorService {
        final ArrayDeque<Runnable> queued = new ArrayDeque<>();
        boolean stopped;
        public void execute(Runnable work) {
            if (stopped) throw new java.util.concurrent.RejectedExecutionException();
            queued.add(work);
        }
        public void shutdown() { stopped = true; }
        public List<Runnable> shutdownNow() { shutdown(); return List.of(); }
        public boolean isShutdown() { return stopped; }
        public boolean isTerminated() { return stopped && queued.isEmpty(); }
        public boolean awaitTermination(long timeout, TimeUnit unit) { return isTerminated(); }
        void drain() { while (!queued.isEmpty()) queued.remove().run(); }
    }
    private static final class Engine implements QcInternetForwarder.Engine {
        long nextId;
        final List<Long> cancelled = new ArrayList<>();
        final List<Long> released = new ArrayList<>();
        QcNativeStateDecoder.EncodedMessage response;
        Runnable beforeNext = () -> {};
        boolean fail;
        boolean failStart;
        public long start(int type, byte[] payload, String observation) {
            if (failStart) throw new IllegalStateException("HTTP worker unavailable");
            assertArrayEquals(new byte[]{1, 2}, payload);
            assertEquals("owner", observation);
            return ++nextId;
        }
        public QcNativeStateDecoder.EncodedMessage next(long task) {
            beforeNext.run();
            if (fail) throw new IllegalStateException("network stopped");
            QcNativeStateDecoder.EncodedMessage result = response;
            response = null;
            return result;
        }
        public boolean finished(long task) { return response == null; }
        public void cancel(long task) { cancelled.add(task); }
        public void release(long task) { released.add(task); }
    }
    @Test public void deliversBinaryResponseAndReleasesSensitiveBuffers() {
        Engine engine = new Engine(); Queue queue = new Queue();
        byte[] body = new byte[]{0, (byte) 255, 3};
        engine.response = new QcNativeStateDecoder.EncodedMessage(61, body);
        List<byte[]> writes = new ArrayList<>();
        QcInternetForwarder forwarder = new QcInternetForwarder(engine,
            (task, message) -> writes.add(message.payload.clone()), queue);
        forwarder.setEnabled(true);
        byte[] request = new byte[]{1, 2};
        forwarder.accept(61, request, "owner");
        assertArrayEquals(new byte[2], request);
        queue.drain();
        assertArrayEquals(new byte[]{0, (byte) 255, 3}, writes.get(0));
        assertArrayEquals(new byte[3], body);
        assertEquals(List.of(1L), engine.released);
    }
    @Test public void disableAfterNetworkCompletionCannotSendLateReply() {
        Engine engine = new Engine(); Queue queue = new Queue();
        byte[] body = new byte[]{5};
        engine.response = new QcNativeStateDecoder.EncodedMessage(29, body);
        QcInternetForwarder forwarder = new QcInternetForwarder(engine,
            (task, message) -> fail("late response reached USB"), queue);
        forwarder.setEnabled(true);
        forwarder.accept(29, new byte[]{1, 2}, "owner");
        engine.beforeNext = () -> forwarder.setEnabled(false);
        queue.drain();
        assertArrayEquals(new byte[1], body);
        assertEquals(List.of(1L), engine.released);
        assertFalse(forwarder.current(1));
    }
    @Test public void cancelledSessionDoesNotReviveWhenEnabledAgain() {
        Engine engine = new Engine(); Queue queue = new Queue();
        List<Long> writes = new ArrayList<>();
        QcInternetForwarder forwarder = new QcInternetForwarder(engine,
            (task, message) -> writes.add(task), queue);
        forwarder.setEnabled(true);
        forwarder.accept(61, new byte[]{1, 2}, "owner");
        forwarder.cancelAll();
        forwarder.setEnabled(false);
        forwarder.setEnabled(true);
        queue.drain();
        assertTrue(writes.isEmpty());
        assertEquals(List.of(1L), engine.released);
    }
    @Test public void shutdownRetiresQueuedTasksRatherThanDiscardingCleanup() throws Exception {
        Engine engine = new Engine(); Queue queue = new Queue();
        QcInternetForwarder forwarder = new QcInternetForwarder(engine,
            (task, message) -> fail("closed task wrote"), queue);
        forwarder.setEnabled(true);
        forwarder.accept(61, new byte[]{1, 2}, "owner");
        forwarder.close();
        assertFalse(forwarder.awaitTermination());
        queue.drain();
        assertTrue(forwarder.awaitTermination());
        assertEquals(List.of(1L), engine.released);
    }
    @Test public void exceptionAndRejectedWorkBothReleaseNativeOwnership() {
        Engine engine = new Engine(); Queue queue = new Queue();
        QcInternetForwarder forwarder = new QcInternetForwarder(engine,
            (task, message) -> fail("failed task wrote"), queue);
        forwarder.setEnabled(true);
        engine.fail = true;
        forwarder.accept(29, new byte[]{1, 2}, "owner");
        queue.drain();
        queue.shutdown();
        forwarder.accept(61, new byte[]{1, 2}, "owner");
        assertEquals(List.of(1L, 2L), engine.released);
    }
    @Test public void notificationsAndDisabledRequestsDoNotStartHttp() {
        Engine engine = new Engine(); Queue queue = new Queue();
        QcInternetForwarder forwarder = new QcInternetForwarder(engine,
            (task, message) -> fail("notification wrote"), queue);
        byte[] request = new byte[]{1, 2};
        forwarder.accept(29, request, "owner");
        forwarder.setEnabled(true);
        forwarder.accept(18, new byte[]{1, 2}, "owner");
        assertArrayEquals(new byte[2], request);
        assertEquals(0, engine.nextId);
        assertTrue(queue.queued.isEmpty());
    }
    @Test public void failedHttpAdmissionCannotStopTheUsbReader() {
        Engine engine = new Engine(); Queue queue = new Queue();
        engine.failStart = true;
        QcInternetForwarder forwarder = new QcInternetForwarder(engine,
            (task, message) -> fail("unadmitted work wrote"), queue);
        forwarder.setEnabled(true);
        byte[] request = new byte[]{1, 2};
        forwarder.accept(29, request, "owner");
        assertArrayEquals(new byte[2], request);
        assertEquals(0, engine.nextId);
        assertTrue(queue.queued.isEmpty());
    }
}
