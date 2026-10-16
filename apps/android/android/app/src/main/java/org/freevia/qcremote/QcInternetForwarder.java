package org.freevia.qcremote;

import java.util.Arrays;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/** Owns native HTTP tasks; request data never crosses the WebView bridge. */
final class QcInternetForwarder implements AutoCloseable {
    interface Engine {
        long start(int type, byte[] payload, String observation);
        QcNativeStateDecoder.EncodedMessage next(long task) throws Exception;
        boolean finished(long task);
        void cancel(long task);
        void release(long task);
    }
    interface Writer {
        void write(long task, QcNativeStateDecoder.EncodedMessage message);
    }
    private final Engine engine;
    private final Writer writer;
    private final ExecutorService workers;
    private final ConcurrentHashMap<Long, AtomicBoolean> tasks = new ConcurrentHashMap<>();
    private volatile boolean enabled;
    private volatile boolean closed;

    QcInternetForwarder(Engine engine, Writer writer) {
        this(engine, writer, Executors.newCachedThreadPool());
    }
    QcInternetForwarder(Engine engine, Writer writer, ExecutorService workers) {
        this.engine = engine;
        this.writer = writer;
        this.workers = workers;
    }
    synchronized void setEnabled(boolean value) {
        enabled = value && !closed;
        if (!enabled) cancelAll();
    }
    boolean current(long task) {
        AtomicBoolean cancelled = tasks.get(task);
        return enabled && !closed && cancelled != null && !cancelled.get();
    }
    synchronized void accept(int type, byte[] payload, String observation) {
        try {
            if (!enabled || closed || type == 18) return;
            long task = engine.start(type, payload, observation);
            if (task <= 0) return;
            AtomicBoolean cancelled = new AtomicBoolean();
            tasks.put(task, cancelled);
            if (!enabled || closed) {
                cancelled.set(true);
                engine.cancel(task);
            }
            try {
                workers.execute(() -> run(task, cancelled));
            } catch (RuntimeException rejected) {
                tasks.remove(task, cancelled);
                engine.cancel(task);
                engine.release(task);
            }
        } catch (RuntimeException unavailable) {
            // Failure to start HTTP work must not terminate the USB reader.
        } finally {
            Arrays.fill(payload, (byte) 0);
        }
    }
    private void run(long task, AtomicBoolean cancelled) {
        try {
            while (current(task)) {
                QcNativeStateDecoder.EncodedMessage message = engine.next(task);
                if (message != null) {
                    try {
                        if (!current(task)) break;
                        writer.write(task, message);
                    } finally {
                        Arrays.fill(message.payload, (byte) 0);
                    }
                } else if (engine.finished(task)) {
                    break;
                }
            }
        } catch (Exception stopped) {
            // The USB owner handles transport failures. Never log HTTP data.
        } finally {
            cancelled.set(true);
            tasks.remove(task, cancelled);
            engine.cancel(task);
            engine.release(task);
        }
    }
    synchronized void cancelAll() {
        tasks.forEach((task, cancelled) -> {
            cancelled.set(true);
            engine.cancel(task);
        });
    }
    @Override public synchronized void close() {
        closed = true;
        enabled = false;
        cancelAll();
        // Tasks retire themselves, including native join, before decoder close.
        // Do not discard queued cleanup callbacks with shutdownNow().
        workers.shutdown();
    }
    boolean awaitTermination() throws InterruptedException {
        return workers.awaitTermination(Long.MAX_VALUE, TimeUnit.NANOSECONDS);
    }
}
